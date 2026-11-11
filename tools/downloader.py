#!/usr/bin/env python3
"""Local Iota-compatible downloader for ParvatiMusic.

The bot resolves YouTube metadata itself (search works without any API), but
audio bytes come from an Iota-style endpoint:

    GET /download/<media-id>?kind=audio&key=<key>&q=<title>
    header X-API-Key: <key>

This service implements that contract on top of yt-dlp, so the bot can play
without an external Iota/Yuki account. `yt:<video-id>` is the only media id it
understands (the only one the built-in search produces). `kind=video` merges a
720p H.264 stream with its audio into a real mp4 — the file the Telegram voice
chat streams at 1280x720/30fps and the mini app plays in its <video> element.
There is no generated cover-video fallback.

YouTube refuses anonymous media fetches from datacenter IPs
("Sign in to confirm you're not a bot", HTTP 403 on the media URL). Three
strategies are tried in order:

  1. a previously cached file on disk (cache/dl/) — instant, no network;
  2. YouTube, optionally with a cookies jar and/or a PO-token provider;
  3. SoundCloud search for the same title (`q=`), which is not IP-gated.

Whatever wins is written to the cache, so a replay or a queue re-add never
hits the network again.

Environment:
    PORT          listen port (default 8099)
    API_KEY       shared key; empty = accept any (default local-dev-key)
    COOKIES_FILE  Netscape cookie jar handed to yt-dlp (helps on datacenter IPs)
    POT_URL       bgutil PO-token provider, e.g. http://127.0.0.1:4416
    JS_RUNTIME    JS runtime for YouTube challenges (default node)
    YTDLP         yt-dlp binary (default: yt-dlp on PATH)
    CACHE_DIR     finished-file cache (default cache/dl)
    NO_FALLBACK   "1" disables the SoundCloud fallback
"""

import glob
import os
import re
import shutil
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs, unquote, quote

PORT = int(os.environ.get("PORT", "8099"))
API_KEY = os.environ.get("API_KEY", "local-dev-key")
# A second key is allowed so the bot can reach this sidecar as the fallback
# backend of the real Iota API, where the key is a different one.
API_KEY_ALT = os.environ.get("API_KEY_ALT", "")
# A cookie jar makes YouTube treat this host like a real browser session.
# `tools/yt_cookies.py` writes one; a logged-in export works even better.
_DEFAULT_COOKIES = os.path.join(os.path.dirname(os.path.abspath(__file__)), "cookies.txt")
COOKIES_FILE = os.environ.get("COOKIES_FILE") or (
    _DEFAULT_COOKIES if os.path.exists(_DEFAULT_COOKIES) else ""
)
POT_URL = os.environ.get("POT_URL", "http://127.0.0.1:4416")
YTDLP = os.environ.get("YTDLP", "yt-dlp")
# yt-dlp needs a JS runtime to solve YouTube's sig/n challenges; without it the
# player response yields storyboards only.
JS_RUNTIME = os.environ.get("JS_RUNTIME", "node")
CACHE_DIR = os.environ.get("CACHE_DIR", "cache/dl")
NO_FALLBACK = os.environ.get("NO_FALLBACK", "") == "1"
# `""` = yt-dlp's own client selection, then two clients that are less
# aggressively bot-gated.
CLIENT_CHAIN = ["", "android_vr", "tv"]

MEDIA_ID = re.compile(r"^([a-z]{2,6}):([A-Za-z0-9_.:-]{2,120})$")
# Audio is delivered as 192 kbps MP3: the mini app and the voice engine both
# take it without a codec dance, and 192 k is the quality floor asked for.
AUDIO_BITRATE = os.environ.get("AUDIO_BITRATE", "192K")
# 720p is what Telegram voice chats accept and what the mini app shows. The
# ladder prefers H.264 mp4 + m4a audio, then any 720p merge, then a
# progressive mp4, and finally the universally available 360p itag 18.
VIDEO_FORMAT = (
    "bestvideo[height<=720][ext=mp4][vcodec^=avc1]+bestaudio[ext=m4a]/"
    "bestvideo[height<=720][vcodec^=avc1]+bestaudio/"
    "bestvideo[height<=720][ext=mp4]+bestaudio[ext=m4a]/"
    "best[height<=720][ext=mp4]/18/"
    "best[height<=720]/best"
)
VIDEO_TIMEOUT = int(os.environ.get("VIDEO_TIMEOUT", "420"))
# A second, freshly-extracted attempt catches YouTube's transient media 403s
# (the stream URL is bound to the IP that resolved it, and this box has a
# rotating egress). Never more than two: a genuinely gated video will not
# start working on try three and the user should get the error promptly.
VIDEO_PASSES = 2
MIN_BYTES = 100_000          # anything smaller is an error page, not media
CACHE_TTL = 6 * 3600         # keep finished files for six hours
_locks_guard = threading.Lock()
_locks: dict[str, threading.Lock] = {}

# Words that only make sense for the video page, never for a music search.
_NOISE = re.compile(
    r"\b(official|music|video|lyrical?|lyrics?|audio|full|song|hd|uhd|4k|8k|"
    r"visuali[sz]er|remaster(ed)?|out now|new|2024|2025|2026|teaser|trailer|"
    r"exclusive|premiere|behind the scenes|making of)\b",
    re.I,
)


def key_lock(key: str) -> threading.Lock:
    """One lock per media id: a slow download no longer blocks every other
    request (that serialisation is what made the mini app feel stuck)."""
    with _locks_guard:
        lock = _locks.get(key)
        if lock is None:
            lock = threading.Lock()
            _locks[key] = lock
        return lock


def clean_query(title: str) -> str:
    """Turn a YouTube title into something SoundCloud's search can match.

    `KEH BHI DEY | KASM | MAXXTURN | OFFICIAL MUSIC VIDEO 2026` returns zero
    hits, while `KEH BHI DEY KASM` returns the song. Keep the first two
    pipe-separated chunks, drop the marketing words, cap the length.
    """
    text = title or ""
    text = re.sub(r"[\(\[\{][^\)\]\}]*[\)\]\}]", " ", text)
    parts = [p.strip() for p in re.split(r"[|•·]", text) if p.strip()]
    if len(parts) > 2:
        parts = parts[:2]
    text = " ".join(parts)
    text = _NOISE.sub(" ", text)
    text = re.sub(r"[^\w\s'&+-]", " ", text, flags=re.UNICODE)
    text = " ".join(text.split())
    return text[:70].strip()


def _transient_error(err: str) -> bool:
    """True when a failure looks like a retryable upstream hiccup."""
    low = (err or "").lower()
    return any(k in low for k in (
        "ffprobe", "429", "too many requests", "timeout", "timed out",
        "temporarily", "try again", "connection reset", "502", "503",
    ))


def sc_queries(title: str) -> list[str]:
    """Search strings to try, most specific first."""
    out: list[str] = []
    clean = clean_query(title)
    if clean:
        out.append(clean)
        words = clean.split()
        if len(words) > 4:
            out.append(" ".join(words[:4]))
    raw = " ".join((title or "").split())[:70]
    if raw and raw not in out:
        out.append(raw)
    seen, uniq = set(), []
    for q in out:
        if q.lower() not in seen and len(q) > 2:
            seen.add(q.lower())
            uniq.append(q)
    return uniq[:3]



_MIME = {
    ".mp3": "audio/mpeg", ".m4a": "audio/mp4", ".webm": "audio/webm",
    ".opus": "audio/ogg", ".ogg": "audio/ogg", ".mp4": "video/mp4",
}


def _mime_for(path: str, kind: str) -> str:
    return _MIME.get(os.path.splitext(path)[1].lower(),
                     "video/mp4" if kind == "video" else "audio/mpeg")


def _safe(name: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.-]", "_", name)[:120]


def cache_path(key: str, kind: str) -> str:
    return os.path.join(CACHE_DIR, f"{_safe(key)}.{'mp4' if kind == 'video' else 'mp3'}")


def cache_get(key: str, kind: str) -> str:
    p = cache_path(key, kind)
    try:
        st = os.stat(p)
    except OSError:
        return ""
    if st.st_size >= MIN_BYTES and time.time() - st.st_mtime < CACHE_TTL:
        return p
    return ""


def cache_put(tmp: str, key: str, kind: str) -> str:
    p = cache_path(key, kind)
    try:
        os.makedirs(CACHE_DIR, exist_ok=True)
        shutil.move(tmp, p)
        return p
    except OSError:
        return ""


def ytdlp_args(url: str, kind: str, dest: str, client: str = "") -> list[str]:
    args = [
        YTDLP,
        "--no-playlist",
        "--no-warnings",
        "--no-progress",
        "--quiet",
        "--no-part",
        # A bot-check or a 403 on one player client is often transient; let
        # yt-dlp re-extract instead of giving up on the first refusal.
        "--extractor-retries", "3",
        "--retries", "3",
        "--fragment-retries", "3",
        "--socket-timeout", "20",
        "-o",
        dest,
    ]
    if kind == "video":
        # "Play video" must be the real clip at 720p, never a cover slideshow.
        # Prefer a 720p H.264 DASH video merged with its m4a audio: H.264/AAC
        # in mp4 plays in iOS Safari (Telegram mini apps), Chrome and ffmpeg
        # alike, whereas AV1/VP9 mp4 does not. The progressive 360p itag 18 is
        # only the last resort when YouTube offers no DASH merge at all.
        args += ["-f", VIDEO_FORMAT, "--merge-output-format", "mp4"]
    else:
        # `-x` runs the extract-audio postprocessor: whatever the source
        # container is (m4a / webm-opus), the file that lands on disk is a
        # 192 kbps MP3 that every client can seek in.
        args += ["-f", "140/bestaudio[ext=m4a]/bestaudio/best",
                 "-x", "--audio-format", "mp3",
                 "--audio-quality", AUDIO_BITRATE,
                 "--postprocessor-args", f"ffmpeg:-b:a {AUDIO_BITRATE.replace('K','k')}"]
    if JS_RUNTIME:
        args += ["--js-runtimes", JS_RUNTIME]
    if COOKIES_FILE and os.path.exists(COOKIES_FILE):
        args += ["--cookies", COOKIES_FILE]
    if client:
        args += ["--extractor-args", f"youtube:player_client={client}"]
    if POT_URL:
        args += ["--extractor-args", f"youtubepot-bgutilhttp:base_url={POT_URL}"]
    args.append(url)
    return args


def _resolve_output(dest: str) -> str:
    """The file yt-dlp actually wrote.

    A postprocessor renames the output (`.part` -> `.mp3`), so the requested
    path may not exist even though the download succeeded. Take `dest` when it
    is there, else the largest `dest.*` sibling that is not a temp file.
    """
    try:
        if os.path.getsize(dest) > 0:
            return dest
    except OSError:
        pass
    best, best_size = "", 0
    for cand in glob.glob(dest + ".*"):
        if cand.endswith((".part", ".ytdl", ".temp", ".json")):
            continue
        try:
            size = os.path.getsize(cand)
        except OSError:
            continue
        if size > best_size:
            best, best_size = cand, size
    return best


def _size_of(path: str) -> int:
    """Size of whatever yt-dlp actually produced for `path` (0 when absent)."""
    produced = _resolve_output(path)
    try:
        return os.path.getsize(produced) if produced else 0
    except OSError:
        return 0


def fetch(url: str, kind: str, dest: str, timeout: int, client: str = "") -> tuple[bool, str]:
    """Run yt-dlp into `dest`; returns (ok, last_error_line)."""
    try:
        os.makedirs(os.path.dirname(dest) or ".", exist_ok=True)
    except OSError:
        pass
    try:
        proc = subprocess.run(
            ytdlp_args(url, kind, dest, client),
            capture_output=True, text=True, timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        return False, "timeout"
    produced = _resolve_output(dest)
    try:
        size = os.path.getsize(produced) if produced else 0
    except OSError:
        size = 0
    if proc.returncode == 0 and size >= MIN_BYTES:
        return True, ""
    if produced and size < MIN_BYTES:
        try:
            os.remove(produced)
        except OSError:
            pass
    err = (proc.stderr or "").strip().splitlines()
    return False, (err[-1] if err else f"exit {proc.returncode}")[:300]


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *a):  # keep the bot's stdout clean
        sys.stderr.write("[downloader] " + fmt % a + "\n")

    def _deny(self, code: int, msg: str):
        body = msg.encode()
        self.send_response(code)
        self.send_header("content-type", "text/plain; charset=utf-8")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _send_file(self, path: str, kind: str):
        size = os.path.getsize(path)
        self.send_response(200)
        self.send_header("content-type", _mime_for(path, kind))
        self.send_header("content-length", str(size))
        self.send_header("cache-control", "no-store")
        self.end_headers()
        try:
            with open(path, "rb") as f:
                while True:
                    buf = f.read(1 << 16)
                    if not buf:
                        break
                    self.wfile.write(buf)
        except (BrokenPipeError, ConnectionResetError):
            pass  # bot seeked away / cancelled the stream

    def do_GET(self):
        url = urlparse(self.path)
        if url.path == "/health":
            return self._deny(200, "ok")

        if not url.path.startswith("/download/"):
            return self._deny(404, "not found")

        qs = parse_qs(url.query)
        key = qs.get("key", [""])[0] or self.headers.get("X-API-Key", "")
        if API_KEY and key != API_KEY and key != API_KEY_ALT:
            return self._deny(401, "bad key")

        # the bot percent-encodes the id, so "yt:<id>" arrives as "yt%3A<id>"
        media = unquote(url.path[len("/download/"):]).split("?")[0]
        m = MEDIA_ID.match(media)
        if not m:
            return self._deny(400, "unsupported media id")
        prov = m.group(1)
        vid = m.group(2)
        kind = qs.get("kind", ["audio"])[0]
        if kind not in ("audio", "video"):
            kind = "audio"
        title = qs.get("q", [""])[0].strip()

        # 1. cache
        hit = cache_get(vid, kind)
        if hit:
            return self._send_file(hit, kind)

        # NOT ".part": yt-dlp treats an output path ending in .part as an
        # in-flight download, skips the postprocessor and leaves a 0-byte file
        # behind ("unable to obtain file audio codec with ffprobe").
        tmp = os.path.join(CACHE_DIR, f".{_safe(vid)}-{os.getpid()}-{threading.get_ident()}.media")
        os.makedirs(CACHE_DIR, exist_ok=True)
        with key_lock(f"{vid}:{kind}"):
            hit = cache_get(vid, kind)  # another thread may have finished it
            if hit:
                return self._send_file(hit, kind)
            # YouTube answers differently per player client, and the refusal
            # ("confirm you're not a bot") is per-client, not per-video: walk a
            # short chain before declaring the video unavailable.
            ok, err = False, ""
            # Only a `yt:` id is a YouTube video id; a catalogue id from
            # another provider has to be matched by title instead.
            if prov == "yt":
                passes = VIDEO_PASSES if kind == "video" else 1
                for pass_no in range(1, passes + 1):
                    for client in CLIENT_CHAIN:
                        ok, err = fetch(f"https://www.youtube.com/watch?v={vid}", kind,
                                        tmp, VIDEO_TIMEOUT if kind == "video" else 150, client)
                        if ok:
                            self.log_message("yt ok %s via %r pass %d (%d bytes)", vid,
                                             client or "default", pass_no, _size_of(tmp))
                            break
                        self.log_message("yt miss %s via %r pass %d (%s)", vid,
                                         client or "default", pass_no, err[:120])
                        # A failed merge can leave a partial file behind; the
                        # next attempt must not mistake it for the result.
                        for leftover in glob.glob(tmp + "*"):
                            try:
                                os.remove(leftover)
                            except OSError:
                                pass
                    if ok:
                        break
                    if pass_no < passes:
                        self.log_message("video pass %d/%d failed for %s, re-extracting",
                                         pass_no, passes, vid)
                        time.sleep(1.5)
            else:
                err = f"no direct source for {prov}: id"
            if not ok and kind == "video":
                # A video request must never degrade to a different song or a
                # cover image: SoundCloud is audio-only, so fail fast and let
                # the bot report the honest error for this exact id.
                self.log_message("yt failed %s (%s) → no video fallback", vid, err)
            elif not ok and not NO_FALLBACK and title:
                # 2. SoundCloud: same song, different CDN, no bot check. The
                # raw YouTube title rarely matches there, so try the cleaned
                # variants before giving up.
                self.log_message("yt failed %s (%s) → scsearch", vid, err)
                sc_err = ""
                for q in sc_queries(title):
                    # SoundCloud throttles bursts and then hands back a 0-byte
                    # HLS body, which yt-dlp reports as "unable to obtain file
                    # audio codec with ffprobe". One short retry turns those
                    # transient misses into a hit.
                    for attempt in (1, 2):
                        ok, sc_err = fetch(f"scsearch1:{q}", kind, tmp, 120)
                        if ok:
                            break
                        if not _transient_error(sc_err):
                            break
                        time.sleep(1.5)
                    if ok:
                        self.log_message("sc ok %s via %r (%d bytes)", vid, q,
                                         _size_of(tmp))
                        break
                    self.log_message("sc miss %r (%s)", q, sc_err)
                if not ok:
                    err = f"{err} | sc: {sc_err}"
            if not ok:
                try:
                    os.remove(tmp)
                except OSError:
                    pass
                return self._deny(502, "yt-dlp: " + err)
            produced = _resolve_output(tmp)
            if not produced:
                return self._deny(502, "yt-dlp: no output file")
            final = cache_put(produced, vid, kind) or produced

        self._send_file(final, kind)


def main():
    if shutil.which(YTDLP) is None:
        sys.exit(f"downloader: {YTDLP} not found on PATH")
    os.makedirs(CACHE_DIR, exist_ok=True)
    srv = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    srv.daemon_threads = True
    ver = subprocess.run([YTDLP, "--version"], capture_output=True, text=True).stdout.strip()
    print(f"downloader: http://127.0.0.1:{PORT}  (yt-dlp {ver}, cache {CACHE_DIR})")
    if COOKIES_FILE:
        print(f"downloader: cookies {COOKIES_FILE}")
    if POT_URL:
        print(f"downloader: po-token {POT_URL}")
    if NO_FALLBACK:
        print("downloader: soundcloud fallback disabled")
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
