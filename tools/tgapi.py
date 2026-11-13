#!/usr/bin/env python3
"""Telegram Bot API sidecar for the Parvati mini app.

The bot itself speaks MTProto (see `bot/src/ferogram_bot.rs`), which is great
for voice chats but awkward for the "social" surface the mini app needs:
profile photos, the administrator roster with custom titles, sticker packs and
premium (custom) emoji.  All of that lives behind the plain Bot API, so this
process is a thin, cached HTTP bridge:

    mini app  ->  bot/src/webapp.rs  ->  127.0.0.1:8098  ->  api.telegram.org

Endpoints (all JSON unless noted)
    GET  /health
    GET  /chat?chat_id=            chat title / username / member count
    GET  /admins?chat_id=          owner + administrators (+ custom titles)
    GET  /member?chat_id=&user_id= single membership snapshot
    GET  /user?user_id=            private-chat profile (bio, username, photos)
    GET  /avatar?user_id=&s=       image bytes (404 when the user has none)
    GET  /set?name=                sticker / custom-emoji pack
    GET  /img?fid=&uid=            image bytes for a Telegram file id
    POST /send                     text (with entities) | sticker | photo
                                   sticker accepts a file_id OR media_key (the
                                   bot's cached MTProto document, uploaded)
    POST /react                    set a reaction on a message
    POST /mod                      promote | demote | kick | ban | unban | mute
    POST /delete                   delete a message

Nothing is trusted: every call is validated, cached on disk under
`cache/tg/`, and the socket only ever binds to loopback.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from io import BytesIO
from pathlib import Path

try:  # optional: avatars are downscaled when Pillow is present
    from PIL import Image
except Exception:  # pragma: no cover
    Image = None

ROOT = Path(__file__).resolve().parent.parent
CACHE = ROOT / "cache" / "tg"
CACHE.mkdir(parents=True, exist_ok=True)

def _env(key: str, default: str = "") -> str:
    """Resolve a setting the same way the Rust hub does: env var wins, else .env.

    bot/src/webapp.rs loads .env with "existing env wins" and sends TGAPI_KEY
    as the `x-tgapi-key` header on *every* request to this bridge. If this side
    only read os.environ (falling back to a hard-coded default), a custom key
    set in .env would be ignored here and every authenticated call would fail
    with 401 while the hub kept sending that custom value. TOKEN always used
    this fallback; TGAPI_KEY / TGAPI_PORT now share it, so the two processes
    cannot disagree no matter how the bridge is launched (supervise.sh,
    systemd, or by hand) — even when the supervisor doesn't export .env.
    """
    val = os.environ.get(key, "").strip()
    if val:
        return val
    dotenv = ROOT / ".env"
    if dotenv.exists():
        for line in dotenv.read_text(encoding="utf-8", errors="ignore").splitlines():
            line = line.strip()
            if line.startswith(key + "="):
                return line.split("=", 1)[1].strip().strip('"').strip("'")
    return default


TOKEN = _env("TOKEN")
KEY = _env("TGAPI_KEY", "local-dev-key")
PORT = int(_env("TGAPI_PORT", "8098"))
API = f"https://api.telegram.org/bot{TOKEN}/"
FILE = f"https://api.telegram.org/file/bot{TOKEN}/"

_dl_lock = threading.Semaphore(6)
_json_lock = threading.Lock()
_json_cache: dict[str, tuple[float, dict]] = {}


# ───────────────────────────── helpers ─────────────────────────────

def api(method: str, **params) -> dict:
    """POST a Bot API call; returns the parsed envelope (never raises)."""
    data = json.dumps(params).encode()
    req = urllib.request.Request(
        API + method,
        data=data,
        headers={"content-type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=25) as r:
            return json.load(r)
    except urllib.error.HTTPError as e:
        try:
            return json.load(e)
        except Exception:
            return {"ok": False, "description": f"HTTP {e.code}"}
    except Exception as e:  # network / timeout
        return {"ok": False, "description": str(e)}


def cached_json(key: str, ttl: float, fn):
    now = time.time()
    with _json_lock:
        hit = _json_cache.get(key)
        if hit and now - hit[0] < ttl:
            return hit[1]
    val = fn()
    with _json_lock:
        _json_cache[key] = (now, val)
    return val


def file_bytes(file_id: str, unique: str = "") -> tuple[bytes, str] | None:
    """Download a Telegram file (getFile + CDN fetch), cached on disk."""
    if not file_id:
        return None
    name = hashlib.sha1((unique or file_id).encode()).hexdigest()
    for p in CACHE.glob(name + ".*"):
        return p.read_bytes(), p.suffix.lstrip(".")
    with _dl_lock:
        for p in CACHE.glob(name + ".*"):
            return p.read_bytes(), p.suffix.lstrip(".")
        got = api("getFile", file_id=file_id)
        if not got.get("ok"):
            return None
        path = got["result"].get("file_path")
        if not path:
            return None
        try:
            with urllib.request.urlopen(FILE + path, timeout=30) as r:
                data = r.read()
        except Exception:
            return None
        ext = os.path.splitext(path)[1].lstrip(".").lower() or "bin"
        (CACHE / f"{name}.{ext}").write_bytes(data)
        return data, ext


def display_name(u: dict) -> str:
    if not u:
        return ""
    n = " ".join(x for x in (u.get("first_name", ""), u.get("last_name", "")) if x).strip()
    if n:
        return n
    un = u.get("username") or ""
    if un:
        return f"@{un}"
    # Accounts with no visible name (deleted / hidden profile) still get a
    # readable label instead of a bare numeric id.
    uid = str(u.get("id", ""))
    return f"User {uid[-4:]}" if uid else "Member"


def member_json(m: dict, me_id: int) -> dict:
    u = m.get("user") or {}
    uid = u.get("id")
    status = m.get("status", "member")
    return {
        "id": str(uid),
        "name": display_name(u),
        "username": u.get("username", ""),
        "photo": f"/api/avatar/{uid}",
        "has_photo": bool(u.get("photo")),
        "status": status,
        "custom": m.get("custom_title", "") or "",
        "bot": bool(u.get("is_bot")),
        "me": uid == me_id,
    }


def thumb_url(st: dict) -> str:
    t = st.get("thumbnail") or {}
    fid = t.get("file_id", "")
    if not fid:
        return ""
    return "/api/tgimg?fid=" + urllib.parse.quote(fid, safe="")


# ───────────────────────────── routes ─────────────────────────────

def r_chat(q) -> tuple[int, dict]:
    cid = q.get("chat_id", [""])[0]
    if not cid:
        return 400, {"error": "chat_id"}
    res = cached_json(f"chat:{cid}", 30, lambda: api("getChat", chat_id=cid))
    if not res.get("ok"):
        return 502, {"error": res.get("description", "getChat failed")}
    c = res["result"]
    count = api("getChatMemberCount", chat_id=cid)
    return 200, {
        "id": str(c.get("id")),
        "title": c.get("title", ""),
        "username": c.get("username", ""),
        "type": c.get("type", ""),
        "description": c.get("description", ""),
        "members": count.get("result", 0) if count.get("ok") else 0,
        "invite": c.get("invite_link", "") or "",
        "photo": f"/api/chatphoto/{cid}" if c.get("photo") else "",
    }


def r_admins(q) -> tuple[int, dict]:
    cid = q.get("chat_id", [""])[0]
    if not cid:
        return 400, {"error": "chat_id"}

    def fetch():
        return api("getChatAdministrators", chat_id=cid)

    res = cached_json(f"admins:{cid}", 45, fetch)
    if not res.get("ok"):
        return 502, {"error": res.get("description", "getChatAdministrators failed")}
    me = api("getMe")
    me_id = me["result"]["id"] if me.get("ok") else 0
    out = [member_json(m, me_id) for m in res["result"]]
    out.sort(key=lambda m: (m["status"] != "creator", m["name"].lower()))
    return 200, {"members": out}


def r_member(q) -> tuple[int, dict]:
    cid = q.get("chat_id", [""])[0]
    uid = q.get("user_id", [""])[0]
    if not cid or not uid:
        return 400, {"error": "chat_id, user_id"}
    res = api("getChatMember", chat_id=cid, user_id=uid)
    if not res.get("ok"):
        return 404, {"error": res.get("description", "getChatMember failed")}
    m = res["result"]
    return 200, member_json(m, 0)


def r_user(q) -> tuple[int, dict]:
    """Profile details behind the "tap a name/avatar" card in the mini app.

    `getChat` with a *user* id returns the private-chat object: exact first /
    last name, username, bio and whether the account has a photo. The Bot API
    does not expose last-seen or a join date for privacy reasons, so the card
    only shows what Telegram actually makes public.
    """
    uid = q.get("user_id", [""])[0]
    if not uid:
        return 400, {"error": "user_id"}
    res = cached_json(f"user:{uid}", 180, lambda: api("getChat", chat_id=uid))
    if not res.get("ok"):
        return 404, {"error": res.get("description", "getChat failed")}
    c = res["result"]
    photos = api("getUserProfilePhotos", user_id=uid, limit=1)
    total = (photos.get("result") or {}).get("total_count", 0) if photos.get("ok") else 0
    return 200, {
        "id": str(c.get("id", uid)),
        "first_name": c.get("first_name", "") or "",
        "last_name": c.get("last_name", "") or "",
        "username": c.get("username", "") or "",
        "bio": c.get("bio", "") or "",
        "type": c.get("type", "private") or "private",
        "photos": total,
        "has_photo": bool(c.get("photo")),
        "premium": bool(c.get("is_premium")),
        "verified": bool(c.get("is_verified")),
        "private_forwards": bool(c.get("has_private_forwards")),
        "restricts_voice": bool(c.get("has_restricted_voice_and_video_messages")),
    }


def r_emoji(q) -> tuple[int, dict]:
    """Custom (premium) emoji documents by id — one or many, comma separated."""
    raw = q.get("ids", [""])[0]
    ids = [x for x in raw.split(",") if x]
    if not ids:
        return 400, {"error": "ids"}

    def fetch():
        return api("getCustomEmojiStickers", custom_emoji_ids=ids)

    res = cached_json("emoji:" + ",".join(ids[:20]), 3600, fetch)
    if not res.get("ok"):
        return 502, {"error": res.get("description", "getCustomEmojiStickers failed")}
    out = []
    for st in res["result"]:
        out.append({
            "id": str(st.get("custom_emoji_id", "")),
            "emoji": st.get("emoji", ""),
            "file_id": st.get("file_id", ""),
            "thumb": thumb_url(st),
            "animated": bool(st.get("is_animated")),
            "video": bool(st.get("is_video")),
        })
    return 200, {"stickers": out}


def r_avatar(q) -> tuple[int, bytes, str]:
    uid = q.get("user_id", [""])[0]
    if not uid:
        return 404, b"", "image/webp"
    try:
        px = int(q.get("s", ["96"])[0] or 96)
    except ValueError:
        px = 96
    px = max(32, min(320, px))
    key = f"av:{uid}"

    def fetch():
        r = api("getUserProfilePhotos", user_id=uid, limit=1)
        if not r.get("ok"):
            return {"fid": "", "uid": ""}
        photos = r["result"].get("photos") or []
        if not photos:
            return {"fid": "", "uid": ""}
        sizes = photos[0]
        best = max(sizes, key=lambda s: (s.get("width", 0) * s.get("height", 0)))
        return {"fid": best["file_id"], "uid": best.get("file_unique_id", "")}

    meta = cached_json(key, 900, fetch)
    if not meta["fid"]:
        return 200, placeholder_avatar(uid, px), "image/webp"
    got = file_bytes(meta["fid"], meta["uid"])
    if not got:
        return 200, placeholder_avatar(uid, px), "image/webp"
    return 200, shrink(got[0], px), "image/webp"


def shrink(data: bytes, px: int) -> bytes:
    """Downscale a profile photo to `px` — a full-size avatar is ~150 KB and a
    member list of ten of them is what made the mini app feel heavy."""
    if Image is None:
        return data
    try:
        im = Image.open(BytesIO(data)).convert("RGB")
        if max(im.size) > px:
            im.thumbnail((px, px), Image.LANCZOS)
        out = BytesIO()
        im.save(out, "WEBP", quality=80, method=4)
        return out.getvalue()
    except Exception:
        return data


_PLACEHOLDERS: dict = {}


def placeholder_avatar(uid: str, px: int) -> bytes:
    """Deterministic gradient tile for members without a photo, so the client
    never has to paint a broken image or fire a second request."""
    ck = (uid, px)
    if ck in _PLACEHOLDERS:
        return _PLACEHOLDERS[ck]
    if Image is None:
        return b""
    try:
        h = int(hashlib.md5(uid.encode()).hexdigest()[:6], 16)
        a = ((h >> 16) & 0xFF, (h >> 8) & 0xFF, h & 0xFF)
        b = (min(255, a[0] // 2 + 30), min(255, a[1] // 2 + 30), min(255, a[2] // 2 + 40))
        im = Image.new("RGB", (px, px))
        px_obj = im.load()
        for y in range(px):
            t = y / max(1, px - 1)
            row = tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(3))
            for x in range(px):
                px_obj[x, y] = row
        out = BytesIO()
        im.save(out, "WEBP", quality=70, method=4)
        data = out.getvalue()
    except Exception:
        data = b""
    _PLACEHOLDERS[ck] = data
    return data


def mime_of(data: bytes) -> str:
    if data[:4] == b"RIFF" and data[8:12] == b"WEBP":
        return "image/webp"
    if data[:3] == b"\xff\xd8\xff":
        return "image/jpeg"
    if data[:8] == b"\x89PNG\r\n\x1a\n":
        return "image/png"
    if data[:3] == b"GIF":
        return "image/gif"
    if data[4:8] == b"ftyp":
        return "video/mp4"
    return "application/octet-stream"


def r_img(q) -> tuple[int, bytes, str]:
    fid = q.get("fid", [""])[0]
    uid = q.get("uid", [""])[0]
    got = file_bytes(fid, uid)
    if not got:
        return 404, b"", "image/webp"
    return 200, got[0], mime_of(got[0])


def r_set(q) -> tuple[int, dict]:
    name = q.get("name", [""])[0]
    if not name:
        return 400, {"error": "name"}

    def fetch():
        return api("getStickerSet", name=name)

    res = cached_json(f"set:{name}", 600, fetch)
    if not res.get("ok"):
        return 404, {"error": res.get("description", "getStickerSet failed")}
    s = res["result"]
    stickers = []
    for st in s.get("stickers", []):
        stickers.append({
            "file_id": st["file_id"],
            "emoji": st.get("emoji", ""),
            "custom_emoji_id": st.get("custom_emoji_id", ""),
            "thumb": thumb_url(st),
            "animated": bool(st.get("is_animated")),
            "video": bool(st.get("is_video")),
            "w": st.get("width", 0),
            "h": st.get("height", 0),
        })
    return 200, {
        "name": s.get("name", name),
        "title": s.get("title", name),
        "type": s.get("sticker_type", "regular"),
        "count": len(stickers),
        "stickers": stickers,
    }


def api_multipart(method: str, fields: dict, file_field: str,
                  filename: str, ctype: str, data: bytes) -> dict:
    """POST one Bot API method as multipart/form-data with a new file upload.

    Only used for re-sending a sticker the bot mirrored over MTProto: a
    MTProto document id is *not* a Bot API file_id, so the exact cached bytes
    have to be uploaded again. Telegram re-issues a real file_id in the reply,
    which is why a sticker saved once can be sent by file_id from then on.
    """
    boundary = "----pv" + hashlib.sha1(
        f"{method}:{filename}:{len(data)}".encode()
    ).hexdigest()[:24]
    out = bytearray()
    for k, v in fields.items():
        out += (f"--{boundary}\r\n"
                f'Content-Disposition: form-data; name="{k}"\r\n\r\n').encode()
        out += str(v).encode() + b"\r\n"
    out += (f"--{boundary}\r\n"
            f'Content-Disposition: form-data; name="{file_field}"; filename="{filename}"\r\n'
            f"Content-Type: {ctype}\r\n\r\n").encode()
    out += data + b"\r\n"
    out += f"--{boundary}--\r\n".encode()
    req = urllib.request.Request(
        API + method,
        data=bytes(out),
        headers={"content-type": f"multipart/form-data; boundary={boundary}"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=40) as r:
            return json.load(r)
    except urllib.error.HTTPError as e:
        try:
            return json.load(e)
        except Exception:
            return {"ok": False, "description": f"HTTP {e.code}"}
    except Exception as e:
        return {"ok": False, "description": str(e)}


def send_cached_sticker(cid: str, key: str, extra: dict) -> dict:
    """Upload `cache/tg/<key>.bin` (a sticker mirrored by the bot) to Telegram.

    The bytes are the original WEBP / gzipped TGS / WEBM document, so they
    already satisfy Telegram's sticker constraints. The extension is sniffed
    from the magic bytes rather than trusted from the request.
    """
    if not re.fullmatch(r"[A-Za-z0-9_.\-]{1,96}", key or ""):
        return {"ok": False, "description": "bad media key"}
    try:
        data = (CACHE / f"{key}.bin").read_bytes()
    except Exception:
        return {"ok": False, "description": "sticker not cached"}
    if not data:
        return {"ok": False, "description": "sticker empty"}
    if data[:2] == b"\x1f\x8b":
        name, ctype = "sticker.tgs", "application/x-tgsticker"
    elif data[:4] == b"\x1a\x45\xdf\xa3":
        name, ctype = "sticker.webm", "video/webm"
    else:
        name, ctype = "sticker.webp", "image/webp"
    fields = {"chat_id": str(cid)}
    if extra.get("reply_parameters"):
        fields["reply_parameters"] = json.dumps(extra["reply_parameters"])
    if extra.get("disable_notification"):
        fields["disable_notification"] = "true"
    return api_multipart("sendSticker", fields, "sticker", name, ctype, data)


def r_send(body: dict) -> tuple[int, dict]:
    cid = body.get("chat_id")
    if not cid:
        return 400, {"error": "chat_id"}
    reply = body.get("reply_to") or 0
    extra = {}
    if reply:
        extra["reply_parameters"] = {"message_id": int(reply), "allow_sending_without_reply": True}
    if body.get("silent"):
        extra["disable_notification"] = True

    kind = body.get("kind", "text")
    if kind == "sticker":
        fid = body.get("sticker", "")
        media_key = body.get("media_key", "")
        if fid:
            res = api("sendSticker", chat_id=cid, sticker=fid, **extra)
        elif media_key:
            res = send_cached_sticker(cid, media_key, extra)
        else:
            return 400, {"error": "sticker"}
    elif kind == "photo":
        url = body.get("photo", "")
        if not url:
            return 400, {"error": "photo"}
        res = api("sendPhoto", chat_id=cid, photo=url, caption=body.get("text", ""), **extra)
    else:
        text = (body.get("text") or "").strip()
        if not text:
            return 400, {"error": "empty"}
        entities = [e for e in (body.get("entities") or []) if e.get("type")]
        if entities:
            extra["entities"] = entities
        res = api(
            "sendMessage",
            chat_id=cid,
            text=text,
            link_preview_options={"is_disabled": True},
            **extra,
        )
    if not res.get("ok"):
        return 502, {"error": res.get("description", "send failed")}
    m = res["result"]
    return 200, {
        "ok": True,
        "message_id": m.get("message_id"),
        "date": m.get("date"),
        "chat_id": str(m.get("chat", {}).get("id", cid)),
    }


def r_react(body: dict) -> tuple[int, dict]:
    cid = body.get("chat_id")
    mid = body.get("message_id")
    emoji = body.get("emoji", "❤")
    if not cid or not mid:
        return 400, {"error": "chat_id, message_id"}
    reaction = [] if emoji == "0" else [{"type": "emoji", "emoji": emoji}]
    res = api("setMessageReaction", chat_id=cid, message_id=int(mid), reaction=reaction)
    if not res.get("ok"):
        return 502, {"error": res.get("description", "reaction failed")}
    return 200, {"ok": True}


RIGHTS = {
    "can_manage_chat": True,
    "can_delete_messages": True,
    "can_restrict_members": True,
    "can_invite_users": True,
    "can_pin_messages": True,
    "can_manage_video_chats": True,
    "can_manage_topics": True,
    "can_promote_members": False,
}


def r_mod(body: dict) -> tuple[int, dict]:
    cid = body.get("chat_id")
    uid = body.get("user_id")
    act = body.get("action", "")
    if not cid or not uid:
        return 400, {"error": "chat_id, user_id"}
    if act == "promote":
        res = api("promoteChatMember", chat_id=cid, user_id=uid, **RIGHTS)
    elif act == "demote":
        res = api("promoteChatMember", chat_id=cid, user_id=uid)
    elif act == "kick":
        res = api("banChatMember", chat_id=cid, user_id=uid)
        if res.get("ok"):
            api("unbanChatMember", chat_id=cid, user_id=uid, only_if_banned=True)
    elif act == "ban":
        res = api("banChatMember", chat_id=cid, user_id=uid)
    elif act == "unban":
        res = api("unbanChatMember", chat_id=cid, user_id=uid, only_if_banned=True)
    elif act == "mute":
        res = api(
            "restrictChatMember",
            chat_id=cid,
            user_id=uid,
            permissions={"can_send_messages": False},
        )
    else:
        return 400, {"error": "action"}
    if not res.get("ok"):
        return 502, {"error": res.get("description", "mod failed")}
    # Roster changed -> drop caches so the next poll is fresh.
    for k in list(_json_cache):
        if k.startswith("admins:") or k.startswith("chat:") or k.startswith("member:"):
            _json_cache.pop(k, None)
    return 200, {"ok": True}


def r_delete(body: dict) -> tuple[int, dict]:
    cid = body.get("chat_id")
    mid = body.get("message_id")
    if not cid or not mid:
        return 400, {"error": "chat_id, message_id"}
    res = api("deleteMessage", chat_id=cid, message_id=int(mid))
    if not res.get("ok"):
        return 502, {"error": res.get("description", "delete failed")}
    return 200, {"ok": True}


def r_leave(body: dict) -> tuple[int, dict]:
    """Bot leaves a group. Called only after the Mini App checked admin rights."""
    cid = body.get("chat_id")
    if not cid:
        return 400, {"error": "chat_id"}
    res = api("leaveChat", chat_id=cid)
    if not res.get("ok"):
        return 502, {"error": res.get("description", "leave failed")}
    for k in list(_json_cache):
        if k.startswith("admins:") or k.startswith("chat:") or k.startswith("member:"):
            _json_cache.pop(k, None)
    return 200, {"ok": True}


# ───────────────────────────── http plumbing ─────────────────────────────

class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):  # keep stdout clean
        pass

    def _key_ok(self) -> bool:
        if not KEY:
            return True
        return self.headers.get("x-tgapi-key", "") == KEY

    def _send(self, code: int, payload, ctype="application/json"):
        if isinstance(payload, (dict, list)):
            data = json.dumps(payload, ensure_ascii=False).encode()
        else:
            data = payload
        self.send_response(code)
        self.send_header("content-type", ctype)
        self.send_header("content-length", str(len(data)))
        self.send_header("cache-control", "no-store")
        self.send_header("connection", "close")
        self.end_headers()
        try:
            self.wfile.write(data)
        except Exception:
            pass
        self.close_connection = True

    def do_GET(self):
        if not self._key_ok():
            return self._send(401, {"error": "unauthorized"})
        u = urllib.parse.urlparse(self.path)
        q = urllib.parse.parse_qs(u.query)
        path = u.path
        try:
            if path == "/health":
                return self._send(200, {"ok": True, "token": bool(TOKEN)})
            if path == "/chat":
                code, body = r_chat(q)
                return self._send(code, body)
            if path == "/admins":
                code, body = r_admins(q)
                return self._send(code, body)
            if path == "/member":
                code, body = r_member(q)
                return self._send(code, body)
            if path == "/user":
                code, body = r_user(q)
                return self._send(code, body)
            if path == "/set":
                code, body = r_set(q)
                return self._send(code, body)
            if path == "/emoji":
                return self._send(*r_emoji(q))
            if path == "/avatar":
                code, data, ctype = r_avatar(q)
                return self._send(code, data, ctype)
            if path == "/img":
                code, data, ctype = r_img(q)
                return self._send(code, data, ctype)
            if path == "/chatphoto":
                cid = q.get("chat_id", [""])[0]
                r = api("getChat", chat_id=cid)
                fid = ((r.get("result") or {}).get("photo") or {}).get("small_file_id", "") if r.get("ok") else ""
                got = file_bytes(fid) if fid else None
                if not got:
                    return self._send(404, b"", "image/webp")
                return self._send(200, got[0], mime_of(got[0]))
            return self._send(404, {"error": "not found"})
        except Exception as e:
            return self._send(500, {"error": str(e)})

    def do_POST(self):
        if not self._key_ok():
            return self._send(401, {"error": "unauthorized"})
        u = urllib.parse.urlparse(self.path)
        try:
            n = int(self.headers.get("content-length", "0"))
            body = json.loads(self.rfile.read(n) or b"{}")
        except Exception:
            body = {}
        try:
            if u.path == "/send":
                code, out = r_send(body)
            elif u.path == "/react":
                code, out = r_react(body)
            elif u.path == "/mod":
                code, out = r_mod(body)
            elif u.path == "/delete":
                code, out = r_delete(body)
            elif u.path == "/leave":
                code, out = r_leave(body)
            else:
                code, out = 404, {"error": "not found"}
        except Exception as e:
            code, out = 500, {"error": str(e)}
        return self._send(code, out)


def main():
    if not TOKEN:
        print("tgapi: no TOKEN found", file=sys.stderr)
        sys.exit(1)
    me = api("getMe")
    who = me.get("result", {}).get("username", "?") if me.get("ok") else "?"
    srv = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    srv.daemon_threads = True
    print(f"tgapi: bot @{who} on 127.0.0.1:{PORT}", flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()
