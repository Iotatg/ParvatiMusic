//! Mini App bridge — a tiny, dependency-free HTTP server that serves the
//! Parvati Music mini app (`miniapp/`) and a JSON API over the live
//! `Rooms` state.
//!
//! Why a hand-rolled server: the bot already ships a `health` thread with a
//! blocking `TcpListener`, and the crate deliberately keeps its dependency
//! surface small (`serde_json` only). This module follows the same shape —
//! `std::net` + one thread per connection — so it adds zero build weight.
//!
//! Two data sources feed the app:
//!   * the live `Rooms` registry (what is playing, queue, position);
//!   * `tools/tgapi.py`, a loopback Bot API bridge that knows the group
//!     roster, profile photos, sticker packs and premium emoji. Everything
//!     social (names, avatars, role tags, stickers) comes from there, so the
//!     mini app shows the real group instead of a mock.
//!
//! Endpoints
//! ---------
//!   GET  /                     mini app (index.html)
//!   GET  /assets/*             static assets
//!   GET  /api/state            full snapshot (room, playback, members, chat)
//!   GET  /api/search?q=        platform search (registry::search_tracks)
//!   GET  /api/events           server-sent events (state + new messages)
//!   GET  /api/user?id=         profile card for one chat/member id
//!   GET  /api/avatar/<uid>?s=  profile photo (proxied + cached, 32..320 px)
//!   GET  /api/tgimg?fid=       sticker / emoji image (proxied + cached)
//!   GET  /api/tgmedia/<key>    media downloaded over MTProto by the bot
//!   GET  /api/stickers?set=    sticker or premium-emoji pack
//!   GET  /api/stickers/saved?me=   per-viewer "Add to Stickers" list
//!   GET  /api/dm?me=&peer=     private thread (or thread list without peer)
//!   POST /api/dm/send          { me, peer, text } — never mirrored
//!   POST /api/toggle           pause / resume
//!   POST /api/seek             { position }
//!   POST /api/loop             { mode: 0|1|2 }
//!   POST /api/mute             { muted: bool }
//!   POST /api/queue/add        { id }
//!   POST /api/queue/remove     { index }
//!   POST /api/queue/clear      {}
//!   POST /api/play             { id } | { query }
//!   POST /api/skip             {}
//!   GET  /api/chats            one row per group (title, preview, unread, permissions)
//!   POST /api/chat/send        { text, kind, sticker, mediaKey, entities, replyTo }
//!   POST /api/chat/react       { mid, emoji }
//!   POST /api/chat/delete      { mid, id? } — delete a mirrored message
//!   POST /api/chat/select      { id } — switch the viewer to another group chat
//!   POST /api/chat/forget      { id } — remove a chat from the app (Telegram untouched)
//!   POST /api/chat/clear       wipe the current chat's mirror
//!   POST /api/chat/read        mark the current chat read for this viewer
//!   POST /api/stickers/saved   { me, action: add|remove, sticker|id }
//!   POST /api/members          { id, action: promote|demote|kick|ban|unban }
//!   POST /api/session/end      { id, name } — stop, leave, clear, summarise
//!   POST /api/settings         { autoplay?: bool, ... }
//!
//! Playback commands that only touch `RoomState` are applied directly and
//! mirrored into the voice engine through `monitor::engine_*`. Anything that
//! needs a download (new track, skip) is handed to the bot's own play path
//! through the hooks below, so the mini app and the chat share one code path.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Seek, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use crate::assistant::AssistantManager;
use crate::models::Track;
use crate::platforms::iota::ProgressFn;
use crate::platforms::registry;
use crate::room::{RoomState, Rooms};

const MAX_BODY: usize = 64 * 1024;
const CHAT_CAP: usize = 300;
const ROSTER_TTL: u64 = 20;
/// How long a "user X is admin of group Y" answer stays usable.
const ADMIN_TTL: u64 = 120;

/* ─────────────────────────── hooks ─────────────────────────── */

type PlayHook = Box<dyn Fn(i64, String) + Send + Sync + 'static>;
type SkipHook = Box<dyn Fn(i64) + Send + Sync + 'static>;

static PLAY_HOOK: OnceLock<Mutex<Option<PlayHook>>> = OnceLock::new();
static SKIP_HOOK: OnceLock<Mutex<Option<SkipHook>>> = OnceLock::new();
/// Unix time until which a mini-app play is being downloaded. The room
/// monitor checks this before advancing, so a tap on song B cannot be
/// overridden by autoplay picking song C while B is still downloading.
static PLAY_PENDING_UNTIL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// True while a mini-app play is in flight (bounded, so a stuck download can
/// never freeze autoplay forever).
pub fn play_pending() -> bool {
    let until = PLAY_PENDING_UNTIL.load(Ordering::Relaxed);
    until > 0 && now_secs() < until
}

fn mark_play_pending() {
    PLAY_PENDING_UNTIL.store(now_secs() + 180, Ordering::Relaxed);
}

fn clear_play_pending() {
    PLAY_PENDING_UNTIL.store(0, Ordering::Relaxed);
}

fn play_slot() -> &'static Mutex<Option<PlayHook>> {
    PLAY_HOOK.get_or_init(|| Mutex::new(None))
}
fn skip_slot() -> &'static Mutex<Option<SkipHook>> {
    SKIP_HOOK.get_or_init(|| Mutex::new(None))
}

/// Register the bot's real "resolve → download → play" entry point.
/// Call once during module wiring (see `docs/miniapp.md`).
pub fn set_play_hook(f: PlayHook) {
    *play_slot().lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
}

/// Register the bot's real "skip to next" entry point.
pub fn set_skip_hook(f: SkipHook) {
    *skip_slot().lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
}

/* ─────────────────────────── state ─────────────────────────── */

pub struct MiniApp {
    rooms: Rooms,
    chat_id: i64,
    title: Mutex<String>,
    dir: PathBuf,
    /// Mirrored messages, **one deque per chat**. A single global queue mixed
    /// every group the bot sits in into one timeline (and persisted whichever
    /// chat happened to be "active" at save time), which is why a second group
    /// looked like it had no chat of its own.
    chats: Mutex<HashMap<i64, VecDeque<Value>>>,
    /// Chat registry: id → `{id, title, kind, members, last_text, last_date}`.
    /// Drives the chat list and survives restarts (`cache/chats.json`).
    chat_meta: Mutex<HashMap<i64, Value>>,
    /// Chats whose `cache/mirror-<id>.json` was already read from disk.
    chats_loaded: Mutex<std::collections::HashSet<i64>>,
    /// Which chat each viewer is looking at (`user id → chat id`).
    view_chat: Mutex<HashMap<i64, i64>>,
    /// `(user id, chat id) → (is admin, checked at)`. Chat-list frames are
    /// built every 1.5 s; asking the Telegram bridge about every other group
    /// on each tick stalled the SSE loop. `/api/chats` refreshes the entries,
    /// the stream only reads them.
    admin_cache: Mutex<HashMap<(i64, i64), (bool, u64)>>,
    /// `uid:chat` → newest message date that viewer has seen (unread badge).
    last_read: Mutex<HashMap<String, i64>>,
    /// Bans performed through this app, per chat (`cache/banned-<chat>.json`).
    /// The Bot API cannot enumerate a group's ban history, so this local list
    /// is what the "Banned users" sheet shows and what "Unban" acts on.
    banned: Mutex<HashMap<i64, Vec<Value>>>,
    banned_loaded: Mutex<std::collections::HashSet<i64>>,
    /// Root of the JSON caches. Injectable so the per-chat tests never write
    /// into the live `cache/` directory.
    cache_dir: PathBuf,
    /// Full roster: administrators from the Bot API + everyone who talked.
    members: Mutex<Vec<Value>>,
    /// Everyone seen talking (id → display data), merged into the roster.
    seen: Mutex<Vec<Value>>,
    member_count: Mutex<usize>,
    invite: Mutex<String>,
    settings: Mutex<Value>,
    quote: Mutex<String>,
    /// True from the first successful play until `/end` (or the queue runs
    /// out). Drives the LIVE badge, the roster's "in voice" flags and the
    /// session summary the app shows once the call is over.
    live: Mutex<bool>,
    session_started: Mutex<u64>,
    /// Tracks started during the current session (reset when it ends).
    session_tracks: Mutex<usize>,
    /// Details of the last finished session: who ended it, when, how long it
    /// ran and how many tracks it played.
    last_session: Mutex<Value>,
    /// uid / file-id → (bytes, mime, fetched-at) so avatars and stickers are
    /// fetched from Telegram once, not on every repaint.
    media: Mutex<HashMap<String, (Vec<u8>, String, u64)>>,
    roster_at: Mutex<u64>,
    /// Every track `/api/search` returned, keyed by id. The UI only ever sends
    /// an id back (`/api/play`, `/api/queue/add`), so without this cache a
    /// search result could not be played or queued — it is not in the room yet.
    search_cache: Mutex<Vec<Track>>,
    /// Private one-to-one conversations (`viewer|peer`), never mirrored to
    /// Telegram and never part of the room chat.
    dms: Mutex<Vec<Value>>,
    /// A track the user just picked that is still downloading. Painted
    /// immediately so every tap has feedback instead of a 30 s dead button.
    pending: Mutex<Value>,
    /// Last playback failure ({title, error, at}) — the app shows it once.
    play_error: Mutex<Value>,
    /// Last JSON pushed over SSE is tracked per connection (`Cursor`): sharing
    /// one string between viewers made one client's frame suppress the rest.
    /// True when the last roster pass got a real group-call participant list.
    /// Without it the app must not hide messages by "in voice" (it would hide
    /// everyone), so it falls back to showing the whole mirrored chat.
    vc_known: Mutex<bool>,
    /// Tracks this room played, newest first. Feeds the "Recently played"
    /// strip in the queue tab and survives a restart (`cache/recent-<chat>`).
    recent: Mutex<Vec<Value>>,
    /// Liked tracks, persisted per chat (`cache/favs-<chat>.json`).
    favs: Mutex<Vec<Value>>,
    /// Unix time at which the sleep timer stops playback (0 = off).
    sleep_at: Mutex<u64>,
    /// One roster pass at a time: a burst of requests used to start several
    /// 25-call member lookups at once, which is what made the app stutter.
    roster_busy: AtomicBool,
    /// Optional shared secret (`MINIAPP_TOKEN`) for trusted integrations. It
    /// never grants a *user* identity — only the control plane.
    token: String,
    /// Bot token. Telegram derives the WebApp `initData` secret from it, so
    /// only this bot's WebApp can mint credentials the server accepts.
    bot_token: String,
    /// Optional allow-list of Telegram user ids (`MINIAPP_ALLOWED_IDS`).
    /// Empty = any user whose `initData` verifies.
    allowed_ids: Vec<i64>,
    /// Reject `initData` older than this (seconds, `MINIAPP_AUTH_MAX_AGE`).
    auth_max_age: u64,
    /// Require the verified user to be a participant of the mirrored chat
    /// (`MINIAPP_REQUIRE_MEMBER`, default on). A cold roster cannot verify
    /// anybody, so it passes until the first roster pass lands.
    require_member: bool,
    /// Explicit dev escape hatch (`MINIAPP_ALLOW_OPEN=1`). Never on in prod.
    allow_open: bool,
}

static APP: OnceLock<Arc<MiniApp>> = OnceLock::new();
static ASSISTS: OnceLock<Arc<AssistantManager>> = OnceLock::new();
static BOT_ID: OnceLock<i64> = OnceLock::new();

/// The bot's own user id, so the roster can tag the account that streams.
pub fn set_bot_id(id: i64) {
    let _ = BOT_ID.set(id);
}

/// Accounts that actually sit in the voice chat: the bot and its assistants.
fn host_ids() -> Vec<i64> {
    let mut v: Vec<i64> = Vec::new();
    if let Some(id) = BOT_ID.get() {
        v.push(*id);
    }
    if let Some(a) = ASSISTS.get() {
        a.for_each(|x| {
            if x.user_id != 0 && !v.contains(&x.user_id) {
                v.push(x.user_id);
            }
        });
    }
    v
}

/// Hand the assistant sessions to the mini app so it can list every group
/// member (the Bot API only exposes administrators). Optional: without it the
/// roster falls back to admins + whoever talked.
pub fn install_assistants(a: Arc<AssistantManager>) {
    let _ = ASSISTS.set(a);
}

/// Ids of every participant of the mirrored group, via the assistant session.
fn participant_ids(chat: i64) -> Vec<i64> {
    let Some(assists) = ASSISTS.get() else {
        return Vec::new();
    };
    let Some(a) = assists.first() else {
        return Vec::new();
    };
    let mut ctx = a.ntg();
    let Some(mt) = ctx.mt_mut() else {
        return Vec::new();
    };
    match mt.channel_participant_ids(chat, 200) {
        Ok(ids) => ids,
        Err(e) => {
            crate::parv_info!("roster", "participants unavailable: {e}");
            Vec::new()
        }
    }
}

/// Ids of the accounts that are in the voice chat right now
/// (`phone.getGroupParticipants`). Empty when nobody is in a call — or when
/// the RPC is unavailable, in which case the caller falls back to the hosts.
fn vc_participant_ids(chat: i64) -> Vec<i64> {
    let Some(assists) = ASSISTS.get() else {
        return Vec::new();
    };
    let Some(a) = assists.for_chat(chat).or_else(|| assists.first()) else {
        return Vec::new();
    };
    let mut ctx = a.ntg();
    let Some(mt) = ctx.mt_mut() else {
        return Vec::new();
    };
    match mt.group_call_participant_ids(chat, 100) {
        Ok(ids) => ids,
        Err(e) => {
            crate::parv_info!("roster", "vc participants unavailable: {e}");
            Vec::new()
        }
    }
}

/// The process-wide instance, if `install` has run.
pub fn app() -> Option<Arc<MiniApp>> {
    APP.get().cloned()
}fn env_str(key: &str, def: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| def.to_string())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Percent-encode a URL for the `url=` / `text=` slots of `t.me/share/url`.
/// Only RFC 3986 unreserved bytes survive; everything else (including `&`,
/// `?`, `=` and spaces) is escaped so the outer link cannot be cut short.
fn pct(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Stable per-room token for `?startapp=p<chat>_<token>`. Not a secret — it
/// only lets the app tell rooms apart when the same bot serves several groups.
fn share_token(chat: i64) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    chat.hash(&mut h);
    "parvati".hash(&mut h);
    format!("{:012x}", h.finish() & 0xffff_ffff_ffff)
}

fn now_hm() -> String {
    let secs = now_secs() % 86_400;
    format!("{:02}:{:02}", secs / 3600, (secs % 3600) / 60)
}

/* ─────────────────────── Bot API bridge (tgapi) ─────────────────────── */

fn tg_addr() -> (String, u16) {
    let url = env_str("TGAPI_URL", "http://127.0.0.1:8098");
    let rest = url.trim_start_matches("http://");
    match rest.rsplit_once(':') {
        Some((h, p)) => (
            h.to_string(),
            p.trim_end_matches('/').parse().unwrap_or(8098),
        ),
        None => (rest.trim_end_matches('/').to_string(), 80),
    }
}

/// One request to the loopback Bot API bridge. Returns (status, content-type, body).
fn tg_call(method: &str, path: &str, body: Option<&[u8]>) -> Option<(u16, String, Vec<u8>)> {
    let (host, port) = tg_addr();
    let mut s = TcpStream::connect((host.as_str(), port)).ok()?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(25)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(10)));
    let key = env_str("TGAPI_KEY", "");
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\nx-tgapi-key: {key}\r\n"
    );
    if let Some(b) = body {
        req.push_str(&format!(
            "content-type: application/json\r\ncontent-length: {}\r\n",
            b.len()
        ));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).ok()?;
    if let Some(b) = body {
        s.write_all(b).ok()?;
    }
    s.flush().ok()?;

    let mut buf = Vec::new();
    s.read_to_end(&mut buf).ok()?;
    let pos = find(&buf, b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&buf[..pos]).to_string();
    let payload = buf[pos + 4..].to_vec();
    let code: u16 = head.split_whitespace().nth(1)?.parse().ok()?;
    let ctype = head
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("content-type:"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim().to_string())
        .unwrap_or_else(|| "application/json".into());
    Some((code, ctype, payload))
}

fn tg_json(path: &str) -> Option<Value> {
    let (code, _, body) = tg_call("GET", path, None)?;
    if code != 200 {
        return None;
    }
    serde_json::from_slice(&body).ok()
}

/// Turn one Bot API member object into a roster row: role, tag list and the
/// avatar URL the client should load.
fn roster_row(m: &Value, hosts: &[i64]) -> Option<Value> {
    let id = m["id"].as_str().unwrap_or("").to_string();
    if id.is_empty() {
        return None;
    }
    let status = m["status"].as_str().unwrap_or("member");
    let custom = m["custom"].as_str().unwrap_or("");
    let is_bot = m["bot"].as_bool().unwrap_or(false);
    let uid: i64 = id.parse().unwrap_or(0);
    // HOST marks the account that actually sits in the voice chat and streams
    // (the bot and its assistants), not every bot that happens to be around.
    let is_host = hosts.contains(&uid);

    // One primary tag, in precedence order: OWNER > HOST > ADMIN > MEMBER.
    // A bot that is both an admin and the account streaming in the call shows
    // HOST (that is the role that matters in a voice room), and a custom
    // title rides along as a second, differently coloured tag.
    let mut tags: Vec<Value> = Vec::new();
    if status == "creator" {
        tags.push(json!({"k":"owner","label":"OWNER"}));
    } else if is_host {
        tags.push(json!({"k":"host","label":"HOST"}));
    } else if status == "administrator" {
        tags.push(json!({"k":"admin","label":"ADMIN"}));
    }
    if !custom.is_empty() {
        tags.push(json!({"k":"title","label":custom}));
    }
    if tags.is_empty() {
        tags.push(json!({"k":"member","label":"MEMBER"}));
    }
    let role = match status {
        "creator" => "owner",
        "administrator" => "admin",
        _ => "member",
    };
    Some(json!({
        "id": id,
        "name": m["name"].as_str().unwrap_or("member"),
        "username": m["username"].as_str().unwrap_or(""),
        // Always point at the avatar proxy: the client swaps in an initials
        // tile when the account has no photo (the proxy answers 404).
        "photo": format!("/api/avatar/{id}"),
        "role": role,
        "status": status,
        "custom": custom,
        "tags": tags,
        "vc": is_host,
        "host": is_host,
        "bot": is_bot,
    }))
}

fn tg_post(path: &str, body: &Value) -> Option<(u16, Value)> {
    let raw = body.to_string();
    let (code, _, bytes) = tg_call("POST", path, Some(raw.as_bytes()))?;
    let val = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({ "error": "bad gateway" }));
    Some((code, val))
}

fn mime_of(data: &[u8]) -> &'static str {
    if data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        "image/webp"
    } else if data.len() > 3 && &data[..3] == b"\xff\xd8\xff" {
        "image/jpeg"
    } else if data.len() > 8 && &data[..8] == b"\x89PNG\r\n\x1a\n" {
        "image/png"
    } else if data.len() > 3 && &data[..3] == b"GIF" {
        "image/gif"
    } else if data.len() > 8 && &data[4..8] == b"ftyp" {
        "video/mp4"
    } else if data.len() > 4 && data[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
        // Matroska / WebM — Telegram's video stickers.
        "video/webm"
    } else if data.len() > 2 && data[..2] == [0x1F, 0x8B] {
        // gzip: an animated `.tgs` sticker (Lottie JSON inside).
        "application/x-tgsticker"
    } else {
        "application/octet-stream"
    }
}

/* ─────────────────────────── install ─────────────────────────── */

/// Install the hub and spawn the HTTP thread. Bind failures only log — the
/// bot keeps running without the mini app (same contract as `health`).
pub fn install(rooms: Rooms) {
    let port = env_str("MINIAPP_PORT", "8080").trim().parse().unwrap_or(8080);
    let chat_id = env_str("MINIAPP_CHAT_ID", "0").trim().parse().unwrap_or(0);
    let dir = PathBuf::from(env_str("MINIAPP_DIR", "miniapp"));

    let hub = Arc::new(MiniApp {
        rooms,
        chat_id,
        title: Mutex::new(env_str("MINIAPP_ROOM_TITLE", "voice chat")),
        dir,
        chats: Mutex::new(HashMap::new()),
        chat_meta: Mutex::new(HashMap::new()),
        chats_loaded: Mutex::new(std::collections::HashSet::new()),
        view_chat: Mutex::new(HashMap::new()),
        admin_cache: Mutex::new(HashMap::new()),
        last_read: Mutex::new(HashMap::new()),
        banned: Mutex::new(HashMap::new()),
        banned_loaded: Mutex::new(std::collections::HashSet::new()),
        cache_dir: PathBuf::from("cache"),
        members: Mutex::new(Vec::new()),
        seen: Mutex::new(Vec::new()),
        member_count: Mutex::new(0),
        invite: Mutex::new(String::new()),
        settings: Mutex::new(json!({
            "autoplay": true, "loop": false, "haptics": true, "notify": true
        })),
        quote: Mutex::new(String::new()),
        live: Mutex::new(false),
        session_started: Mutex::new(0),
        session_tracks: Mutex::new(0),
        last_session: Mutex::new(json!({})),
        media: Mutex::new(HashMap::new()),
        roster_at: Mutex::new(0),
        search_cache: Mutex::new(Vec::new()),
        dms: Mutex::new(Vec::new()),
        pending: Mutex::new(Value::Null),
        play_error: Mutex::new(Value::Null),
        vc_known: Mutex::new(false),
        recent: Mutex::new(Vec::new()),
        favs: Mutex::new(Vec::new()),
        sleep_at: Mutex::new(0),
        roster_busy: AtomicBool::new(false),
        token: env_str("MINIAPP_TOKEN", ""),
        bot_token: {
            let t = env_str("TOKEN", "");
            if t.trim().is_empty() { env_str("BOT_TOKEN", "") } else { t }
        },
        allowed_ids: env_str("MINIAPP_ALLOWED_IDS", "")
            .split(',')
            .filter_map(|s| s.trim().parse::<i64>().ok())
            .collect(),
        auth_max_age: env_str("MINIAPP_AUTH_MAX_AGE", "86400")
            .trim()
            .parse()
            .unwrap_or(86_400),
        require_member: env_str("MINIAPP_REQUIRE_MEMBER", "1") != "0",
        allow_open: env_str("MINIAPP_ALLOW_OPEN", "0") == "1",
    });
    let _ = APP.set(hub.clone());
    // Chats survive restarts: names, tags, stickers and text come back from
    // cache/mirror-<id>.json instead of an empty Chat tab. The registry is read
    // first so the switcher has every group the bot already knows.
    hub.load_chats();
    hub.load_dms();
    hub.load_library();

    let listener = match TcpListener::bind(("0.0.0.0", port)) {
        Ok(l) => l,
        Err(e) => {
            crate::logger::Logger::instance().log(
                crate::logger::LogLevel::Error,
                "miniapp",
                &format!("bind 0.0.0.0:{port} failed: {e}"),
            );
            return;
        }
    };
    crate::logger::Logger::instance().log(
        crate::logger::LogLevel::Info,
        "miniapp",
        &format!("mini app listening on 0.0.0.0:{port}"),
    );

    // Roster refresher: names, avatars, custom titles and the member count
    // change while the app is open, and the Bot API call is cheap enough to
    // poll every 45s in the background.
    let hub_roster = hub.clone();
    thread::spawn(move || loop {
        hub_roster.refresh_roster();
        thread::sleep(Duration::from_secs(ROSTER_TTL));
    });

    // Sleep timer: the phone is usually locked when it fires, so the stop has
    // to happen here and not in the browser.
    let hub_sleep = hub.clone();
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(5));
        let armed = *hub_sleep.sleep_at.lock().unwrap_or_else(|e| e.into_inner()) != 0;
        if !armed || hub_sleep.sleep_left() > 0 {
            continue;
        }
        hub_sleep.set_sleep(0);
        let chat = hub_sleep.active_chat();
        stop_engine(chat);
        hub_sleep.finish_session(chat, 0, "sleep timer", "sleep timer", "", false);
        hub_sleep.note_message("Parvati", "Sleep timer reached — voice chat closed. Good night 🌙");
    });

    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(s) = stream else { continue };
            let hub = hub.clone();
            thread::spawn(move || {
                let _ = handle(hub, s);
            });
        }
    });
}

/* ─────────────────────────── snapshot ─────────────────────────── */

fn track_json(t: &Track) -> Value {
    json!({
        "id": t.id,
        "title": t.title,
        "artist": t.title.split(" - ").next().unwrap_or(&t.title),
        "duration": t.duration,
        "art": t.artwork,
        "requester": t.requester,
        "source": t.source,
        "url": t.url,
        // The app shows a video player (and a VIDEO tag) for these, and the
        // voice engine streams the picture as well as the sound. A video
        // track is always the real 720p clip — there is no generated cover.
        "video": t.video,
        "kind": if t.video { "video" } else { "audio" },
    })
}

fn playback_json(r: &RoomState) -> Value {
    let q: Vec<Value> = r.queue.iter().map(track_json).collect();
    json!({
        "track": r.track.as_ref().map(track_json),
        "position": r.position,
        "paused": r.paused,
        // true only once the voice engine accepted the stream — `paused:false`
        // alone just means "not paused", it does not prove a live call.
        "playing": r.playing,
        "loop": r.loop_n,
        "shuffle": r.shuffle,
        "muted": r.muted,
        "autoplay": r.autoplay,
        "queue": q,
    })
}

/// A verified request identity. `user_id` is only set when Telegram signed the
/// `initData` — a static token or the dev escape hatch never impersonate a user.
struct Auth {
    ok: bool,
    user_id: Option<i64>,
    name: String,
    username: String,
    /// "initdata" | "token" | "open" | "not-member" | "not-allowed" | "none"
    via: &'static str,
}

impl Auth {
    fn denied() -> Auth {
        Auth {
            ok: false,
            user_id: None,
            name: String::new(),
            username: String::new(),
            via: "none",
        }
    }

    /// Verified Telegram user id (0 when the request carried no identity).
    fn id(&self) -> i64 {
        self.user_id.unwrap_or(0)
    }

    /// The identity to use for `me`. A verified user always wins over whatever
    /// the client sent, which is what stops impersonation.
    fn me(&self, fallback: String) -> String {
        match self.user_id {
            Some(id) => id.to_string(),
            None => fallback,
        }
    }

    fn display_name(&self, fallback: String) -> String {        if !self.name.is_empty() {
            self.name.clone()
        } else if !self.username.is_empty() {
            self.username.clone()
        } else {
            fallback
        }
    }
}

/// Constant-time comparison of two lowercase hex digests.
fn ct_eq_hex(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

/// How long a Telegram membership answer is trusted before asking again.
const AUTH_MEMBER_TTL: u64 = 300;

/// id → (allowed, checked_at) for verified users missing from the cached
/// roster (large groups: the roster only carries the first 200 participants).
fn auth_members() -> &'static Mutex<std::collections::HashMap<i64, (bool, u64)>> {
    static C: OnceLock<Mutex<std::collections::HashMap<i64, (bool, u64)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// `secret = HMAC_SHA256("WebAppData", bot_token)`,
/// `hash   = HMAC_SHA256(secret, sorted "k=v" lines)`.
fn tg_init_data_hash(bot_token: &str, fields: &[(String, String)]) -> Option<String> {
    let mut lines: Vec<String> = fields.iter().map(|(k, v)| format!("{k}={v}")).collect();
    lines.sort();
    let check = lines.join("\n");

    type HmacSha256 = Hmac<Sha256>;
    let mut secret = HmacSha256::new_from_slice(b"WebAppData").ok()?;
    secret.update(bot_token.as_bytes());
    let secret = secret.finalize().into_bytes();

    let mut mac = HmacSha256::new_from_slice(&secret).ok()?;
    mac.update(check.as_bytes());
    Some(to_hex(&mac.finalize().into_bytes()))
}

/// Verify a Telegram WebApp `initData` string. Returns
/// `(user_id, display_name, username)` when the signature and the timestamp
/// are valid.
fn verify_tg_init_data(
    bot_token: &str,
    max_age: u64,
    raw: &str,
) -> Option<(i64, String, String)> {
    if raw.is_empty() || bot_token.is_empty() {
        return None;
    }
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut hash = String::new();
    for part in raw.split('&') {
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        if k == "hash" {
            hash = v.to_ascii_lowercase();
        } else {
            fields.push((k.to_string(), urldecode(v)));
        }
    }
    if hash.is_empty() {
        return None;
    }

    // Telegram added `signature` (Ed25519) next to `hash`. The HMAC covers
    // every field except `hash`; some clients also drop `signature`, so accept
    // either construction.
    let mut valid = tg_init_data_hash(bot_token, &fields)
        .map(|c| ct_eq_hex(&c, &hash))
        .unwrap_or(false);
    if !valid {
        let no_sig: Vec<(String, String)> = fields
            .iter()
            .filter(|(k, _)| k != "signature")
            .cloned()
            .collect();
        valid = tg_init_data_hash(bot_token, &no_sig)
            .map(|c| ct_eq_hex(&c, &hash))
            .unwrap_or(false);
    }
    if !valid {
        return None;
    }

    // Freshness: a leaked initData must not be replayable forever.
    let auth_date = fields
        .iter()
        .find(|(k, _)| k == "auth_date")
        .and_then(|(_, v)| v.parse::<u64>().ok())?;
    let now = now_secs();
    if auth_date > now + 300 || now.saturating_sub(auth_date) > max_age {
        return None;
    }

    let user: Value = fields
        .iter()
        .find(|(k, _)| k == "user")
        .and_then(|(_, v)| serde_json::from_str(v).ok())?;
    let id = user["id"].as_i64()?;
    let first = user["first_name"].as_str().unwrap_or("");
    let last = user["last_name"].as_str().unwrap_or("");
    let name = format!("{first} {last}").trim().to_string();
    let username = user["username"].as_str().unwrap_or("").to_string();
    Some((id, name, username))
}

impl MiniApp {
    /// Authenticate a request. The only identity that counts is the one
    /// Telegram signed in `initData`; anything the client sends as `me` is
    /// ignored once a real user is known.
    fn authorize(&self, req: &Request) -> Auth {
        // initData arrives three ways: a header (fetch), the query string
        // (EventSource / <video> cannot set headers) or the cookie the page
        // sets once, which covers <img>/<video>/SSE automatically.
        let raw = req
            .init_data
            .clone()
            .or_else(|| qs(&req.query, "init"))
            .or_else(|| qs(&req.query, "initData"))
            .or_else(|| req.cookies.get("parvati_auth").cloned())
            .unwrap_or_default();

        if !raw.is_empty() {
            if let Some((id, name, username)) = self.verify_init_data(&raw) {
                if !self.allowed_ids.is_empty() && !self.allowed_ids.contains(&id) {
                    return Auth {
                        ok: false,
                        user_id: Some(id),
                        name,
                        username,
                        via: "not-allowed",
                    };
                }
                if !self.member_ok(id) {
                    return Auth {
                        ok: false,
                        user_id: Some(id),
                        name,
                        username,
                        via: "not-member",
                    };
                }
                return Auth {
                    ok: true,
                    user_id: Some(id),
                    name,
                    username,
                    via: "initdata",
                };
            }
        }

        // Trusted integration: a static token grants control, never a user id.
        if !self.token.is_empty() {
            let ok = qs(&req.query, "token").as_deref() == Some(self.token.as_str())
                || req.header_token.as_deref() == Some(self.token.as_str());
            if ok {
                return Auth {
                    ok: true,
                    user_id: None,
                    name: String::new(),
                    username: String::new(),
                    via: "token",
                };
            }
        }

        // Explicit dev escape hatch (MINIAPP_ALLOW_OPEN=1).
        if self.allow_open {
            return Auth {
                ok: true,
                user_id: None,
                name: String::new(),
                username: String::new(),
                via: "open",
            };
        }

        Auth::denied()
    }

    /// Validate Telegram WebApp `initData`:
    /// `secret = HMAC_SHA256("WebAppData", bot_token)`,
    /// `hash   = HMAC_SHA256(secret, sorted "k=v" lines)`.
    fn verify_init_data(&self, raw: &str) -> Option<(i64, String, String)> {
        verify_tg_init_data(&self.bot_token, self.auth_max_age, raw)
    }

    /// A verified Telegram user still has to belong to the mirrored room.
    fn member_ok(&self, id: i64) -> bool {
        if !self.require_member || self.allowed_ids.contains(&id) {
            return true;
        }
        if std::env::var("OWNER_ID")
            .ok()
            .and_then(|v| v.trim().parse::<i64>().ok())
            == Some(id)
        {
            return true;
        }
        let sid = id.to_string();
        let members = self.members.lock().unwrap_or_else(|e| e.into_inner());
        // A cold roster cannot prove anything; let the request through and let
        // the first roster pass decide.
        if members.is_empty() {
            return true;
        }
        if members.iter().any(|m| m["id"].as_str() == Some(sid.as_str())) {
            return true;
        }
        drop(members);

        // Not in the cached roster: the roster only holds the first 200
        // participants, so ask Telegram once and remember the answer.
        let now = now_secs();
        {
            let cache = auth_members().lock().unwrap_or_else(|e| e.into_inner());
            if let Some((ok, at)) = cache.get(&id) {
                if now.saturating_sub(*at) < AUTH_MEMBER_TTL {
                    return *ok;
                }
            }
        }
        let status = tg_json(&format!(
            "/member?chat_id={}&user_id={id}",
            self.active_chat()
        ))
        .and_then(|v| v["status"].as_str().map(|s| s.to_string()));
        let ok = match status.as_deref() {
            Some("creator") | Some("administrator") | Some("member") | Some("restricted") => true,
            Some("left") | Some("kicked") => false,
            // Bridge offline / unexpected answer: never lock a real user out.
            _ => true,
        };
        auth_members()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, (ok, now));
        ok
    }

    /// Chat the app mirrors: the configured one when it already has a room,
    /// otherwise the most recently updated live room — so the mini app follows
    /// whatever voice chat the bot is actually playing in instead of a fixed id.
    fn active_chat(&self) -> i64 {
        if self.rooms.room_for(self.chat_id).is_some() {
            return self.chat_id;
        }
        let mut best: Option<(i64, i64)> = None;
        for (id, room) in self.rooms.all_rooms() {
            let g = room.lock().unwrap_or_else(|e| e.into_inner());
            let stamp = g.updated_at.max(g.position);
            if best.map(|(_, s)| stamp > s).unwrap_or(true) {
                best = Some((id, stamp));
            }
        }
        best.map(|(id, _)| id).unwrap_or(self.chat_id)
    }

    fn room(&self) -> Option<Arc<Mutex<RoomState>>> {
        self.rooms.room_for(self.active_chat())
    }

    fn snapshot(&self, with_chat: bool, viewer: Option<&Auth>) -> Value {
        let room = self.room();
        let (mut playback, room_active, cur_track) = match room.as_ref() {
            Some(r) => {
                let g = r.lock().unwrap_or_else(|e| e.into_inner());
                (playback_json(&g), g.active(), g.track.clone())
            }
            None => (
                json!({
                    "track": null, "position": 0, "paused": true, "playing": false,
                    "loop": 0, "shuffle": false, "muted": false, "autoplay": false, "queue": []
                }),
                false,
                None,
            ),
        };

        // History is written the moment a new track shows up, not by every
        // caller that can start playback (mini app, /play, autoplay, skip).
        if let Some(t) = &cur_track {
            let head = self
                .recent
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .first()
                .and_then(|x| x["id"].as_str().map(|s| s.to_string()));
            if head.as_deref() != Some(t.id.as_str()) {
                self.record_recent(t);
            }
        }
        if let Some(o) = playback.as_object_mut() {
            o.insert(
                "liked".to_string(),
                json!(cur_track
                    .as_ref()
                    .map(|t| self.is_liked(&t.id))
                    .unwrap_or(false)),
            );
        }

        // A room that is already streaming keeps the session alive even when
        // the flag has not been flipped yet (the play thread sets it just
        // before creating the room). A live flag with no room left means the
        // queue ran dry without /end, so close the session quietly.
        let mut live = self.is_live() || room_active;
        if live && !self.is_live() {
            self.mark_live();
        }
        if self.is_live() && room.is_none() {
            let started = *self.session_started.lock().unwrap_or_else(|e| e.into_inner());
            if started > 0 && now_secs().saturating_sub(started) > 5 {
                self.finish_session(self.active_chat(), 0, "", "finished", "", false);
                live = false;
            }
        }

        let members = self.members.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let settings = self.settings.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let quote = self.quote.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let title = self.title.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let count = *self.member_count.lock().unwrap_or_else(|e| e.into_inner());
        let invite = self.invite.lock().unwrap_or_else(|e| e.into_inner()).clone();
        // `me` is the *viewer*, not the room owner: the profile header must
        // show the person who opened the app. When the roster is cold the row
        // is built from the signed identity, so it is never the wrong person.
        let vid = viewer.and_then(|a| a.user_id);
        let me = vid
            .and_then(|id| {
                let sid = id.to_string();
                members
                    .iter()
                    .find(|m| m["id"].as_str() == Some(sid.as_str()))
                    .cloned()
            })
            .or_else(|| {
                viewer.filter(|a| a.user_id.is_some()).map(|a| {
                    let id = a.user_id.unwrap_or(0);
                    json!({
                        "id": id.to_string(),
                        "name": if a.name.is_empty() { "you".to_string() } else { a.name.clone() },
                        "username": a.username.clone(),
                        "photo": format!("/api/avatar/{id}"),
                        "role": "member",
                        "status": "member",
                        "custom": "",
                        "tags": [{"k":"member","label":"MEMBER"}],
                        "vc": false,
                        "host": false,
                        "bot": false,
                    })
                })
            })
            .or_else(|| members.iter().find(|m| m["host"] == true).cloned())
            .or_else(|| members.iter().find(|m| m["role"] == "owner").cloned())
            .unwrap_or(Value::Null);
        let session = self.session_json(live);

        // Share / invite links always carry the bot's @username, so opening
        // them lands in this bot's mini app (or adds it to another group)
        // instead of a bare t.me/+ invite that only works for the group.
        let bot = crate::buttons::bot_username();
        let chat_id = self.active_chat();
        let app_link = if bot.is_empty() {
            String::new()
        } else {
            format!("https://t.me/{bot}?startapp=p{chat_id}_{}", share_token(chat_id))
        };
        let share = if app_link.is_empty() {
            String::new()
        } else {
            format!(
                "https://t.me/share/url?url={}&text={}",
                pct(&app_link),
                pct("Join my music room")
            )
        };
        let invite_share = if invite.is_empty() {
            String::new()
        } else {
            format!(
                "https://t.me/share/url?url={}&text={}",
                pct(&invite),
                pct(&format!("Join {title} on Telegram"))
            )
        };
        let add_to_group = if bot.is_empty() {
            String::new()
        } else {
            format!("https://t.me/{bot}?startgroup=true&admin=invite_users")
        };

        let mut out = json!({
            "room": {
                "id": chat_id,
                "title": title,
                "members": count.max(members.len()),
                "shown": members.len(),
                // Nobody is in the voice chat once the session is over.
                "online": if live {
                    members.iter().filter(|m| m["vc"] == true).count().max(1)
                } else {
                    0
                },
                "invite": invite,
                "invite_share": invite_share,
                "bot": bot,
                "app_link": app_link,
                "share": share,
                "add_to_group": add_to_group,
                "live": live,
                "vc_known": *self.vc_known.lock().unwrap_or_else(|e| e.into_inner()),
                // Explicit list of who is in the call, so the app can scope the
                // chat without re-deriving it from the member rows.
                "vc_ids": members
                    .iter()
                    .filter(|m| m["vc"] == true)
                    .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
                    .collect::<Vec<String>>(),
            },
            "session": session,
            "me": me,
            "quote": quote,
            "playback": playback,
            "members": members,
            "settings": settings,
            "recent": self.recent.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            "favorites": self.favs.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            "sleep": self.sleep_left(),
        });
        // Optimistic playback: a picked-but-not-yet-downloaded track and the
        // last failure both ride along with the playback block so the UI can
        // paint "starting…" and explain a failed tap.
        out["playback"]["pending"] = self.pending.lock().unwrap_or_else(|e| e.into_inner()).clone();
        out["playback"]["error"] = self.play_error.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if with_chat {
            let chat = self.chat_for(viewer);
            out["messages"] = json!(self.chat_msgs(chat));
            // Header + switcher for the chat the viewer actually picked. The
            // room block above stays on the *playing* room, so the player tab
            // never points at a group that has no voice chat.
            out["chat"] = self.chat_head(chat, viewer);
            out["chats"] = json!(self.chat_list(viewer));
        }
        out
    }

    /// Header block for one chat: id, display title, member count, live flag
    /// and whether this viewer may clear its mirrored history.
    fn chat_head(&self, chat: i64, viewer: Option<&Auth>) -> Value {
        let meta = self
            .chat_meta
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&chat)
            .cloned()
            .unwrap_or(Value::Null);
        let title = meta["title"]
            .as_str()
            .filter(|t| !t.is_empty())
            .map(|t| t.to_string())
            .unwrap_or_else(|| {
                if chat == self.chat_id {
                    self.title.lock().unwrap_or_else(|e| e.into_inner()).clone()
                } else {
                    format!("Chat {chat}")
                }
            });
        json!({
            "id": chat.to_string(),
            "title": title,
            "members": meta["members"].as_u64().unwrap_or(0),
            "live": self.rooms.room_for(chat).is_some(),
            "can_clear": viewer
                .map(|a| self.is_owner(a) || self.is_admin_of(a, chat, false))
                .unwrap_or(false),
        })
    }

    /* ── session lifecycle ── */

    fn is_live(&self) -> bool {
        *self.live.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Flip the session on and stamp its start time (no-op while live, so a
    /// second track does not reset the clock).
    fn mark_live(&self) {
        let mut live = self.live.lock().unwrap_or_else(|e| e.into_inner());
        if !*live {
            *live = true;
            *self.session_started.lock().unwrap_or_else(|e| e.into_inner()) = now_secs();
            // The streaming accounts are in the call from this instant, not
            // from the next roster poll (45s later).
            drop(live);
            let mut members = self.members.lock().unwrap_or_else(|e| e.into_inner());
            for m in members.iter_mut() {
                let host = m["host"].as_bool().unwrap_or(false);
                if let Some(o) = m.as_object_mut() {
                    o.insert("vc".to_string(), json!(host));
                }
            }
        }
    }

    /// Public shape of the session block: live now, plus the summary of the
    /// last one that ended (who, when, how long, how many tracks).
    fn session_json(&self, live: bool) -> Value {
        let started = *self.session_started.lock().unwrap_or_else(|e| e.into_inner());
        let playing = *self.session_tracks.lock().unwrap_or_else(|e| e.into_inner());
        let last = self.last_session.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let now = now_secs();
        json!({
            "live": live,
            "started_at": if live { started } else { last["started_at"].as_u64().unwrap_or(0) },
            "duration": if live {
                now.saturating_sub(started)
            } else {
                last["duration"].as_u64().unwrap_or(0)
            },
            "ended_at": last["ended_at"].as_u64().unwrap_or(0),
            "ended_hm": last["ended_hm"].as_str().unwrap_or(""),
            "ended_by": last["ended_by"].as_str().unwrap_or(""),
            "ended_by_id": last["ended_by_id"].as_str().unwrap_or(""),
            "reason": last["reason"].as_str().unwrap_or(""),
            // While live the counter is the truth; once ended it comes from
            // the summary that finish_session stored.
            "tracks": if live { playing as u64 } else { last["tracks"].as_u64().unwrap_or(0) },
            "last_track": last["last_track"].as_str().unwrap_or(""),
            "cleared": last["cleared"].as_bool().unwrap_or(false),
        })
    }

    /// Close the session: everyone leaves the voice chat, the mirrored chat is
    /// cleared (on an explicit end) and the summary is kept for the UI.
    fn finish_session(
        &self,
        chat_id: i64,
        by_id: i64,
        by_name: &str,
        reason: &str,
        last_track: &str,
        clear_chat: bool,
    ) {
        let started = *self.session_started.lock().unwrap_or_else(|e| e.into_inner());
        let tracks = *self.session_tracks.lock().unwrap_or_else(|e| e.into_inner());
        let now = now_secs();
        let duration = if started > 0 && now > started { now - started } else { 0 };
        *self.live.lock().unwrap_or_else(|e| e.into_inner()) = false;
        *self.session_started.lock().unwrap_or_else(|e| e.into_inner()) = 0;
        *self.session_tracks.lock().unwrap_or_else(|e| e.into_inner()) = 0;
        *self.pending.lock().unwrap_or_else(|e| e.into_inner()) = Value::Null;
        if clear_chat {
            self.clear_chat(chat_id);
        }
        {
            let mut members = self.members.lock().unwrap_or_else(|e| e.into_inner());
            for m in members.iter_mut() {
                if let Some(o) = m.as_object_mut() {
                    o.insert("vc".to_string(), json!(false));
                }
            }
        }
        // A second end (queue ran out, then someone pressed End in the app)
        // must not overwrite a real summary with a 0s / 0-track one.
        let already = self.last_session.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if started > 0 || already["duration"].as_u64().unwrap_or(0) == 0 {
            *self.last_session.lock().unwrap_or_else(|e| e.into_inner()) = json!({
                "chat_id": chat_id,
                "started_at": started,
                "ended_at": now,
                "ended_hm": now_hm(),
                "ended_by": by_name,
                "ended_by_id": by_id.to_string(),
                "duration": duration,
                "tracks": tracks,
                "last_track": last_track,
                "reason": reason,
                "cleared": clear_chat,
            });
        }
        crate::parv_info!(
            "session",
            "ended chat={chat_id} by={by_id} reason={reason} dur={duration}s tracks={tracks}"
        );
    }

    /// Push the current state to one SSE client. Frames identical to the last
    /// one sent are skipped: the app re-renders the whole screen per frame, so
    /// repeating the same payload every tick was the main source of jank.
    /// `first` forces a full frame for a newly connected client.
    fn broadcast(
        &self,
        res: &mut TcpStream,
        first: bool,
        viewer: Option<&Auth>,
        cur: &mut Cursor,
    ) -> std::io::Result<()> {
        let mut state_val = self.snapshot(false, viewer);
        // The switcher rides on the state stream too. Without it the chat list
        // only refreshed on a manual /api/state, so a group the bot had just
        // joined stayed invisible until the app was reopened.
        if let Some(o) = state_val.as_object_mut() {
            let chat = self.chat_for(viewer);
            o.insert("chat".to_string(), self.chat_head(chat, viewer));
            o.insert("chats".to_string(), json!(self.chat_list(viewer)));
        }
        let state = json!({ "type": "state", "data": state_val.clone() }).to_string();
        // The playhead moves every second, which used to make *every* frame a
        // "change" and re-render the whole app. Compare without `position`
        // (the client interpolates it locally) and only re-send the playhead
        // every few seconds as a drift correction.
        if let Some(p) = state_val.get_mut("playback").and_then(|v| v.as_object_mut()) {
            p.remove("position");
        }
        let sig = state_val.to_string();
        let now = now_secs();
        let drift = now.saturating_sub(cur.at) >= 5;
        if first || cur.state != sig || drift {
            cur.state = sig;
            cur.at = now;
            write!(res, "data: {state}\n\n")?;
        }

        let chat = self.chat_for(viewer);
        let messages = json!({
            "type": "messages",
            "chat": chat.to_string(),
            "data": self.chat_msgs(chat),
        })
        .to_string();
        if first || cur.messages != messages {
            cur.messages = messages.clone();
            write!(res, "data: {messages}\n\n")?;
        }
        res.flush()
    }

    /* ── per-chat message store ── */

    /// Chat this viewer is looking at. Falls back to the playing room when the
    /// viewer never picked one (or picked a chat that has since vanished).
    fn chat_for(&self, viewer: Option<&Auth>) -> i64 {
        if let Some(a) = viewer {
            let picked = self
                .view_chat
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&a.id())
                .copied();
            if let Some(id) = picked {
                if self.chat_known(id) {
                    return id;
                }
            }
        }
        self.active_chat()
    }

    fn chat_known(&self, id: i64) -> bool {
        id == self.chat_id
            || self.rooms.room_for(id).is_some()
            || self
                .chat_meta
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(&id)
    }

    /// Read `cache/mirror-<id>.json` once, on first touch.
    fn ensure_chat(&self, chat: i64) {
        if !self
            .chats_loaded
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(chat)
        {
            return;
        }
        self.load_chat(chat, false);
    }

    fn chat_msgs(&self, chat: i64) -> Vec<Value> {
        self.ensure_chat(chat);
        self.chats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&chat)
            .map(|q| q.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn chat_msgs_for(&self, chat: i64, mid: i64) -> Option<Value> {
        if mid == 0 {
            return None;
        }
        self.ensure_chat(chat);
        self.chats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&chat)
            .and_then(|q| q.iter().find(|m| m["mid"].as_i64() == Some(mid)).cloned())
    }

    fn push_message(&self, chat: i64, msg: Value) {
        self.ensure_chat(chat);
        {
            let mut map = self.chats.lock().unwrap_or_else(|e| e.into_inner());
            let q = map.entry(chat).or_default();
            if let Some(mid) = msg.get("mid").and_then(|v| v.as_i64()) {
                if mid != 0 {
                    if let Some(slot) = q.iter_mut().find(|m| m["mid"].as_i64() == Some(mid)) {
                        *slot = msg;
                        drop(map);
                        self.save_chat(chat);
                        return;
                    }
                }
            }
            q.push_back(msg);
            while q.len() > CHAT_CAP {
                q.pop_front();
            }
        }
        self.save_chat(chat);
    }

    /// Remove one message from a chat. Returns true when something was removed.
    /// Replies that pointed at it are unlinked so the UI never renders a
    /// "reply to a message that no longer exists" stub.
    fn drop_message(&self, chat: i64, mid: i64) -> bool {
        if mid == 0 {
            return false;
        }
        self.ensure_chat(chat);
        let removed = {
            let mut map = self.chats.lock().unwrap_or_else(|e| e.into_inner());
            let Some(q) = map.get_mut(&chat) else {
                return false;
            };
            let before = q.len();
            q.retain(|m| m["mid"].as_i64() != Some(mid));
            let removed = q.len() != before;
            if removed {
                for m in q.iter_mut() {
                    if m["reply"]["mid"].as_i64() == Some(mid) {
                        if let Some(o) = m.as_object_mut() {
                            o.remove("reply");
                        }
                    }
                }
            }
            removed
        };
        if removed {
            self.save_chat(chat);
        }
        removed
    }

    fn clear_chat(&self, chat: i64) {
        self.ensure_chat(chat);
        self.chats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&chat);
        self.save_chat(chat);
    }

    /// Delete a chat from the app for good: registry row, mirrored messages,
    /// the on-disk mirror, and every viewer's "last read" stamp for it.
    /// Telegram is untouched — the group and its history stay where they are.
    /// The bot's home chat is never removed, only cleared, so the switcher can
    /// never lose the room the app was configured for.
    fn forget_chat(&self, chat: i64) {
        self.clear_chat(chat);
        let _ = std::fs::remove_file(self.chat_file(chat));
        self.admin_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(_, c), _| *c != chat);
        if chat == self.chat_id {
            return;
        }
        self.chat_meta
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&chat);
        self.view_chat
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|_, id| *id != chat);
        let suffix = format!(":{chat}");
        self.last_read
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|k, _| !k.ends_with(&suffix));
        self.chats_loaded
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&chat);
        self.save_chats();
    }

    /// Re-read one chat's mirror from disk and refresh its group metadata and
    /// roster in the background. Used by "Refresh chat" in the "…" sheet.
    fn refresh_chat(&self, chat: i64) {
        // Mark loaded again up front: the replace below is the authoritative
        // copy, so the next `ensure_chat` must not re-read the file and undo
        // any message that arrived between the refresh and the next frame.
        self.chats_loaded
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(chat);
        self.load_chat(chat, true);
        if let Some(hub) = crate::webapp::app() {
            thread::spawn(move || {
                hub.resolve_chat_meta(chat);
                hub.refresh_roster();
            });
        }
    }

    /* ── bans made through this app ── */

    fn banned_file(&self, chat: i64) -> PathBuf {
        self.cache_dir.join(format!("banned-{chat}.json"))
    }

    fn ensure_banned(&self, chat: i64) {
        let mut loaded = self.banned_loaded.lock().unwrap_or_else(|e| e.into_inner());
        if !loaded.insert(chat) {
            return;
        }
        drop(loaded);
        let Ok(raw) = std::fs::read(self.banned_file(chat)) else {
            return;
        };
        let Ok(list) = serde_json::from_slice::<Vec<Value>>(&raw) else {
            return;
        };
        self.banned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(chat, list);
    }

    fn banned_list(&self, chat: i64) -> Vec<Value> {
        self.ensure_banned(chat);
        self.banned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&chat)
            .cloned()
            .unwrap_or_default()
    }

    fn save_banned(&self, chat: i64) {
        let list = self
            .banned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&chat)
            .cloned()
            .unwrap_or_default();
        let path = self.banned_file(chat);
        if list.is_empty() {
            let _ = std::fs::remove_file(&path);
            return;
        }
        let Ok(body) = serde_json::to_vec(&list) else {
            return;
        };
        let _ = std::fs::create_dir_all(&self.cache_dir);
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, body).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    fn ban_add(&self, chat: i64, id: &str, by: &str) {
        if id.is_empty() {
            return;
        }
        self.ensure_banned(chat);
        let known = self.member_by_id(id);
        let row = json!({
            "id": id,
            "name": known.as_ref().and_then(|m| m["name"].as_str()).unwrap_or(id),
            "username": known.as_ref().and_then(|m| m["username"].as_str()).unwrap_or(""),
            "by": by,
            "at": now_secs(),
            "hm": now_hm(),
        });
        {
            let mut map = self.banned.lock().unwrap_or_else(|e| e.into_inner());
            let q = map.entry(chat).or_default();
            q.retain(|b| b["id"].as_str() != Some(id));
            q.push(row);
            if q.len() > 200 {
                q.remove(0);
            }
        }
        self.save_banned(chat);
    }

    fn ban_remove(&self, chat: i64, id: &str) -> bool {
        self.ensure_banned(chat);
        let removed = {
            let mut map = self.banned.lock().unwrap_or_else(|e| e.into_inner());
            let Some(q) = map.get_mut(&chat) else {
                return false;
            };
            let before = q.len();
            q.retain(|b| b["id"].as_str() != Some(id));
            q.len() != before
        };
        if removed {
            self.save_banned(chat);
        }
        removed
    }

    /* ── chat persistence ── */

    /// Where one mirrored chat is kept between restarts.
    fn chat_file(&self, chat: i64) -> PathBuf {
        self.cache_dir.join(format!("mirror-{chat}.json"))
    }

    /// Write one chat to disk so a restart does not wipe it. Atomic
    /// (tmp + rename) — a half-written file would break the next boot.
    fn save_chat(&self, chat: i64) {
        let list: Vec<Value> = {
            let map = self.chats.lock().unwrap_or_else(|e| e.into_inner());
            map.get(&chat)
                .map(|q| q.iter().cloned().collect())
                .unwrap_or_default()
        };
        let path = self.chat_file(chat);
        if list.is_empty() {
            let _ = std::fs::remove_file(&path);
            return;
        }
        let Ok(body) = serde_json::to_vec(&list) else {
            return;
        };
        let _ = std::fs::create_dir_all(&self.cache_dir);
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, body).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    /// Restore one mirrored chat (names, tags, stickers, text) from disk.
    ///
    /// `replace` makes the reload idempotent: "Refresh chat" calls this a
    /// second time on a chat that is already in memory, so appending would
    /// duplicate every bubble. When it is set and the mirror is gone or
    /// unreadable, the in-memory copy is dropped too — disk is authoritative.
    /// The first-touch path keeps the existing copy on a read failure so a
    /// transient I/O error can never wipe a live chat.
    fn load_chat(&self, chat: i64, replace: bool) {
        let parsed = std::fs::read(self.chat_file(chat))
            .ok()
            .and_then(|raw| serde_json::from_slice::<Vec<Value>>(&raw).ok());
        let Some(list) = parsed else {
            if replace {
                self.chats
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&chat);
            }
            return;
        };
        let mut q = VecDeque::new();
        for m in list.into_iter().rev().take(CHAT_CAP).rev() {
            // Older builds mirrored Telegram service messages; they render as
            // empty bubbles, so drop them while restoring the cache.
            let kind = m["kind"].as_str().unwrap_or("text");
            let text = m["text"].as_str().unwrap_or("");
            if kind == "text" && text.trim().is_empty() {
                continue;
            }
            q.push_back(m);
        }
        self.chats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(chat, q);
    }

    /* ── chat registry ── */

    /// Record a chat so it shows up in the switcher. `title`/`members` are
    /// only overwritten when known, so a message arriving before the roster
    /// pass cannot blank out a real name.
    fn touch_chat(&self, chat: i64, title: Option<&str>, members: Option<usize>) {
        if chat == 0 {
            return;
        }
        let mut changed;
        {
            let mut meta = self.chat_meta.lock().unwrap_or_else(|e| e.into_inner());
            changed = !meta.contains_key(&chat);
            let row = meta
                .entry(chat)
                .or_insert_with(|| json!({ "id": chat.to_string(), "title": "", "kind": "group" }));
            if let Some(t) = title {
                let t = t.trim();
                if !t.is_empty() && row["title"].as_str() != Some(t) {
                    row["title"] = json!(t);
                    changed = true;
                }
            }
            if let Some(n) = members {
                if n > 0 && row["members"].as_u64() != Some(n as u64) {
                    row["members"] = json!(n);
                    changed = true;
                }
            }
            if row["title"].as_str().unwrap_or("").is_empty() {
                row["title"] = json!(format!("Chat {chat}"));
                changed = true;
            }
        }
        // Every mirrored message used to rewrite chats.json. Only a new group
        // or genuinely new metadata is worth a disk write.
        if changed {
            self.save_chats();
        }
    }

    fn chats_file(&self) -> PathBuf {
        self.cache_dir.join("chats.json")
    }

    /// Fill in a chat's real title and member count from Telegram. Runs off the
    /// request thread: the bridge can be slow, and a nameless chat is not worth
    /// blocking a switch on.
    fn resolve_chat_meta(&self, chat: i64) {
        if chat == 0 {
            return;
        }
        let Some(info) = tg_json(&format!("/chat?chat_id={chat}")) else {
            return;
        };
        let title = info["title"].as_str().unwrap_or("");
        let members = info["members"].as_u64().map(|n| n as usize);
        self.touch_chat(chat, Some(title), members);
    }

    fn save_chats(&self) {
        let list: Vec<Value> = {
            let meta = self.chat_meta.lock().unwrap_or_else(|e| e.into_inner());
            let mut v: Vec<Value> = meta.values().cloned().collect();
            // Newest activity first; the registry is capped so a bot sitting in
            // dozens of groups cannot grow this file without bound.
            v.sort_by(|a, b| {
                b["last_date"]
                    .as_i64()
                    .unwrap_or(0)
                    .cmp(&a["last_date"].as_i64().unwrap_or(0))
            });
            v.truncate(40);
            v
        };
        let Ok(body) = serde_json::to_vec(&list) else {
            return;
        };
        let _ = std::fs::create_dir_all(&self.cache_dir);
        let path = self.chats_file();
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, body).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    fn load_chats(&self) {
        let Ok(raw) = std::fs::read(self.chats_file()) else {
            return;
        };
        let Ok(list) = serde_json::from_slice::<Vec<Value>>(&raw) else {
            return;
        };
        let mut meta = self.chat_meta.lock().unwrap_or_else(|e| e.into_inner());
        for row in list {
            if let Some(id) = row["id"].as_str().and_then(|s| s.parse::<i64>().ok()) {
                meta.insert(id, row);
            }
        }
    }

    /// Bump the "last message" preview of a chat (drives list ordering).
    fn note_chat_activity(&self, chat: i64, text: &str, ts: &str, date: i64) {
        {
            let mut meta = self.chat_meta.lock().unwrap_or_else(|e| e.into_inner());
            let row = meta
                .entry(chat)
                .or_insert_with(|| json!({ "id": chat.to_string(), "title": format!("Chat {chat}"), "kind": "group" }));
            row["last_text"] = json!(text.chars().take(80).collect::<String>());
            row["last_ts"] = json!(ts);
            row["last_date"] = json!(date);
        }
        self.save_chats();
    }

    /// Unread count for a viewer: messages newer than their last read stamp.
    fn unread_for(&self, chat: i64, viewer: Option<&Auth>) -> usize {
        let Some(a) = viewer else { return 0 };
        let key = format!("{}:{}", a.id(), chat);
        let seen = self
            .last_read
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .copied()
            .unwrap_or(0);
        self.chat_msgs(chat)
            .iter()
            .filter(|m| m["date"].as_i64().unwrap_or(0) > seen)
            .filter(|m| m["from"].as_str() != Some(a.id().to_string().as_str()))
            .count()
    }

    fn mark_read(&self, chat: i64, viewer: Option<&Auth>) {
        let Some(a) = viewer else { return };
        let newest = self
            .chat_msgs(chat)
            .iter()
            .map(|m| m["date"].as_i64().unwrap_or(0))
            .max()
            .unwrap_or(0);
        let key = format!("{}:{}", a.id(), chat);
        let mut map = self.last_read.lock().unwrap_or_else(|e| e.into_inner());
        let slot = map.entry(key).or_insert(0);
        if newest > *slot {
            *slot = newest;
        }
    }

    /// The chat list the switcher renders. Permission flags come from the
    /// short-lived cache, so a state frame never waits on the Telegram bridge;
    /// `/api/chats` warms that cache before the sheet opens.
    fn chat_list(&self, viewer: Option<&Auth>) -> Vec<Value> {
        let active = self.chat_for(viewer);
        // Snapshot the registry first: `is_admin_of` takes the member roster
        // lock, and holding the registry lock across it would invert the lock
        // order used by `refresh_roster` (members → registry).
        let mut src: Vec<(i64, Value)> = {
            let meta = self.chat_meta.lock().unwrap_or_else(|e| e.into_inner());
            meta.iter().map(|(id, row)| (*id, row.clone())).collect()
        };
        // A group can have a live room before it has a single message (the bot
        // joins the call first). It still deserves its own chat row.
        for (id, _) in self.rooms.all_rooms() {
            if !src.iter().any(|(x, _)| *x == id) {
                src.push((
                    id,
                    json!({ "id": id.to_string(), "title": "", "kind": "group" }),
                ));
            }
        }
        let mut rows: Vec<Value> = Vec::new();
        for (id, mut r) in src {
            r["id"] = json!(id.to_string());
            if r["title"].as_str().unwrap_or("").trim().is_empty() {
                r["title"] = json!(if id == self.chat_id {
                    self.title.lock().unwrap_or_else(|e| e.into_inner()).clone()
                } else {
                    format!("Chat {id}")
                });
            }
            r["live"] = json!(self.rooms.room_for(id).is_some());
            r["active"] = json!(id == active);
            r["unread"] = json!(self.unread_for(id, viewer));
            r["can_delete"] = json!(match viewer {
                Some(a) => self.is_owner(a) || self.is_admin_of(a, id, false),
                None => false,
            });
            rows.push(r);
        }
        // The configured chat always exists, even before its first message.
        if !rows.iter().any(|r| r["id"].as_str() == Some(active.to_string().as_str())) {
            let title = self.title.lock().unwrap_or_else(|e| e.into_inner()).clone();
            rows.push(json!({
                "id": active.to_string(),
                "title": title,
                "kind": "group",
                "last_text": "",
                "last_ts": "",
                "last_date": 0,
                "live": self.rooms.room_for(active).is_some(),
                "active": true,
                "unread": 0,
                "can_delete": false,
            }));
        }
        rows.sort_by(|a, b| {
            // The chat you are in stays on top, then newest activity first.
            b["active"]
                .as_bool()
                .unwrap_or(false)
                .cmp(&a["active"].as_bool().unwrap_or(false))
                .then(
                    b["last_date"]
                        .as_i64()
                        .unwrap_or(0)
                        .cmp(&a["last_date"].as_i64().unwrap_or(0)),
                )
        });
        rows
    }

    /// The list for the switcher: refresh permission flags first, then build
    /// the rows. State frames call `chat_list` directly and stay network-free.
    fn chat_list_warmed(&self, viewer: Option<&Auth>) -> Vec<Value> {
        if let Some(a) = viewer {
            let mut ids: Vec<i64> = self
                .chat_meta
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .keys()
                .copied()
                .collect();
            for (id, _) in self.rooms.all_rooms() {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            self.warm_admin_cache(a, &ids);
        }
        self.chat_list(viewer)
    }

    /* ── private chat ── */

    fn dm_file(&self) -> PathBuf {
        PathBuf::from("cache").join(format!("dm-{}.json", self.active_chat()))
    }

    fn save_dms(&self) {
        let list: Vec<Value> = self
            .dms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if list.is_empty() {
            let _ = std::fs::remove_file(self.dm_file());
            return;
        }
        let Ok(body) = serde_json::to_vec(&list) else {
            return;
        };
        let _ = std::fs::create_dir_all("cache");
        let tmp = self.dm_file().with_extension("json.tmp");
        if std::fs::write(&tmp, body).is_ok() {
            let _ = std::fs::rename(&tmp, self.dm_file());
        }
    }

    fn load_dms(&self) {
        let Ok(raw) = std::fs::read(self.dm_file()) else {
            return;
        };
        if let Ok(list) = serde_json::from_slice::<Vec<Value>>(&raw) {
            *self.dms.lock().unwrap_or_else(|e| e.into_inner()) = list;
        }
    }

    /* ── recently played / liked ── */

    fn library_file(&self) -> PathBuf {
        PathBuf::from("cache").join(format!("library-{}.json", self.active_chat()))
    }

    /// `{"recent": [...], "favs": [...]}` — one file, one atomic write.
    fn load_library(&self) {
        let Ok(raw) = std::fs::read(self.library_file()) else {
            return;
        };
        let Ok(v) = serde_json::from_slice::<Value>(&raw) else {
            return;
        };
        // Old rows can still carry the `visual` flag from the removed
        // cover-video fallback. Scrub it on load so the UI never sees a
        // "COVER" label for a track that is now a real video.
        let scrub = |list: &Vec<Value>| -> Vec<Value> {
            let mut rows = list.clone();
            for row in &mut rows {
                if let Some(obj) = row.as_object_mut() {
                    obj.remove("visual");
                }
            }
            rows
        };
        if let Some(list) = v["recent"].as_array() {
            *self.recent.lock().unwrap_or_else(|e| e.into_inner()) = scrub(list);
        }
        if let Some(list) = v["favs"].as_array() {
            *self.favs.lock().unwrap_or_else(|e| e.into_inner()) = scrub(list);
        }
    }

    fn save_library(&self) {
        let recent = self.recent.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let favs = self.favs.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if recent.is_empty() && favs.is_empty() {
            let _ = std::fs::remove_file(self.library_file());
            return;
        }
        let body = json!({ "recent": recent, "favs": favs });
        let Ok(raw) = serde_json::to_vec(&body) else {
            return;
        };
        let _ = std::fs::create_dir_all("cache");
        let path = self.library_file();
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, raw).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }

    /// Remember a track the room started. Newest first, deduplicated by id.
    fn record_recent(&self, t: &Track) {
        let mut list = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        list.retain(|x| x["id"].as_str() != Some(t.id.as_str()));
        list.insert(0, track_json(t));
        list.truncate(40);
        drop(list);
        self.save_library();
    }

    fn is_liked(&self, id: &str) -> bool {
        self.favs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|x| x["id"].as_str() == Some(id))
    }

    /// Toggle a track in the liked list; returns the new state.
    fn toggle_fav(&self, t: &Track) -> bool {
        let liked = {
            let mut list = self.favs.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(pos) = list.iter().position(|x| x["id"].as_str() == Some(t.id.as_str())) {
                list.remove(pos);
                false
            } else {
                list.insert(0, track_json(t));
                list.truncate(200);
                true
            }
        };
        self.save_library();
        liked
    }

    /// Remaining sleep-timer seconds (0 = no timer).
    fn sleep_left(&self) -> u64 {
        let at = *self.sleep_at.lock().unwrap_or_else(|e| e.into_inner());
        if at == 0 {
            0
        } else {
            at.saturating_sub(now_secs())
        }
    }

    fn set_sleep(&self, minutes: u64) {
        let at = if minutes == 0 {
            0
        } else {
            now_secs() + minutes * 60
        };
        *self.sleep_at.lock().unwrap_or_else(|e| e.into_inner()) = at;
    }

    /// Messages of one private conversation, oldest first.
    fn dm_thread(&self, me: &str, peer: &str) -> Vec<Value> {
        self.dms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|m| {
                let a = m["a"].as_str().unwrap_or("");
                let b = m["b"].as_str().unwrap_or("");
                (a == me && b == peer) || (a == peer && b == me)
            })
            .cloned()
            .collect()
    }

    /// Conversation list for one viewer: peer, last line, time, avatar.
    fn dm_threads(&self, me: &str) -> Vec<Value> {
        let roster = self.members.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let mut out: Vec<Value> = Vec::new();
        let list = self.dms.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for m in list.iter() {
            let a = m["a"].as_str().unwrap_or("");
            let b = m["b"].as_str().unwrap_or("");
            if a != me && b != me {
                continue;
            }
            let peer = if a == me { b } else { a };
            if peer.is_empty() {
                continue;
            }
            let last = m["text"].as_str().unwrap_or("");
            let at = m["date"].as_i64().unwrap_or(0);
            if let Some(row) = out.iter_mut().find(|r| r["id"].as_str() == Some(peer)) {
                if at >= row["at"].as_i64().unwrap_or(0) {
                    row["last"] = json!(last);
                    row["ts"] = json!(m["ts"].as_str().unwrap_or(""));
                    row["at"] = json!(at);
                }
                continue;
            }
            let known = roster
                .iter()
                .find(|r| r["id"].as_str() == Some(peer))
                .cloned()
                .unwrap_or_else(|| json!({"name": "member", "username": "", "photo": ""}));
            out.push(json!({
                "id": peer,
                "name": known["name"].as_str().unwrap_or("member"),
                "username": known["username"].as_str().unwrap_or(""),
                "photo": known["photo"].as_str().unwrap_or(""),
                "tags": known.get("tags").cloned().unwrap_or(json!([])),
                "vc": known["vc"].as_bool().unwrap_or(false),
                "last": last,
                "ts": m["ts"].as_str().unwrap_or(""),
                "at": at,
            }));
        }
        out.sort_by(|x, y| y["at"].as_i64().unwrap_or(0).cmp(&x["at"].as_i64().unwrap_or(0)));
        out
    }

    /// Remember a sender so they keep showing up in the member list even when
    /// they are neither administrator nor currently talking.
    fn remember(&self, id: &str, name: &str, photo: &str) {
        if id.is_empty() {
            return;
        }
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        seen.retain(|m| m["id"].as_str() != Some(id));
        seen.insert(
            0,
            json!({
                "id": id,
                "name": name,
                "photo": photo,
                "role": "member",
                "status": "member",
                "tags": [{"k": "member", "label": "MEMBER"}],
                "vc": false,
                "host": false,
                "bot": false,
            }),
        );
        seen.truncate(60);
    }

    fn member_by_id(&self, id: &str) -> Option<Value> {
        let roster = self.members.lock().unwrap_or_else(|e| e.into_inner());
        roster.iter().find(|m| m["id"].as_str() == Some(id)).cloned()
    }

    /// Deployment owner (`OWNER_ID`) or one of the bot's own accounts.
    fn is_owner(&self, auth: &Auth) -> bool {
        if std::env::var("OWNER_ID")
            .ok()
            .and_then(|v| v.trim().parse::<i64>().ok())
            == Some(auth.id())
        {
            return true;
        }
        host_ids().contains(&auth.id())
    }

    /// Administrator of *this* chat. `refresh = true` re-asks Telegram (an
    /// actual permission check, or the switcher opening); `false` only reads
    /// the short cache so a 1.5 s state frame never blocks on the network.
    /// The playing room is answered from its local roster, which is always
    /// current and needs no cache at all.
    fn is_admin_of(&self, auth: &Auth, chat: i64, refresh: bool) -> bool {
        if self.is_owner(auth) {
            return true;
        }
        let sid = auth.id().to_string();
        if chat == self.active_chat() {
            let roster = self.members.lock().unwrap_or_else(|e| e.into_inner());
            return roster
                .iter()
                .find(|m| m["id"].as_str() == Some(sid.as_str()))
                .map(|m| {
                    m["host"].as_bool().unwrap_or(false)
                        || matches!(m["role"].as_str().unwrap_or(""), "owner" | "admin")
                })
                .unwrap_or(false);
        }
        let key = (auth.id(), chat);
        let now = now_secs();
        if !refresh {
            return self
                .admin_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&key)
                .filter(|(_, at)| now.saturating_sub(*at) < ADMIN_TTL)
                .map(|(ok, _)| *ok)
                .unwrap_or(false);
        }
        let ok = match tg_json(&format!("/member?chat_id={chat}&user_id={sid}")) {
            Some(m) => matches!(
                m["status"].as_str().unwrap_or(""),
                "creator" | "administrator" | "owner"
            ),
            None => false,
        };
        self.admin_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, (ok, now));
        ok
    }

    /// Fill in permission flags for the chats the switcher is about to show.
    /// Only stale/missing entries are queried, and at most eight per open, so
    /// a bot in many groups cannot turn one tap into a burst of bridge calls.
    fn warm_admin_cache(&self, auth: &Auth, ids: &[i64]) {
        let now = now_secs();
        let stale: Vec<i64> = {
            let cache = self.admin_cache.lock().unwrap_or_else(|e| e.into_inner());
            ids.iter()
                .copied()
                .filter(|id| {
                    !cache
                        .get(&(auth.id(), *id))
                        .map(|(_, at)| now.saturating_sub(*at) < ADMIN_TTL)
                        .unwrap_or(false)
                })
                .collect()
        };
        for id in stale.into_iter().take(8) {
            self.is_admin_of(auth, id, true);
        }
    }

    /* ── roster ── */

    fn refresh_roster(&self) {
        // One pass at a time. The SSE loop, the 45 s tick and every /api/state
        // used to be able to start a pass simultaneously, each resolving up to
        // 25 members over HTTP — that is what made the app stutter.
        if self.roster_busy.swap(true, Ordering::SeqCst) {
            return;
        }
        let chat = self.active_chat();
        let hosts = host_ids();

        if let Some(info) = tg_json(&format!("/chat?chat_id={chat}")) {
            if let Some(t) = info["title"].as_str() {
                if !t.is_empty() {
                    *self.title.lock().unwrap_or_else(|e| e.into_inner()) = t.to_string();
                }
            }
            if let Some(n) = info["members"].as_u64() {
                *self.member_count.lock().unwrap_or_else(|e| e.into_inner()) = n as usize;
            }
            if let Some(l) = info["invite"].as_str() {
                if !l.is_empty() {
                    *self.invite.lock().unwrap_or_else(|e| e.into_inner()) = l.to_string();
                }
            }
            // Keep the switcher's name/member count in sync with the room.
            self.touch_chat(
                chat,
                info["title"].as_str(),
                info["members"].as_u64().map(|n| n as usize),
            );
        }

        let mut list: Vec<Value> = Vec::new();
        if let Some(res) = tg_json(&format!("/admins?chat_id={chat}")) {
            if let Some(arr) = res["members"].as_array() {
                for m in arr {
                    if let Some(row) = roster_row(m, &hosts) {
                        list.push(row);
                    }
                }
            }
        }

        // Everyone else: the Bot API has no member listing, the assistant
        // session does, so pull the ids and resolve names/roles one by one.
        //
        // Resolving a big group one HTTP call at a time is what made the app
        // feel stuck: 120 sequential `getChatMember` calls every refresh held
        // the MTProto link and the request threads. Known members are reused
        // from the previous pass, and a pass resolves at most 25 new ids, so
        // steady state costs two calls and a cold start fills in gradually.
        let known: Vec<Value> = self.members.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let mut budget = 25usize;
        for id in participant_ids(chat).into_iter().take(200) {
            let sid = id.to_string();
            if list.iter().any(|m| m["id"].as_str() == Some(sid.as_str())) {
                continue;
            }
            if let Some(prev) = known.iter().find(|m| m["id"].as_str() == Some(sid.as_str())) {
                list.push(prev.clone());
                continue;
            }
            if budget == 0 {
                continue;
            }
            budget -= 1;
            if let Some(m) = tg_json(&format!("/member?chat_id={chat}&user_id={sid}")) {
                if let Some(row) = roster_row(&m, &hosts) {
                    list.push(row);
                }
            }
        }

        // Append everyone who has spoken and is not already listed.
        let seen = self.seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for s in seen {
            let id = s["id"].as_str().unwrap_or("");
            if id.is_empty() || list.iter().any(|m| m["id"].as_str() == Some(id)) {
                continue;
            }
            list.push(json!({
                "id": id,
                "name": s["name"].as_str().unwrap_or("member"),
                "username": "",
                "photo": s["photo"].as_str().unwrap_or(""),
                "role": "member",
                "status": "member",
                "custom": "",
                "tags": [{"k":"member","label":"MEMBER"}],
                "vc": false,
                "host": false,
                "bot": false,
            }));
        }

        // "In voice" means *in the call*, not *in the group*: ask Telegram for
        // the group-call participants. The bot's own accounts are always in
        // the call while a session is live, so they stay marked even if the
        // RPC is temporarily unavailable.
        let mut in_call: Vec<i64> = if self.is_live() {
            vc_participant_ids(chat)
        } else {
            Vec::new()
        };
        if self.is_live() && in_call.is_empty() {
            in_call = hosts.clone();
            *self.vc_known.lock().unwrap_or_else(|e| e.into_inner()) = false;
        } else {
            *self.vc_known.lock().unwrap_or_else(|e| e.into_inner()) = self.is_live();
        }
        for m in list.iter_mut() {
            let uid: i64 = m["id"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0);
            let is_host = m["host"].as_bool().unwrap_or(false);
            let vc = self.is_live() && (in_call.contains(&uid) || is_host);
            if let Some(o) = m.as_object_mut() {
                o.insert("vc".to_string(), json!(vc));
            }
        }

        if !list.is_empty() {
            *self.members.lock().unwrap_or_else(|e| e.into_inner()) = list;
        }
        *self.roster_at.lock().unwrap_or_else(|e| e.into_inner()) = now_secs();
        self.roster_busy.store(false, Ordering::SeqCst);
    }

    fn roster_age(&self) -> u64 {
        now_secs().saturating_sub(*self.roster_at.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /* ── incoming chat ── */

    fn push_incoming(&self, ev: ChatEvent) {
        // Mirror every group the bot is in. `tracks_chat` used to accept only
        // the *active* chat, so a second group produced no chat at all; each
        // message now lands in its own per-chat queue instead of one shared
        // timeline.
        let chat = ev.chat_id;
        if chat == 0 {
            return;
        }
        self.touch_chat(chat, None, None);
        // Telegram's own housekeeping (join/leave/pin) has no author and no
        // text; painting it as "member: " just adds noise.
        if ev.sender_id == 0 && ev.text.trim().is_empty() {
            return;
        }
        let sid = ev.sender_id.to_string();
        let known = self.member_by_id(&sid);
        let name = known
            .as_ref()
            .and_then(|m| m["name"].as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                if ev.name.trim().is_empty() {
                    "member".to_string()
                } else {
                    ev.name.clone()
                }
            });
        let photo = format!("/api/avatar/{sid}");
        let tags = known
            .as_ref()
            .and_then(|m| m.get("tags"))
            .cloned()
            .unwrap_or_else(|| json!([{"k":"member","label":"MEMBER"}]));

        if ev.sender_id != 0 {
            self.remember(&sid, &name, &photo);
        }

        let reply = if ev.reply_to != 0 {
            // Message ids are per chat: resolving against another group's queue
            // attached the wrong author/text to a reply.
            self.chat_msgs_for(chat, ev.reply_to).map(|m| {
                json!({
                    "mid": ev.reply_to,
                    "name": m["name"].as_str().unwrap_or(""),
                    "text": m["text"].as_str().unwrap_or(""),
                })
            })
        } else {
            None
        };

        self.push_message(chat, json!({
            "id": format!("tg{}", ev.mid),
            "mid": ev.mid,
            "from": sid,
            "name": name,
            "photo": photo,
            "tags": tags,
            "mine": false,
            "kind": ev.kind,
            "text": ev.text,
            "entities": ev.entities,
            "emoji": ev.emoji,
            "media": ev.media,
            "media_kind": ev.media_kind,
            "reply": reply,
            "ts": ev.ts,
            "date": ev.date,
        }));
        // Preview + ordering for the chat list.
        let preview = if ev.text.trim().is_empty() {
            match ev.kind.as_str() {
                "sticker" => format!("Sticker {}", ev.emoji).trim().to_string(),
                "photo" => "Photo".to_string(),
                "video" => "Video".to_string(),
                "voice" => "Voice message".to_string(),
                other => other.to_string(),
            }
        } else {
            ev.text.clone()
        };
        self.note_chat_activity(chat, &preview, &ev.ts, ev.date);
    }
}

/* ─────────────────────────── commands ─────────────────────────── */

fn cmd_pause(app: &MiniApp, pause: bool) -> bool {
    let Some(room) = app.room() else { return false };
    let mut g = room.lock().unwrap_or_else(|e| e.into_inner());
    if pause {
        if !g.pause() {
            return false;
        }
        let _ = crate::monitor::engine_pause(g.chat_id);
    } else {
        if !g.resume() {
            return false;
        }
        let _ = crate::monitor::engine_resume(g.chat_id);
    }
    true
}

fn cmd_seek(app: &MiniApp, position: i64) -> bool {
    let Some(room) = app.room() else { return false };
    let mut g = room.lock().unwrap_or_else(|e| e.into_inner());
    let duration = g.track.as_ref().map(|t| t.duration).unwrap_or(0);
    if duration <= 0 {
        return false;
    }
    let target = position.clamp(0, duration.saturating_sub(2));
    // `seek` is relative in RoomState; move to the absolute target.
    let delta = target - g.position;
    if g.seek(delta).is_none() {
        g.position = target;
        g.paused = false;
    }
    let chat_id = g.chat_id;
    let _ = crate::monitor::engine_restart(chat_id, &mut g);
    true
}

fn cmd_loop(app: &MiniApp, mode: i64) -> bool {
    let Some(room) = app.room() else { return false };
    let mut g = room.lock().unwrap_or_else(|e| e.into_inner());
    g.set_loop(mode);
    true
}

fn cmd_mute(app: &MiniApp, muted: bool) -> bool {
    let Some(room) = app.room() else { return false };
    let mut g = room.lock().unwrap_or_else(|e| e.into_inner());
    if muted {
        g.mute();
        let _ = crate::monitor::engine_mute(g.chat_id);
    } else {
        g.unmute();
        let _ = crate::monitor::engine_unmute(g.chat_id);
    }
    true
}

fn cmd_queue_add(app: &MiniApp, t: Track) {
    // The app can add to the queue before the chat ever started a room (the
    // very first search → + tap). Create it instead of dropping the track.
    let room = app
        .room()
        .unwrap_or_else(|| app.rooms.create_room(app.active_chat(), true));
    let mut g = room.lock().unwrap_or_else(|e| e.into_inner());
    let dup = g.queue.iter().any(|x| x.id == t.id)
        || (!t.url.is_empty() && g.track.as_ref().map(|c| c.url == t.url).unwrap_or(false));
    if dup {
        return;
    }
    g.queue.push(t);
    g.updated_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
}

fn cmd_queue_remove(app: &MiniApp, index: usize) -> bool {
    app.room()
        .map(|room| {
            let mut g = room.lock().unwrap_or_else(|e| e.into_inner());
            g.remove_at(index)
        })
        .unwrap_or(false)
}

fn cmd_queue_clear(app: &MiniApp) {
    if let Some(room) = app.room() {
        let mut g = room.lock().unwrap_or_else(|e| e.into_inner());
        g.clear_queue();
    }
}

fn cmd_play(app: &MiniApp, id: Option<&str>, query: Option<&str>, kind: Option<&str>) -> bool {
    // "audio" (default) or "video": the same song, downloaded as an m4a or an
    // mp4. `Track::video` already drives both the downloader (kind=video) and
    // the voice engine (stream the picture too), so this is the only switch.
    let video = kind.map(is_video_kind).unwrap_or(false);
    // A concrete id is resolved against the room and the search cache, so the
    // mini app can replay anything it already sees without a new search.
    if let Some(id) = id {
        if let Some(mut t) = find_track(app, id) {
            t.video = video;
            set_pending(app, &t);
            mark_play_pending();
            if let Some(slot) = play_slot().lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                slot(app.active_chat(), t.url.clone());
                return true;
            }
            start_track_async(app.active_chat(), app.rooms.clone(), t, true);
            return true;
        }
    }
    if let Some(q) = query {
        mark_play_pending();
        if let Some(slot) = play_slot().lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            slot(app.active_chat(), q.to_string());
            return true;
        }
        resolve_and_play(app.active_chat(), app.rooms.clone(), q.to_string(), video);
        return true;
    }
    false
}

fn is_video_kind(kind: &str) -> bool {
    matches!(kind.trim().to_ascii_lowercase().as_str(), "video" | "mp4" | "v")
}

/// Remember the track the user just picked so the app can paint it while it
/// downloads (and clear any previous failure).
fn set_pending(app: &MiniApp, t: &Track) {
    *app.pending.lock().unwrap_or_else(|e| e.into_inner()) = track_json(t);
    *app.play_error.lock().unwrap_or_else(|e| e.into_inner()) = Value::Null;
}

fn clear_pending(app: &MiniApp) {
    *app.pending.lock().unwrap_or_else(|e| e.into_inner()) = Value::Null;
    clear_play_pending();
}

/// Resolve a free-text query, then download + start it off the request thread
/// (a YouTube fetch takes seconds; the UI follows along on `/api/events`).
fn resolve_and_play(chat_id: i64, rooms: Rooms, query: String, video: bool) {
    thread::spawn(move || {
        let Some(mut t) = registry::resolve_query(&query, video).into_iter().next() else {
            return;
        };
        t.video = video;
        start_track_async(chat_id, rooms, t, true);
    });
}

/// Catalogue rows (`dz:…`, `am:…`, `sp:…`) come from the Iota metadata index.
/// Their bytes are not reachable from this host, so playing them means finding
/// the *same song* on a provider that can actually deliver audio. The match is
/// strict — first word plus a token-overlap floor — because silently playing a
/// different song is worse than failing loudly.
fn resolve_playable(track: &Track) -> Option<Track> {
    let want = crate::models::normalize_track_title(&track.title);
    if want.is_empty() {
        return None;
    }
    let first = want.split_whitespace().next().unwrap_or("").to_string();
    if first.is_empty() {
        return None;
    }
    let hits = registry::search_tracks(&track.title, false);
    let mut best: Option<(f32, Track)> = None;
    for h in hits {
        if h.id == track.id || h.id.is_empty() {
            continue;
        }
        let key = crate::models::normalize_track_title(&h.title);
        if key.is_empty() || key.split_whitespace().next().unwrap_or("") != first {
            continue;
        }
        let a: Vec<&str> = want.split_whitespace().filter(|w| w.len() > 2).collect();
        let b: Vec<&str> = key.split_whitespace().filter(|w| w.len() > 2).collect();
        if a.is_empty() || b.is_empty() {
            continue;
        }
        let common = a.iter().filter(|w| b.contains(w)).count() as f32;
        let score = common / a.len().min(b.len()) as f32;
        if score >= 0.5 && best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
            best = Some((score, h));
        }
    }
    best.map(|(_, mut alt)| {
        // Keep the label the user tapped (and the catalogue artwork/duration):
        // only the id/source change, so the row still reads as the same song.
        alt.title = track.title.clone();
        if !track.artwork.is_empty() {
            alt.artwork = track.artwork.clone();
        }
        if track.duration > 0 {
            alt.duration = track.duration;
        }
        alt.video = track.video;
        alt
    })
}

fn download(track: &Track) -> (String, String) {
    // Replays and re-queues hit the local copy: no network, no wait.
    let hit = registry::cached_path(track);
    if !hit.is_empty() {
        return (hit, String::new());
    }
    let mut err = String::new();
    let cancel = AtomicBool::new(false);
    let progress: Arc<ProgressFn> = Arc::new(|_p: f64, _t: f64, _d: i64, _s: f64| {});
    let path = registry::download_track(track, &mut err, &cancel, progress.as_ref());
    (path, err)
}

fn note(level: crate::logger::LogLevel, msg: &str) {
    crate::logger::Logger::instance().log(level, "miniapp", msg);
}

/// Download `track` and hand it to the voice engine. `force` also rewrites the
/// now-playing state (a fresh play); without it the caller already moved the
/// queue (skip / loop) and only needs the stream switched.
fn start_track_async(chat_id: i64, rooms: Rooms, track: Track, force: bool) {
    thread::spawn(move || {
        let mut pick = track;
        // Iota catalogue ids ("dz:…", "am:…") carry metadata only; swap in a
        // playable id for the very same song before spending time on a
        // download that is guaranteed to fail.
        if pick.id.contains(':') {
            if let Some(alt) = resolve_playable(&pick) {
                note(
                    crate::logger::LogLevel::Info,
                    &format!("catalogue {} → playable {} ({})", pick.id, alt.id, pick.title),
                );
                pick = alt;
            }
        }
        let (path, err) = download(&pick);
        // "Play video" must always be the real clip. The downloader walks its
        // player-client chain and merges a 720p H.264 video stream with its
        // audio for this exact id; if that still fails we report the error
        // instead of silently switching to audio or generating a cover
        // slideshow. Never substitute a different song — that is what made
        // every tap play the first result of the list.
        let Some(app) = app() else { return };
        if path.is_empty() {
            note(
                crate::logger::LogLevel::Error,
                &format!("download failed for {}: {err}", pick.title),
            );
            clear_pending(&app);
            *app.play_error.lock().unwrap_or_else(|e| e.into_inner()) = json!({
                "title": pick.title,
                "error": if err.is_empty() { "download failed".to_string() } else { err.clone() },
                "at": now_secs(),
            });
            // A fresh play that never got a file must not leave a track that
            // looks paused forever — that is exactly "the song won't play".
            if force {
                if let Some(room) = rooms.room_for(chat_id) {
                    let mut g = room.lock().unwrap_or_else(|e| e.into_inner());
                    g.roll_back_failed_start();
                }
            }
            return;
        }
        note(
            crate::logger::LogLevel::Info,
            &format!("play {} → {path}", pick.title),
        );
        clear_pending(&app);
        *app.play_error.lock().unwrap_or_else(|e| e.into_inner()) = Value::Null;
        // The mini app can start playback before any chat command created the
        // room, so create it on demand instead of dropping the track.
        let room = rooms.create_room(chat_id, true);
        // A group that just got a voice chat needs its own chat row even before
        // anyone types: the switcher is how you reach it.
        app.touch_chat(chat_id, None, None);
        let mut g = room.lock().unwrap_or_else(|e| e.into_inner());
        if force {
            g.play(pick.clone(), path.clone(), true);
        } else {
            // Skip/loop already advanced the queue: only bind the file.
            let same = g.track.as_ref().map(|t| t.id == pick.id).unwrap_or(false);
            if !same {
                return;
            }
            g.file_path = path.clone();
            g.position = 0;
            g.paused = false;
            g.playing = true;
        }
        match crate::modules::assistants() {
            Some(assists) => {
                crate::monitor::start_track(chat_id, &mut g, assists, &path, pick.video);
            }
            None => g.playing = true,
        }
        drop(g);
        // The call was just joined: refresh "who is in voice" right away
        // instead of waiting for the next roster tick.
        let hub = app.clone();
        thread::spawn(move || hub.refresh_roster());
    });
}

fn find_track(app: &MiniApp, id: &str) -> Option<Track> {
    if let Some(room) = app.room() {
        let g = room.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = g.track.as_ref() {
            if t.id == id {
                return Some(t.clone());
            }
        }
        if let Some(t) = g.queue.iter().find(|t| t.id == id) {
            return Some(t.clone());
        }
    }
    // A search hit is not in the room yet: that is the normal path from the
    // app's Search sheet, so resolve it from the cache the search filled.
    app.search_cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|t| t.id == id)
        .cloned()
}

fn cmd_skip(app: &MiniApp) {
    if let Some(slot) = skip_slot().lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        slot(app.active_chat());
        return;
    }
    let Some(room) = app.room() else { return };
    let next = {
        let mut g = room.lock().unwrap_or_else(|e| e.into_inner());
        g.next_track()
    };
    if let Some(t) = next {
        start_track_async(app.active_chat(), app.rooms.clone(), t, false);
    }
}

/* ─────────────────────────── HTTP ─────────────────────────── */

fn handle(app: Arc<MiniApp>, mut stream: TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let Some(req) = read_request(&mut stream)? else {
        return Ok(());
    };

    let path = req.path.clone();
    let auth = app.authorize(&req);
    if path.starts_with("/api/") && !auth.ok {
        let hint = match auth.via {
            "not-member" => "You are not a member of this room.",
            "not-allowed" => "This account is not allowed here.",
            _ => "Open this app from Telegram.",
        };
        return respond_json(
            &mut stream,
            401,
            &json!({ "error": "unauthorized", "reason": auth.via, "hint": hint }),
        );
    }
    if path == "/api/events" {
        return stream_events(app, stream, auth);
    }
    if path.starts_with("/api/") {
        if serve_media(&app, &mut stream, &req) {
            return Ok(());
        }
        if serve_image(&app, &mut stream, &req) {
            return Ok(());
        }
        let (code, body) = route_api(&app, &req, &auth);
        return respond_json(&mut stream, code, &body);
    }
    serve_static(&app, &mut stream, &path)
}

struct Request {
    method: String,
    path: String,
    query: String,
    body: Vec<u8>,
    header_token: Option<String>,
    /// `X-Telegram-Init-Data` — the signed Telegram WebApp payload.
    init_data: Option<String>,
    /// Parsed `Cookie` header (only `parvati_auth` is read).
    cookies: std::collections::HashMap<String, String>,
    /// `Range: bytes=a-b` — only the media stream uses it, so it is parsed
    /// once here instead of growing a full header map.
    range: Option<String>,
}

/// Per-SSE-connection frame dedupe. This used to live on `MiniApp`, which meant
/// the first viewer's frame marked the payload "sent" for everybody else — two
/// phones on the same room made each other's updates disappear.
struct Cursor {
    state: String,
    messages: String,
    at: u64,
}

impl Cursor {
    fn new() -> Self {
        Self {
            state: String::new(),
            messages: String::new(),
            at: 0,
        }
    }
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<Option<Request>> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 1024];
    let header_end;
    loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(p) = find(&buf, b"\r\n\r\n") {
            header_end = p;
            break;
        }
        if buf.len() > MAX_BODY {
            return Ok(None);
        }
    }

    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.lines();
    let first = lines.next().unwrap_or_default();
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target, String::new()),
    };

    let mut len = 0usize;
    let mut header_token = None;
    let mut init_data = None;
    let mut cookies = std::collections::HashMap::new();
    let mut range = None;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                len = v.trim().parse().unwrap_or(0);
            } else if k.eq_ignore_ascii_case("x-miniapp-token") {
                header_token = Some(v.trim().to_string());
            } else if k.eq_ignore_ascii_case("x-telegram-init-data") {
                // Header values are percent-encoded by the page so the raw
                // `&`/`=` inside initData survive the header.
                init_data = Some(urldecode(v.trim()));
            } else if k.eq_ignore_ascii_case("cookie") {
                for pair in v.trim().split(';') {
                    if let Some((ck, cv)) = pair.trim().split_once('=') {
                        cookies.insert(ck.trim().to_string(), urldecode(cv.trim()));
                    }
                }
            } else if k.eq_ignore_ascii_case("range") {
                range = Some(v.trim().to_string());
            }
        }
    }
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < len {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(len.min(MAX_BODY));

    Ok(Some(Request { method, path, query, body, header_token, init_data, cookies, range }))
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn respond_json(stream: &mut TcpStream, code: u16, body: &Value) -> std::io::Result<()> {
    let payload = body.to_string();
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\n\
         Content-Type: application/json; charset=utf-8\r\n\
         Content-Length: {len}\r\n\
         Cache-Control: no-store\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Headers: content-type\r\n\
         Connection: close\r\n\r\n",
        reason = if code == 200 { "OK" } else { "Error" },
        len = payload.len(),
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(payload.as_bytes())?;
    stream.flush()
}

fn respond_bytes(
    stream: &mut TcpStream,
    code: u16,
    mime: &str,
    body: &[u8],
    max_age: u32,
) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\n\
         Content-Type: {mime}\r\n\
         Content-Length: {len}\r\n\
         Cache-Control: public, max-age={max_age}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Connection: close\r\n\r\n",
        reason = if code == 200 { "OK" } else { "Not Found" },
        len = body.len(),
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

fn qs(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        if k == key {
            Some(urldecode(v))
        } else {
            None
        }
    })
}

fn urldecode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn valid_key(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 96
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

/// `/api/media?id=<track id>&kind=video|audio` — the exact file the voice
/// engine is streaming. A video pick shows a `<video>` player in the mini app
/// (the sound still comes from the call). Range requests are honoured: a
/// `<video>` element seeks with them, and Safari will not play without them.
fn serve_media(_app: &MiniApp, stream: &mut TcpStream, req: &Request) -> bool {
    if req.method != "GET" || req.path != "/api/media" {
        return false;
    }
    let Some(id) = qs(&req.query, "id") else { return false };
    if !valid_key(&id) {
        let _ = respond_bytes(stream, 400, "text/plain", b"bad id", 0);
        return true;
    }
    let video = qs(&req.query, "kind").map(|k| is_video_kind(&k)).unwrap_or(false);
    let dir = if video { "video" } else { "audio" };
    let prefix = format!("{dir}_{id}");
    let mut file: Option<PathBuf> = None;
    if let Ok(entries) = std::fs::read_dir("downloads") {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(rest) = name.strip_prefix(&prefix) else { continue };
            // `audio_<id>.m4a` — the dot stops one id from matching another
            // that merely starts with it.
            if !rest.starts_with('.') || rest.len() < 2 {
                continue;
            }
            // Legacy cover-video markers and in-flight temp files are not media.
            if rest == ".visual" || rest == ".part" {
                continue;
            }
            if let Ok(md) = e.metadata() {
                if md.len() > 0 {
                    file = Some(e.path());
                    break;
                }
            }
        }
    }
    let Some(file) = file else {
        let _ = respond_bytes(stream, 404, "text/plain", b"not ready", 0);
        return true;
    };
    let Ok(mut f) = std::fs::File::open(&file) else {
        let _ = respond_bytes(stream, 404, "text/plain", b"not ready", 0);
        return true;
    };
    let total = f.metadata().map(|m| m.len()).unwrap_or(0);
    if total == 0 {
        let _ = respond_bytes(stream, 404, "text/plain", b"empty", 0);
        return true;
    }
    // Sniff the container instead of trusting the name: the downloader can
    // hand back mp3, m4a or webm depending on what the source offered, and a
    // wrong content-type makes the player refuse a perfectly good file.
    let mut head = [0u8; 12];
    let sniffed = f.read_exact(&mut head).ok().map(|_| head);
    let _ = f.rewind();
    let mime = if video {
        "video/mp4"
    } else {
        match sniffed {
            Some(h) if &h[0..3] == b"ID3" || (h[0] == 0xFF && h[1] & 0xE0 == 0xE0) => "audio/mpeg",
            Some(h) if &h[4..8] == b"ftyp" => "audio/mp4",
            Some(h) if h[0..4] == [0x1A, 0x45, 0xDF, 0xA3] => "audio/webm",
            Some(h) if &h[0..4] == b"OggS" => "audio/ogg",
            _ => "audio/mpeg",
        }
    };

    let range = req.range.as_deref().map(|h| parse_range(h, total));
    let (code, start, end) = match range {
        None => (200u16, 0u64, total - 1),
        Some(Some((s, e))) => (206u16, s, e),
        // Header present but unsatisfiable.
        Some(None) => {
            let head = format!(
                "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\n\
                 Content-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.write_all(head.as_bytes());
            return true;
        }
    };
    let len = end - start + 1;
    let head = if code == 206 {
        format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Type: {mime}\r\nContent-Length: {len}\r\n\
             Content-Range: bytes {start}-{end}/{total}\r\nAccept-Ranges: bytes\r\n\
             Cache-Control: no-store\r\nConnection: close\r\n\r\n"
        )
    } else {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {len}\r\n\
             Accept-Ranges: bytes\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n"
        )
    };
    if stream.write_all(head.as_bytes()).is_err() {
        return true;
    }
    if f.seek(std::io::SeekFrom::Start(start)).is_err() {
        return true;
    }
    let mut left = len;
    let mut buf = vec![0u8; 64 * 1024];
    while left > 0 {
        let want = buf.len().min(left as usize);
        match f.read(&mut buf[..want]) {
            Ok(0) => break,
            Ok(n) => {
                if stream.write_all(&buf[..n]).is_err() {
                    break;
                }
                left -= n as u64;
            }
            Err(_) => break,
        }
    }
    true
}

/// `bytes=a-b` / `bytes=a-` / `bytes=-n` → inclusive `(start, end)`.
fn parse_range(header: &str, total: u64) -> Option<(u64, u64)> {
    let spec = header.trim().strip_prefix("bytes=")?;
    let (a, b) = spec.split_once('-')?;
    if a.trim().is_empty() {
        let n: u64 = b.trim().parse().ok()?;
        if n == 0 {
            return None;
        }
        return Some((total.saturating_sub(n), total - 1));
    }
    let start: u64 = a.trim().parse().ok()?;
    if start >= total {
        return None;
    }
    let end = if b.trim().is_empty() {
        total - 1
    } else {
        b.trim().parse::<u64>().ok()?.min(total - 1)
    };
    if start > end {
        return None;
    }
    Some((start, end))
}

/// Image endpoints are binary, so they bypass the JSON router.
fn serve_image(app: &MiniApp, stream: &mut TcpStream, req: &Request) -> bool {
    let path = req.path.as_str();

    // /api/tgmedia/<key> — media the bot downloaded over MTProto.
    if let Some(key) = path.strip_prefix("/api/tgmedia/") {
        if !valid_key(key) {
            let _ = respond_bytes(stream, 404, "text/plain", b"bad key", 0);
            return true;
        }
        let file = PathBuf::from("cache/tg").join(format!("{key}.bin"));
        return match std::fs::read(&file) {
            Ok(bytes) => {
                let _ = respond_bytes(stream, 200, mime_of(&bytes), &bytes, 86400);
                true
            }
            Err(_) => {
                let _ = respond_bytes(stream, 404, "text/plain", b"not ready", 0);
                true
            }
        };
    }

    // /api/emoji/<doc_id> — premium (custom) emoji, resolved to a sticker
    // document once and then served from the media cache.
    if let Some(doc) = path.strip_prefix("/api/emoji/") {
        if !valid_key(doc) {
            return false;
        }
        let key = format!("em:{doc}");
        if let Some((bytes, mime, at)) = app
            .media
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .cloned()
        {
            if now_secs().saturating_sub(at) < 3600 {
                let _ = respond_bytes(stream, 200, &mime, &bytes, 86400);
                return true;
            }
        }
        let fid = tg_json(&format!("/emoji?ids={doc}"))
            .and_then(|v| v["stickers"][0]["file_id"].as_str().map(|s| s.to_string()));
        let Some(fid) = fid else {
            let _ = respond_bytes(stream, 404, "text/plain", b"no emoji", 30);
            return true;
        };
        let mut encoded = String::new();
        for b in fid.as_bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'=' => {
                    encoded.push(*b as char)
                }
                _ => encoded.push_str(&format!("%{b:02X}")),
            }
        }
        return match tg_call("GET", &format!("/img?fid={encoded}"), None) {
            Some((200, ctype, bytes)) if !bytes.is_empty() => {
                let mime = if ctype.starts_with("image/") {
                    ctype
                } else {
                    mime_of(&bytes).to_string()
                };
                let mut cache = app.media.lock().unwrap_or_else(|e| e.into_inner());
                if cache.len() > 400 {
                    cache.clear();
                }
                cache.insert(key, (bytes.clone(), mime.clone(), now_secs()));
                let _ = respond_bytes(stream, 200, &mime, &bytes, 86400);
                true
            }
            _ => {
                let _ = respond_bytes(stream, 404, "text/plain", b"no emoji", 30);
                true
            }
        };
    }

    // /api/avatar/<uid> and /api/tgimg?fid=... — proxied + memory cached.
    // Chat rows ask for 96 px (the Telegram original is ~150 KB and a roster
    // of ten of them made the first paint crawl); the profile card asks for
    // 320 px. The size is part of the cache key so the two never collide.
    let (key, upstream) = if let Some(uid) = path.strip_prefix("/api/avatar/") {
        if !valid_key(uid) {
            return false;
        }
        let px = qs(&req.query, "s")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(96)
            .clamp(32, 320);
        (
            format!("av:{uid}:{px}"),
            format!("/avatar?user_id={uid}&s={px}"),
        )
    } else if path == "/api/tgimg" {
        let fid = qs(&req.query, "fid").unwrap_or_default();
        if fid.is_empty() || fid.len() > 400 {
            let _ = respond_bytes(stream, 404, "text/plain", b"no file", 0);
            return true;
        }
        let mut encoded = String::new();
        for b in fid.as_bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'=' => {
                    encoded.push(*b as char)
                }
                _ => encoded.push_str(&format!("%{b:02X}")),
            }
        }
        (format!("fid:{fid}"), format!("/img?fid={encoded}"))
    } else {
        return false;
    };

    if let Some((bytes, mime, at)) = app
        .media
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key)
        .cloned()
    {
        if now_secs().saturating_sub(at) < 3600 {
            let _ = respond_bytes(stream, 200, &mime, &bytes, 86400);
            return true;
        }
    }

    match tg_call("GET", &upstream, None) {
        Some((200, ctype, bytes)) if !bytes.is_empty() => {
            let mime = if ctype.starts_with("image/") {
                ctype
            } else {
                mime_of(&bytes).to_string()
            };
            let mut cache = app.media.lock().unwrap_or_else(|e| e.into_inner());
            if cache.len() > 400 {
                cache.clear();
            }
            cache.insert(key, (bytes.clone(), mime.clone(), now_secs()));
            let _ = respond_bytes(stream, 200, &mime, &bytes, 86400);
            true
        }
        _ => {
            let _ = respond_bytes(stream, 404, "text/plain", b"no photo", 30);
            true
        }
    }
}

/* ─────────────────── saved stickers (per viewer) ─────────────────── */

/// "Add to Stickers" is personal, not a room setting: one JSON file per
/// Telegram viewer under `cache/`, so a saved pack follows the account across
/// restarts and is never broadcast to anyone else. The viewer id comes from
/// `initDataUnsafe`, so it is sanitised into a filename here as well.
fn saved_stickers_file(me: &str) -> PathBuf {
    let mut safe: String = me
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(48)
        .collect();
    if safe.is_empty() {
        safe = "anon".to_string();
    }
    PathBuf::from("cache").join(format!("stickers-{safe}.json"))
}

fn saved_stickers_read(me: &str) -> Vec<Value> {
    let Ok(raw) = std::fs::read(saved_stickers_file(me)) else {
        return Vec::new();
    };
    match serde_json::from_slice::<Vec<Value>>(&raw) {
        Ok(list) => list,
        Err(_) => Vec::new(),
    }
}

fn saved_stickers_write(me: &str, list: &[Value]) -> bool {
    let Ok(body) = serde_json::to_vec(list) else {
        return false;
    };
    let _ = std::fs::create_dir_all("cache");
    let path = saved_stickers_file(me);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).is_ok() && std::fs::rename(&tmp, &path).is_ok()
}

/// Merge every key of `src` into `dst`. Profile cards are assembled from three
/// sources (roster, Bot API `getChat`, local chat stats); without a helper the
/// same "insert if present" loop would be repeated for each one.
fn merge_obj(dst: &mut serde_json::Map<String, Value>, src: &Value) {
    if let Some(o) = src.as_object() {
        for (k, v) in o {
            dst.insert(k.clone(), v.clone());
        }
    }
}

fn route_api(app: &MiniApp, req: &Request, auth: &Auth) -> (u16, Value) {
    let body: Value = if req.body.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&req.body).unwrap_or_else(|_| json!({}))
    };

    if req.method == "GET" && req.path == "/api/state" {
        // Never block the request on Telegram: the roster thread keeps the
        // cache warm, and a cold cache is filled in the background so the
        // first paint is instant (names arrive on the next SSE frame).
        let cold = app.members.lock().unwrap_or_else(|e| e.into_inner()).is_empty();
        if cold && app.roster_age() > 5 {
            if let Some(hub) = APP.get().cloned() {
                thread::spawn(move || hub.refresh_roster());
            }
        }
        return (200, app.snapshot(true, Some(auth)));
    }

    // ── Profile card: tap a name or avatar in the chat / member list. ──
    // Three sources are merged (roster → Bot API getChat → local chat stats),
    // and every one of them is optional. A user who only ever posted one
    // message and never became an admin still gets a useful card: name,
    // username, role tags, how many messages they sent here and their bio.
    if req.method == "GET" && req.path == "/api/user" {
        let id = qs(&req.query, "id").unwrap_or_default();
        if !valid_key(&id) {
            return (400, json!({ "error": "id" }));
        }
        let mut out = serde_json::Map::new();

        // 1) Room role, custom title, "in voice" and the tag chips.
        let known = app.member_by_id(&id);
        if let Some(m) = &known {
            merge_obj(&mut out, m);
        } else if let Some(m) = tg_json(&format!(
            "/member?chat_id={}&user_id={id}",
            app.active_chat()
        ))
        .and_then(|m| roster_row(&m, &host_ids()))
        {
            merge_obj(&mut out, &m);
        }

        // 2) Public Telegram profile. Overrides the roster's (possibly stale)
        // name/username with the exact values, and adds bio / photo count.
        if let Some(u) = tg_json(&format!("/user?user_id={id}")) {
            merge_obj(&mut out, &u);
            let first = u["first_name"].as_str().unwrap_or("");
            let last = u["last_name"].as_str().unwrap_or("");
            let full = format!("{first} {last}").trim().to_string();
            if !full.is_empty() {
                out.insert("name".to_string(), json!(full));
            }
        }

        // 3) Never let an upstream decide the id or the avatar URL: the mini
        //    app always loads photos through the cache proxy.
        out.insert("id".to_string(), json!(id));
        out.insert("photo".to_string(), json!(format!("/api/avatar/{id}")));
        out.insert(
            "photo_big".to_string(),
            json!(format!("/api/avatar/{id}?s=320")),
        );

        // 4) Local facts Telegram cannot expose (privacy): how much this
        //    person talked in the mirrored room chat and their last stamp.
        //    Counted over every known chat, so a profile does not depend on
        //    which tab the viewer happens to have open.
        let msgs = {
            let ids: Vec<i64> = {
                let meta = app.chat_meta.lock().unwrap_or_else(|e| e.into_inner());
                meta.keys().copied().collect()
            };
            let mut all: Vec<Value> = Vec::new();
            for c in ids {
                all.extend(app.chat_msgs(c));
            }
            all
        };
        let posts: usize = msgs
            .iter()
            .filter(|m| m["from"].as_str() == Some(id.as_str()))
            .count();
        let last = msgs
            .iter()
            .filter(|m| m["from"].as_str() == Some(id.as_str()))
            .max_by_key(|m| m["date"].as_i64().unwrap_or(0))
            .and_then(|m| m["ts"].as_str())
            .unwrap_or("")
            .to_string();
        out.insert("messages".to_string(), json!(posts));
        out.insert("last_seen".to_string(), json!(last));
        out.insert(
            "in_voice".to_string(),
            json!(known.as_ref().map(|m| m["vc"] == true).unwrap_or(false)),
        );
        if out
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .is_empty()
        {
            out.insert("name".to_string(), json!("member"));
        }
        if !out.contains_key("tags") {
            out.insert("tags".to_string(), json!([{"k":"member","label":"MEMBER"}]));
        }
        if !out.contains_key("role") {
            out.insert("role".to_string(), json!("member"));
            out.insert("status".to_string(), json!("member"));
        }
        return (200, Value::Object(out));
    }

    if req.method == "GET" && req.path == "/api/stickers" {
        let set = qs(&req.query, "set").unwrap_or_default();
        if !valid_key(&set) {
            return (400, json!({ "error": "bad set" }));
        }
        return match tg_json(&format!("/set?name={set}")) {
            Some(v) => (200, v),
            None => (502, json!({ "error": "sticker set unavailable" })),
        };
    }

    // "Add to Stickers": the viewer's personal saved list. A GET is cheap and
    // never touches Telegram, so the Saved tab opens instantly even offline.
    if req.method == "GET" && req.path == "/api/stickers/saved" {
        let me = auth.me(qs(&req.query, "me").unwrap_or_default());
        if me.is_empty() || me.len() > 64 {
            return (400, json!({ "error": "me" }));
        }
        return (200, json!({ "stickers": saved_stickers_read(&me) }));
    }

    if req.method == "GET" && req.path == "/api/search" {
        let q = qs(&req.query, "q").unwrap_or_default();
        if q.trim().is_empty() {
            return (200, json!({ "results": [] }));
        }
        let mut yt_hits = crate::platforms::registry::search_tracks(&q, false);
        // The Iota catalogue (Deezer / Apple Music / Spotify / SoundCloud
        // metadata) is merged in *inside* the list, not after it: YouTube
        // already fills the whole page, so appending put the catalogue past
        // the cut and it never showed up. Ten YouTube hits stay on top, the
        // catalogue gets the next slots, and the rest of YouTube follows.
        let mut catalogue: Vec<Track> = Vec::new();
        if crate::platforms::registry::iota_active() {
            let mut seen: Vec<String> = yt_hits
                .iter()
                .map(|t| crate::models::normalize_track_title(&t.title))
                .collect();
            // `yt:<id>` catalogue rows are the same video the list already has
            // (the artist suffix makes the title key differ) — match on the id
            // as well so the page never shows one song twice.
            let mut seen_ids: Vec<String> = yt_hits.iter().map(|t| t.id.clone()).collect();
            for t in crate::platforms::registry::iota_search(&q, 10) {
                let key = crate::models::normalize_track_title(&t.title);
                let bare = t
                    .id
                    .split_once(':')
                    .map(|(_, rest)| rest.to_string())
                    .unwrap_or_else(|| t.id.clone());
                if key.is_empty()
                    || seen.contains(&key)
                    || seen_ids.iter().any(|x| *x == t.id || *x == bare)
                {
                    continue;
                }
                seen.push(key);
                seen_ids.push(t.id.clone());
                catalogue.push(t);
                if catalogue.len() >= 6 {
                    break;
                }
            }
        }
        let mut tracks: Vec<Track> = Vec::with_capacity(20);
        let head = yt_hits.len().min(10);
        tracks.extend(yt_hits.drain(..head));
        tracks.extend(catalogue);
        tracks.extend(yt_hits);
        tracks.truncate(20);
        // Remember the hits: the client plays/queues by id, and a search
        // result is not in the room yet.
        {
            let mut cache = app.search_cache.lock().unwrap_or_else(|e| e.into_inner());
            for t in tracks.iter().take(20) {
                if let Some(slot) = cache.iter_mut().find(|x| x.id == t.id) {
                    *slot = t.clone();
                } else {
                    cache.push(t.clone());
                }
            }
            let overflow = cache.len().saturating_sub(300);
            if overflow > 0 {
                cache.drain(0..overflow);
            }
        }
        let results: Vec<Value> = tracks.iter().take(20).map(track_json).collect();
        return (200, json!({ "results": results }));
    }

    if req.method == "GET" && req.path == "/api/dm" {
        let me = auth.me(qs(&req.query, "me").unwrap_or_default());
        if me.is_empty() {
            return (400, json!({ "error": "me" }));
        }
        match qs(&req.query, "peer") {
            Some(peer) if !peer.is_empty() => {
                return (200, json!({
                    "peer": peer,
                    "messages": app.dm_thread(&me, &peer),
                }))
            }
            _ => return (200, json!({ "threads": app.dm_threads(&me) })),
        }
    }

    // Chat switcher. The app refreshes it with a plain GET fetch, so it has
    // to be handled before the POST-only router below (a GET falling through
    // there used to answer 404 and the sheet never got its permissions).
    if req.method == "GET" && req.path == "/api/chats" {
        let list = app.chat_list_warmed(Some(auth));
        // Groups seen only through a message carry a placeholder title. Ask
        // Telegram for a few real names in the background; the next open fills
        // in the rest so one tap cannot turn into a burst of bridge calls.
        let unnamed: Vec<i64> = list
            .iter()
            .filter(|r| {
                r["title"]
                    .as_str()
                    .map(|t| t.starts_with("Chat "))
                    .unwrap_or(true)
            })
            .filter_map(|r| r["id"].as_str().and_then(|s| s.parse::<i64>().ok()))
            .take(4)
            .collect();
        for id in unnamed {
            if let Some(hub) = crate::webapp::app() {
                thread::spawn(move || hub.resolve_chat_meta(id));
            }
        }
        return (
            200,
            json!({
                "chats": list,
                "active": app.chat_for(Some(auth)).to_string(),
            }),
        );
    }

    // Bans recorded by this app for the chat the viewer is looking at. Only
    // admins may see or change them; the Bot API cannot enumerate Telegram's
    // own ban history, so this is deliberately scoped to app-made bans.
    if req.method == "GET" && req.path == "/api/chat/banned" {
        let target = app.chat_for(Some(auth));
        if !app.is_owner(auth) && !app.is_admin_of(auth, target, true) {
            return (403, json!({ "error": "not allowed" }));
        }
        return (
            200,
            json!({
                "banned": app.banned_list(target),
                "chat": target.to_string(),
            }),
        );
    }

    if req.method != "POST" {
        return (404, json!({ "error": "not found" }));
    }

    match req.path.as_str() {
        "/api/toggle" => {
            let paused = app
                .room()
                .map(|r| r.lock().unwrap_or_else(|e| e.into_inner()).paused)
                .unwrap_or(true);
            cmd_pause(app, !paused);
        }
        "/api/pause" => {
            cmd_pause(app, true);
        }
        "/api/resume" => {
            cmd_pause(app, false);
        }
        "/api/seek" => {
            cmd_seek(app, body["position"].as_i64().unwrap_or(0));
        }
        "/api/loop" => {
            cmd_loop(app, body["mode"].as_i64().unwrap_or(0));
        }
        "/api/mute" => {
            cmd_mute(app, body["muted"].as_bool().unwrap_or(false));
        }
        "/api/skip" => cmd_skip(app),
        "/api/play" => {
            let id = body["id"].as_str();
            let query = body["query"].as_str();
            let kind = body["kind"].as_str();
            if !cmd_play(app, id, query, kind) {
                return (404, json!({ "error": "track not found" }));
            }
        }
        "/api/queue/add" => {
            let id = body["id"].as_str().unwrap_or_default();
            let video = body["kind"].as_str().map(is_video_kind).unwrap_or(false);
            let mut picked = find_track(app, id);
            if picked.is_none() {
                if let Some(q) = body["query"].as_str() {
                    picked = crate::platforms::registry::resolve_query(q, video)
                        .into_iter()
                        .next();
                }
            }
            match picked {
                Some(mut t) => {
                    t.video = video;
                    // Nothing playing? A song "added to the queue" would just
                    // sit there forever — the monitor only advances when a
                    // track ends. Start it now instead.
                    let idle = match app.room() {
                        Some(r) => {
                            let g = r.lock().unwrap_or_else(|e| e.into_inner());
                            g.track.is_none() || !g.playing
                        }
                        None => true,
                    };
                    if idle {
                        set_pending(app, &t);
                        start_track_async(app.active_chat(), app.rooms.clone(), t, true);
                    } else {
                        cmd_queue_add(app, t);
                    }
                }
                None => return (404, json!({ "error": "track not found" })),
            }
        }
        "/api/queue/remove" => {
            let idx = body["index"].as_u64().unwrap_or(0) as usize;
            cmd_queue_remove(app, idx);
        }
        "/api/queue/move" => {
            let from = body["from"].as_u64().unwrap_or(0) as usize;
            let to = body["to"].as_u64().unwrap_or(0) as usize;
            let moved = app
                .room()
                .map(|room| {
                    let mut g = room.lock().unwrap_or_else(|e| e.into_inner());
                    g.move_to(from, to)
                })
                .unwrap_or(false);
            if !moved {
                return (400, json!({ "error": "bad index" }));
            }
        }
        "/api/fav" => {
            // Toggle the liked state of a track the app can already see.
            let id = body["id"].as_str().unwrap_or_default();
            let Some(t) = find_track(app, id) else {
                return (404, json!({ "error": "track not found" }));
            };
            let liked = app.toggle_fav(&t);
            return (200, json!({ "ok": true, "liked": liked }));
        }
        "/api/sleep" => {
            // minutes: 0 cancels. The watcher thread stops playback when it
            // fires, so the phone can be locked and the call still ends.
            let minutes = body["minutes"].as_u64().unwrap_or(0);
            app.set_sleep(minutes);
            return (200, json!({ "ok": true, "sleep": app.sleep_left() }));
        }
        "/api/queue/clear" => cmd_queue_clear(app),
        "/api/chat/send" => {
            let kind = body["kind"].as_str().unwrap_or("text").to_string();
            let text = body["text"].as_str().unwrap_or("").to_string();
            let sticker = body["sticker"].as_str().unwrap_or("").to_string();
            if kind == "text" && text.trim().is_empty() {
                return (400, json!({ "error": "empty" }));
            }
            // Send into the chat the viewer is looking at, not whichever room
            // happens to be playing — otherwise a message typed in group B
            // landed in group A's Telegram chat.
            let target = app.chat_for(Some(&auth));
            let reply = body["replyTo"].as_i64().unwrap_or(0);
            // Keep the client's rich copy for local rendering, but only send
            // the fields the Bot API understands (it rejects unknown keys).
            let entities = body["entities"].clone();
            let send_entities: Vec<Value> = entities
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|e| {
                            let t = e["type"].as_str()?;
                            let mut m = serde_json::Map::new();
                            m.insert("type".into(), json!(t));
                            m.insert("offset".into(), json!(e["offset"].as_i64().unwrap_or(0)));
                            m.insert("length".into(), json!(e["length"].as_i64().unwrap_or(0)));
                            if let Some(id) = e["custom_emoji_id"].as_str() {
                                m.insert("custom_emoji_id".into(), json!(id));
                            }
                            if let Some(u) = e["url"].as_str() {
                                m.insert("url".into(), json!(u));
                            }
                            Some(Value::Object(m))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let mut req_body = json!({
                "chat_id": target.to_string(),
                "kind": kind,
                "text": text,
                "reply_to": reply,
                "entities": send_entities,
            });
            if kind == "sticker" {
                req_body["sticker"] = json!(sticker);
                // A sticker mirrored from the Telegram group has no Bot API
                // file_id, only the bytes the bot cached under a media key.
                // The tgapi bridge uploads those bytes with multipart
                // sendSticker, so "Saved" stickers work for chat stickers too,
                // not just the ones that came out of a public pack.
                let media_key = body["mediaKey"].as_str().unwrap_or("");
                if !media_key.is_empty() && media_key.len() <= 96 {
                    req_body["media_key"] = json!(media_key);
                }
            }
            // Preview/seed path: store the message for this instance only.
            // Handy for screenshots and demos without posting to the group.
            if body["local"].as_bool().unwrap_or(false) {
                let sender_id = auth.me(body["senderId"].as_str().unwrap_or("").to_string());
                let sender_name = auth.display_name(
                    body["senderName"].as_str().unwrap_or("member").to_string(),
                );
                let msg = json!({
                    "id": format!("loc{}", now_ms()),
                    "mid": 0,
                    "from": sender_id,
                    "name": sender_name,
                    "photo": if sender_id.is_empty() { String::new() } else { format!("/api/avatar/{sender_id}") },
                    "tags": body.get("tags").cloned().unwrap_or(json!([])),
                    "mine": false,
                    "kind": kind,
                    "text": text,
                    "entities": entities,
                    "emoji": body["emoji"].as_str().unwrap_or(""),
                    "media": body["media"].as_str()
                        .or_else(|| body["thumb"].as_str())
                        .unwrap_or(""),
                    "media_kind": body["media_kind"].as_str()
                        .unwrap_or(if kind == "sticker" { "image" } else { "" }),
                    "sticker": body["thumb"].as_str().unwrap_or(""),
                    "ts": now_hm(),
                    "date": now_secs() as i64,
                    "reacts": body.get("reacts").cloned().unwrap_or(json!([])),
                });
                app.push_message(target, msg.clone());
                return (200, json!({ "ok": true, "message": msg }));
            }

            match tg_post("/send", &req_body) {
                Some((code, res)) if code == 200 && res["ok"] == true => {
                    // Attribute the message to the verified Telegram user, not
                    // to whatever id the client put in the body.
                    let sender_id = auth.me(body["senderId"].as_str().unwrap_or("").to_string());
                    let sender_name =
                        auth.display_name(body["senderName"].as_str().unwrap_or("you").to_string());
                    let mid = res["message_id"].as_i64().unwrap_or(0);
                    let msg = json!({
                        "id": format!("l{mid}"),
                        "mid": mid,
                        "from": sender_id,
                        "name": sender_name,
                        "photo": format!("/api/avatar/{sender_id}"),
                        "tags": body.get("tags").cloned().unwrap_or(json!([])),
                        // The client decides "mine" from the viewer id, so a
                        // message can be attributed to whoever typed it.
                        "mine": false,
                        "kind": kind,
                        "text": text,
                        "entities": entities,
                        "emoji": body["emoji"].as_str().unwrap_or(""),
                        "media": body["media"].as_str()
                            .or_else(|| body["thumb"].as_str())
                            .unwrap_or(""),
                        "media_kind": body["media_kind"].as_str()
                            .unwrap_or(if kind == "sticker" { "image" } else { "" }),
                        "ts": now_hm(),
                        "date": res["date"].as_i64().unwrap_or(now_secs() as i64),
                    });
                    app.push_message(target, msg.clone());
                    return (200, json!({ "ok": true, "message": msg }));
                }
                Some((_, res)) => {
                    return (502, json!({ "error": res["error"].as_str().unwrap_or("send failed") }))
                }
                None => return (503, json!({ "error": "telegram bridge offline" })),
            }
        }
        "/api/chat/react" => {
            let mid = body["mid"].as_i64().unwrap_or(0);
            let emoji = body["emoji"].as_str().unwrap_or("❤️");
            if mid == 0 {
                return (400, json!({ "error": "mid" }));
            }
            let target = app.chat_for(Some(auth));
            let req_body = json!({
                "chat_id": target.to_string(),
                "message_id": mid,
                "emoji": emoji,
            });
            match tg_post("/react", &req_body) {
                Some((code, _)) if code == 200 => {
                    // The queue must be loaded before it can be edited, and the
                    // reaction has to survive a restart: without save_chat the
                    // heart vanished on the next boot.
                    app.ensure_chat(target);
                    let mut changed = false;
                    {
                        let mut map = app.chats.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(m) = map
                            .get_mut(&target)
                            .and_then(|q| q.iter_mut().find(|m| m["mid"].as_i64() == Some(mid)))
                        {
                            m["reacted"] = json!(emoji);
                            changed = true;
                        }
                    }
                    if changed {
                        app.save_chat(target);
                    }
                    return (200, json!({ "ok": true }));
                }
                _ => return (502, json!({ "error": "reaction failed" })),
            }
        }
        "/api/chat/delete" => {
            let mid = body["mid"].as_i64().unwrap_or(0);
            let local_id = body["id"].as_str().unwrap_or("");
            let target = app.chat_for(Some(auth));

            // A local-only bubble (`local:true`, mid 0) never reached Telegram,
            // so it is removed from the mirror directly.
            if mid == 0 {
                if local_id.is_empty() {
                    return (400, json!({ "error": "mid" }));
                }
                app.ensure_chat(target);
                let uid = auth.id().to_string();
                // The client cannot be trusted to decide who may remove a
                // local bubble: check its stored author against the verified id.
                let mine = {
                    let map = app.chats.lock().unwrap_or_else(|e| e.into_inner());
                    map.get(&target)
                        .and_then(|q| q.iter().find(|m| m["id"].as_str() == Some(local_id)))
                        .map(|m| m["from"].as_str() == Some(uid.as_str()))
                };
                if mine.is_none() {
                    return (404, json!({ "error": "not found" }));
                }
                if mine != Some(true) && !app.is_admin_of(auth, target, true) {
                    return (403, json!({ "error": "not allowed" }));
                }
                let removed = {
                    let mut map = app.chats.lock().unwrap_or_else(|e| e.into_inner());
                    map.get_mut(&target)
                        .map(|q| {
                            let before = q.len();
                            q.retain(|m| m["id"].as_str() != Some(local_id));
                            q.len() != before
                        })
                        .unwrap_or(false)
                };
                if removed {
                    app.save_chat(target);
                }
                return (200, json!({ "ok": true }));
            }

            // Only a message that is actually mirrored here can be deleted
            // from here. An arbitrary mid would otherwise send a pointless
            // (and failing) deleteMessage to Telegram for every request.
            let stored = app.chat_msgs_for(target, mid);
            if stored.is_none() {
                return (404, json!({ "error": "not found" }));
            }
            // Permission is decided here, from the verified identity: only the
            // author or an administrator of *this* chat may delete. The client
            // used to decide this alone, which meant any viewer could ask the
            // bot to delete anybody's message.
            let mine = stored
                .as_ref()
                .map(|m| m["from"].as_str() == Some(auth.id().to_string().as_str()))
                .unwrap_or(false);
            if !mine && !app.is_admin_of(auth, target, true) {
                return (403, json!({ "error": "not allowed" }));
            }

            let req_body = json!({
                "chat_id": target.to_string(),
                "message_id": mid,
            });
            match tg_post("/delete", &req_body) {
                Some((code, res)) if code == 200 => {
                    // Drop it from the mirror too, otherwise the next SSE frame
                    // would paint the deleted bubble straight back.
                    app.drop_message(target, mid);
                    return (200, json!({ "ok": true }));
                }
                Some((code, res)) => {
                    // "message to delete not found" means it is already gone
                    // upstream: keeping the bubble would leave a ghost that can
                    // never be removed. Treat it as success and purge locally.
                    let err = res["error"].as_str().unwrap_or("").to_lowercase();
                    if code == 400
                        && (err.contains("not found")
                            || err.contains("message_id_invalid")
                            || err.contains("message can't be deleted"))
                    {
                        app.drop_message(target, mid);
                        return (200, json!({ "ok": true, "already": true }));
                    }
                    return (
                        502,
                        json!({ "error": res["error"].as_str().unwrap_or("delete failed") }),
                    );
                }
                None => return (503, json!({ "error": "telegram bridge offline" })),
            }
        }
        "/api/chat/refresh" => {
            // Re-read the on-disk mirror, then refresh names + roster in the
            // background. The response carries a complete frame so the client
            // can repaint once without waiting for the next SSE tick.
            let target = app.chat_for(Some(auth));
            app.refresh_chat(target);
            return (
                200,
                json!({
                    "ok": true,
                    "chat": app.chat_head(target, Some(auth)),
                    "chats": app.chat_list_warmed(Some(auth)),
                    "messages": app.chat_msgs(target),
                    "state": app.snapshot(true, Some(auth)),
                }),
            );
        }
        "/api/chat/unban" => {
            let target = app.chat_for(Some(auth));
            if !app.is_owner(auth) && !app.is_admin_of(auth, target, true) {
                return (403, json!({ "error": "not allowed" }));
            }
            let id = body["id"].as_str().unwrap_or_default().to_string();
            if id.is_empty() {
                return (400, json!({ "error": "id" }));
            }
            match tg_post(
                "/mod",
                &json!({
                    "chat_id": target.to_string(),
                    "user_id": id,
                    "action": "unban",
                }),
            ) {
                Some((code, _)) if code == 200 => {
                    app.ban_remove(target, &id);
                    app.refresh_roster();
                    return (
                        200,
                        json!({ "ok": true, "banned": app.banned_list(target) }),
                    );
                }
                Some((_, res)) => {
                    return (
                        502,
                        json!({ "error": res["error"].as_str().unwrap_or("unban failed") }),
                    )
                }
                None => return (503, json!({ "error": "telegram bridge offline" })),
            }
        }
        "/api/chat/leave" => {
            let target = app.chat_for(Some(auth));
            if !app.is_owner(auth) && !app.is_admin_of(auth, target, true) {
                return (403, json!({ "error": "not allowed" }));
            }
            let left = matches!(
                tg_post("/leave", &json!({ "chat_id": target.to_string() })),
                Some((code, _)) if code == 200
            );
            // The configured home chat is only cleared (the switcher must keep
            // the room the app is bound to); other groups are fully forgotten.
            app.forget_chat(target);
            return (
                200,
                json!({
                    "ok": true,
                    "left": left,
                    "active": app.chat_for(Some(auth)).to_string(),
                    "chats": app.chat_list(Some(auth)),
                }),
            );
        }
        "/api/chat/clear" => {
            // Wipe the local mirror of one chat. Telegram is untouched: this is
            // the "clear this chat in the app" action, useful when a room is
            // reused and the old conversation is just noise.
            let target = app.chat_for(Some(auth));
            if !app.is_admin_of(auth, target, true) && !app.is_owner(auth) {
                return (403, json!({ "error": "not allowed" }));
            }
            app.clear_chat(target);
            app.mark_read(target, Some(auth));
            return (200, json!({ "ok": true }));
        }
        "/api/chat/forget" => {
            // Delete a chat from the app (not from Telegram). The bot's home
            // chat is only cleared: removing it would drop the room the app is
            // configured for and it would reappear on the next roster pass.
            let id: i64 = body["id"]
                .as_str()
                .and_then(|s| s.trim().parse().ok())
                .or_else(|| body["id"].as_i64())
                .unwrap_or(0);
            if id == 0 {
                return (400, json!({ "error": "id" }));
            }
            if !app.chat_known(id) {
                return (404, json!({ "error": "unknown chat" }));
            }
            if !app.is_admin_of(auth, id, true) {
                return (403, json!({ "error": "not allowed" }));
            }
            let home = id == app.chat_id;
            app.forget_chat(id);
            let next = app.chat_for(Some(auth));
            return (
                200,
                json!({
                    "ok": true,
                    "cleared": home,
                    "active": next.to_string(),
                    "chats": app.chat_list(Some(auth)),
                }),
            );
        }
        "/api/chat/select" => {
            let id: i64 = body["id"]
                .as_str()
                .and_then(|s| s.trim().parse().ok())
                .or_else(|| body["id"].as_i64())
                .unwrap_or(0);
            if id == 0 || !app.chat_known(id) {
                return (404, json!({ "error": "unknown chat" }));
            }
            app.view_chat
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(auth.id(), id);
            app.mark_read(id, Some(auth));
            // A chat picked for the first time may still be titled "Chat -100…".
            if let Some(hub) = crate::webapp::app() {
                thread::spawn(move || hub.resolve_chat_meta(id));
            }
            return (200, json!({ "ok": true, "chat": id.to_string() }));
        }
        "/api/chat/read" => {
            let target = app.chat_for(Some(auth));
            app.mark_read(target, Some(auth));
            return (200, json!({ "ok": true }));
        }
        "/api/stickers/saved" => {
            let me = auth.me(body["me"].as_str().unwrap_or("").to_string());
            if me.is_empty() || me.len() > 64 {
                return (400, json!({ "error": "me" }));
            }
            let action = body["action"].as_str().unwrap_or("add");
            let mut list = saved_stickers_read(&me);
            match action {
                "remove" => {
                    let id = body["id"].as_str().unwrap_or("").to_string();
                    list.retain(|s| s["id"].as_str() != Some(id.as_str()));
                }
                "clear" => list.clear(),
                _ => {
                    let mut s = body["sticker"].clone();
                    if !s.is_object() {
                        return (400, json!({ "error": "sticker" }));
                    }
                    let sid = s["id"].as_str().unwrap_or("").to_string();
                    let sid = if sid.is_empty() {
                        format!("sv{}", now_ms())
                    } else {
                        sid
                    };
                    if let Some(o) = s.as_object_mut() {
                        o.insert("id".to_string(), json!(sid.clone()));
                        o.insert("saved_at".to_string(), json!(now_secs()));
                    }
                    // Re-saving moves a sticker to the top instead of adding a
                    // duplicate; 120 is plenty for a "recently saved" strip.
                    list.retain(|x| x["id"].as_str() != Some(sid.as_str()));
                    list.insert(0, s);
                    list.truncate(120);
                }
            }
            if !saved_stickers_write(&me, &list) {
                return (500, json!({ "error": "could not save stickers" }));
            }
            return (200, json!({ "ok": true, "stickers": list }));
        }
        "/api/members" => {
            let id = body["id"].as_str().unwrap_or_default().to_string();
            let action = body["action"].as_str().unwrap_or("");
            if id.is_empty() {
                return (400, json!({ "error": "id" }));
            }
            // Promote/kick/ban are group-admin powers. Previously any viewer
            // could call the bridge with them; Telegram would usually refuse,
            // but the request itself was never authorised here.
            let target = app.chat_for(Some(auth));
            if !app.is_owner(auth) && !app.is_admin_of(auth, target, true) {
                return (403, json!({ "error": "not allowed" }));
            }
            let req_body = json!({
                "chat_id": target.to_string(),
                "user_id": id,
                "action": action,
            });
            match tg_post("/mod", &req_body) {
                Some((code, res)) if code == 200 => {
                    if action == "ban" {
                        app.ban_add(target, &id, &auth.display_name("admin".to_string()));
                    } else if action == "unban" || action == "kick" {
                        app.ban_remove(target, &id);
                    }
                    app.refresh_roster();
                    return (200, json!({ "ok": true, "result": res }));
                }
                Some((_, res)) => {
                    return (502, json!({ "error": res["error"].as_str().unwrap_or("action failed") }))
                }
                None => return (503, json!({ "error": "telegram bridge offline" })),
            }
        }
        "/api/session/end" => {
            let target = app.chat_for(Some(auth));
            if !app.is_owner(auth) && !app.is_admin_of(auth, target, true) {
                return (403, json!({ "error": "not allowed" }));
            }
            let by = auth.display_name(body["name"].as_str().unwrap_or("mini app").to_string());
            let id: i64 = match auth.user_id {
                Some(id) => id,
                None => body["id"]
                    .as_str()
                    .and_then(|s| s.trim().parse().ok())
                    .or_else(|| body["id"].as_i64())
                    .unwrap_or(0),
            };
            if !end_session(target, id, &by, "app") {
                return (404, json!({ "error": "no active voice chat" }));
            }
            return (200, json!({ "ok": true, "session": app.session_json(false) }));
        }
        "/api/dm/send" => {
            let me = auth.me(body["me"].as_str().unwrap_or_default().to_string());
            let peer = body["peer"].as_str().unwrap_or_default().to_string();
            let text = body["text"].as_str().unwrap_or("").trim().to_string();
            if me.is_empty() || peer.is_empty() || me == peer {
                return (400, json!({ "error": "me, peer" }));
            }
            if text.is_empty() {
                return (400, json!({ "error": "empty" }));
            }
            let text: String = text.chars().take(4000).collect();
            let msg = json!({
                "id": format!("dm{}", now_ms()),
                "a": me,
                "b": peer,
                "from": me,
                "to": peer,
                "text": text,
                "ts": now_hm(),
                "date": now_secs(),
                // Only the two participants ever read this: it is not pushed
                // to the room chat and never sent to Telegram.
                "private": true,
            });
            {
                let mut list = app.dms.lock().unwrap_or_else(|e| e.into_inner());
                list.push(msg.clone());
                let overflow = list.len().saturating_sub(2000);
                if overflow > 0 {
                    list.drain(0..overflow);
                }
            }
            app.save_dms();
            return (200, json!({ "ok": true, "message": msg }));
        }
        "/api/settings" => {
            let mut s = app.settings.lock().unwrap_or_else(|e| e.into_inner());
            if let (Some(dst), Some(src)) = (s.as_object_mut(), body.as_object()) {
                for (k, v) in src {
                    dst.insert(k.clone(), v.clone());
                }
            }
        }
        _ => return (404, json!({ "error": "not found" })),
    }

    (200, app.snapshot(true, Some(auth)))
}

fn stream_events(app: Arc<MiniApp>, mut stream: TcpStream, auth: Auth) -> std::io::Result<()> {
    stream.write_all(
        b"HTTP/1.1 200 OK\r\n\
          Content-Type: text/event-stream\r\n\
          Cache-Control: no-cache\r\n\
          Connection: keep-alive\r\n\
          Access-Control-Allow-Origin: *\r\n\r\n",
    )?;
    stream.flush()?;
    let mut first = true;
    let mut idle = 0u32;
    let mut cur = Cursor::new();
    loop {
        match app.broadcast(&mut stream, first, Some(&auth), &mut cur) {
            Ok(()) => {}
            Err(_) => break,
        }
        first = false;
        idle += 1;
        // A comment frame keeps proxies from dropping an idle connection
        // without waking the client up.
        if idle % 15 == 0 {
            if write!(stream, ": ping\n\n").is_err() || stream.flush().is_err() {
                break;
            }
        }
        thread::sleep(Duration::from_millis(1000));
    }
    Ok(())
}

/* ─────────────────────────── static ─────────────────────────── */

const MIME_CSS: &str = "text/css; charset=utf-8";
const MIME_JS: &str = "text/javascript; charset=utf-8";
const MIME_HTML: &str = "text/html; charset=utf-8";

fn serve_static(app: &MiniApp, stream: &mut TcpStream, path: &str) -> std::io::Result<()> {
    let rel = if path == "/" { "/index.html" } else { path };
    if rel.contains("..") {
        return not_found(stream);
    }
    let file = app.dir.join(rel.trim_start_matches('/'));
    let Ok(bytes) = std::fs::read(&file) else {
        return not_found(stream);
    };
    let mime = match file.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => MIME_HTML,
        "css" => MIME_CSS,
        "js" => MIME_JS,
        "svg" => "image/svg+xml",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    };
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nCache-Control: {cache}\r\nConnection: close\r\n\r\n",
        bytes.len(),
        // The HTML and the auth bootstrap must never be served stale: a cached
        // app.js from before an auth change would leave the app in a 401 loop.
        cache = if mime == MIME_HTML {
            "no-store"
        } else {
            "public, max-age=60"
        },
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(&bytes)?;
    stream.flush()
}

fn not_found(stream: &mut TcpStream) -> std::io::Result<()> {
    let body = b"not found";
    let head = format!(
        "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

/* ─────────────────── helpers for the bot ─────────────────── */

/// One incoming group message, already flattened by the MTProto layer.
#[derive(Clone, Debug)]
pub struct ChatEvent {
    pub chat_id: i64,
    pub mid: i64,
    pub sender_id: i64,
    pub name: String,
    pub text: String,
    /// text | sticker | photo | video | document | service
    pub kind: String,
    /// sticker alt-emoji, empty otherwise
    pub emoji: String,
    /// `/api/tgmedia/<key>` once the bot downloaded the media, else empty
    pub media: String,
    /// image | video | tgs — Telegram stickers are `.webp`, `.webm` or a
    /// gzipped Lottie `.tgs`; the client needs to know which before painting.
    pub media_kind: String,
    pub reply_to: i64,
    pub date: i64,
    pub ts: String,
    pub outgoing: bool,
    pub entities: Vec<Value>,
}

/// Push one message from Telegram into the mini app's Chat tab.
/// A track is about to start: the session is live from this moment on and
/// the counter behind the "TRACKS" tile goes up by one.
pub fn begin_session(_chat_id: i64) {
    if let Some(app) = app() {
        app.mark_live();
        *app.session_tracks.lock().unwrap_or_else(|e| e.into_inner()) += 1;
    }
}

/// End the voice chat: stop the stream, leave the Telegram group call, clear
/// the mirrored chat and record the summary. Returns false when there is no
/// room to end, so callers can keep their own "no active room" fallback.
pub fn end_session(chat_id: i64, by_id: i64, by_name: &str, reason: &str) -> bool {
    let Some(app) = app() else {
        return false;
    };
    let Some(room) = app.rooms.room_for(chat_id) else {
        return false;
    };
    let last_track = {
        let g = room.lock().unwrap_or_else(|e| e.into_inner());
        g.track.as_ref().map(|t| t.title.clone()).unwrap_or_default()
    };
    app.rooms.drop_room(chat_id);
    stop_engine(chat_id);
    app.finish_session(chat_id, by_id, by_name, reason, &last_track, true);
    true
}

/// Stop the assistant's WebRTC stream and leave the group call. Leaving is
/// best effort: if Telegram refuses, the session still ends locally and the
/// next play re-joins as usual.
/// Grant the assistant the "manage voice chats" right through the Bot API.
///
/// Creating a group call needs that admin right; in a brand-new group the
/// assistant is a plain member and `phone.createGroupCall` answers
/// CHAT_ADMIN_REQUIRED. The bot itself is admin (that is how it was added), so
/// it can promote the assistant once and playback works from then on.
pub fn ensure_assistant_admin(chat_id: i64) -> bool {
    let Some(assists) = ASSISTS.get() else {
        return false;
    };
    let Some(a) = assists.first() else {
        return false;
    };
    let uid = {
        let mut ctx = a.ntg();
        ctx.mt_mut()
            .and_then(|mt| mt.self_user().ok())
            .map(|(id, _)| id)
            .unwrap_or(0)
    };
    if uid == 0 {
        return false;
    }
    let body = json!({
        "chat_id": chat_id.to_string(),
        "user_id": uid.to_string(),
        "action": "promote",
    });
    matches!(tg_post("/mod", &body), Some((200, _)))
}

fn stop_engine(chat_id: i64) {
    let Some(assists) = ASSISTS.get().cloned() else {
        return;
    };
    // Kill the encoder/stream synchronously — that part is local and makes
    // playback stop the instant the user taps End.
    if let Some(a) = assists.for_chat(chat_id).or_else(|| assists.first()) {
        let mut ctx = a.ntg();
        ctx.stop(chat_id);
    }
    // `leaveGroupCall` is a network RPC. The MTProto link can be busy with a
    // roster fetch or a download, and blocking /api/session/end on it made
    // the button look dead. Leave in the background instead.
    thread::spawn(move || {
        let Some(a) = assists.for_chat(chat_id).or_else(|| assists.first()) else {
            return;
        };
        let mut ctx = a.ntg();
        if let Some(mt) = ctx.mt_mut() {
            match mt.get_group_call(chat_id) {
                Ok(call) => {
                    if let Err(e) = mt.leave_group_call(&call) {
                        crate::parv_info!("session", "leave group call failed chat={chat_id}: {e}");
                    }
                }
                Err(e) => crate::parv_info!("session", "no group call to leave chat={chat_id}: {e}"),
            }
        }
    });
}

pub fn ingest(ev: ChatEvent) {
    if ev.outgoing {
        return;
    }
    // Service messages (joined, pinned, call started/ended…) arrive with no
    // text and no media. Mirroring them produced empty bubbles authored by
    // "member", so they are dropped here.
    if ev.kind == "text" && ev.text.trim().is_empty() {
        return;
    }
    if let Some(app) = app() {
        app.push_incoming(ev);
    }
}

/// True when the mini app is mirroring `chat_id` (lets the bot skip work).
pub fn tracks_chat(chat_id: i64) -> bool {
    // Every group the bot is in is tracked; each one gets its own chat.
    // Only the zero id (non-group updates) is refused.
    match app() {
        Some(_) => chat_id != 0,
        None => false,
    }
}

/// Register a group as soon as the bot sees *any* update from it. Service
/// messages (bot added, member joined, call started) carry no text and are
/// dropped by `ingest`, so without this a newly joined group stayed invisible
/// in the switcher until somebody typed a real message.
pub fn note_chat(chat_id: i64) {
    if chat_id == 0 {
        return;
    }
    let Some(hub) = app() else {
        return;
    };
    let is_new = !hub
        .chat_meta
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(&chat_id);
    hub.touch_chat(chat_id, None, None);
    // Resolve the real title once per group. Retrying on every message would
    // spawn a bridge call per update whenever Telegram is unreachable.
    if is_new {
        thread::spawn(move || hub.resolve_chat_meta(chat_id));
    }
}

impl MiniApp {
    /// Mirror a chat message the bot posted into the room, so the mini app's
    /// Chat tab stays in sync with Telegram.
    pub fn note_message(&self, author: &str, text: &str) {
        let chat = self.active_chat();
        self.push_message(chat, json!({
            "id": format!("b{}", now_ms()),
            "mid": 0,
            "from": "bot",
            "name": author,
            "photo": "",
            "tags": [{"k": "host", "label": "HOST"}],
            "mine": false,
            "kind": "text",
            "text": text,
            "ts": now_hm(),
            "date": now_secs() as i64,
        }));
    }

    /// Replace the member roster (call after joins/leaves).
    pub fn set_members(&self, members: Vec<Value>) {
        *self.members.lock().unwrap_or_else(|e| e.into_inner()) = members;
    }

    /// Rotating quote shown above the artwork on the Song tab.
    pub fn set_quote(&self, quote: &str) {
        *self.quote.lock().unwrap_or_else(|e| e.into_inner()) = quote.to_string();
    }

    /// Force a roster refresh on the next `/api/state`.
    pub fn invalidate_roster(&self) {
        *self.roster_at.lock().unwrap_or_else(|e| e.into_inner()) = 0;
    }
}

#[cfg(test)]
mod auth_tests {
    use super::*;

    const TOKEN: &str = "123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11";
    // Cross-checked against an independent Python HMAC-SHA256 implementation.
    const HASH: &str = "6083fcffa5274993fead6c2e2e738dbfd50855b262eaae13bb4f6dad7d646b27";
    const RAW: &str = "auth_date=1730000000&query_id=AAHdF6IQAAAAAN0XohDhrOrc&user=%7B%22id%22%3A42%2C%22first_name%22%3A%22Test%22%2C%22last_name%22%3A%22User%22%2C%22username%22%3A%22testuser%22%2C%22language_code%22%3A%22en%22%7D&hash=6083fcffa5274993fead6c2e2e738dbfd50855b262eaae13bb4f6dad7d646b27";

    #[test]
    fn hmac_matches_reference_vector() {
        let fields = vec![
            ("auth_date".to_string(), "1730000000".to_string()),
            ("query_id".to_string(), "AAHdF6IQAAAAAN0XohDhrOrc".to_string()),
            (
                "user".to_string(),
                r#"{"id":42,"first_name":"Test","last_name":"User","username":"testuser","language_code":"en"}"#
                    .to_string(),
            ),
        ];
        assert_eq!(tg_init_data_hash(TOKEN, &fields).as_deref(), Some(HASH));
    }

    #[test]
    fn valid_init_data_yields_the_verified_user() {
        let (id, name, username) = verify_tg_init_data(TOKEN, u64::MAX, RAW).expect("valid");
        assert_eq!(id, 42);
        assert_eq!(name, "Test User");
        assert_eq!(username, "testuser");
    }

    #[test]
    fn tampered_payload_is_rejected() {
        // Same hash, different user id: the HMAC no longer matches.
        let forged = RAW.replace("%22id%22%3A42", "%22id%22%3A43");
        assert!(verify_tg_init_data(TOKEN, u64::MAX, &forged).is_none());
    }

    #[test]
    fn wrong_bot_token_is_rejected() {
        assert!(verify_tg_init_data("999:wrong", u64::MAX, RAW).is_none());
    }

    #[test]
    fn stale_init_data_is_rejected() {
        // auth_date is 2024-11-01, so a one-hour window must refuse it.
        assert!(verify_tg_init_data(TOKEN, 3600, RAW).is_none());
    }

    #[test]
    fn missing_init_data_is_rejected() {
        assert!(verify_tg_init_data(TOKEN, u64::MAX, "").is_none());
    }
}

#[cfg(test)]
mod chat_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);
    const HOME: i64 = -1001;
    const OTHER: i64 = -1002;

    fn unique_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "parvati-chat-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, AtomicOrdering::Relaxed)
        ))
    }

    fn app_in(chat_id: i64, cache_dir: PathBuf) -> MiniApp {
        MiniApp {
            rooms: Rooms::new(),
            chat_id,
            title: Mutex::new("Home group".to_string()),
            dir: PathBuf::from("miniapp"),
            chats: Mutex::new(HashMap::new()),
            chat_meta: Mutex::new(HashMap::new()),
            chats_loaded: Mutex::new(std::collections::HashSet::new()),
            view_chat: Mutex::new(HashMap::new()),
            admin_cache: Mutex::new(HashMap::new()),
            last_read: Mutex::new(HashMap::new()),
            banned: Mutex::new(HashMap::new()),
            banned_loaded: Mutex::new(std::collections::HashSet::new()),
            cache_dir,
            members: Mutex::new(Vec::new()),
            seen: Mutex::new(Vec::new()),
            member_count: Mutex::new(0),
            invite: Mutex::new(String::new()),
            settings: Mutex::new(json!({})),
            quote: Mutex::new(String::new()),
            live: Mutex::new(false),
            session_started: Mutex::new(0),
            session_tracks: Mutex::new(0),
            last_session: Mutex::new(json!({})),
            media: Mutex::new(HashMap::new()),
            roster_at: Mutex::new(0),
            search_cache: Mutex::new(Vec::new()),
            dms: Mutex::new(Vec::new()),
            pending: Mutex::new(Value::Null),
            play_error: Mutex::new(Value::Null),
            vc_known: Mutex::new(false),
            recent: Mutex::new(Vec::new()),
            favs: Mutex::new(Vec::new()),
            sleep_at: Mutex::new(0),
            roster_busy: AtomicBool::new(false),
            token: String::new(),
            bot_token: String::new(),
            allowed_ids: Vec::new(),
            auth_max_age: 86_400,
            require_member: false,
            allow_open: false,
        }
    }

    fn app() -> (MiniApp, PathBuf) {
        let dir = unique_dir();
        (app_in(HOME, dir.clone()), dir)
    }

    fn viewer(id: i64) -> Auth {
        Auth {
            ok: true,
            user_id: Some(id),
            name: format!("User {id}"),
            username: String::new(),
            via: "initdata",
        }
    }

    fn msg(mid: i64, from: &str, text: &str, date: i64) -> Value {
        json!({
            "id": format!("m{mid}"), "mid": mid, "from": from,
            "name": from, "text": text, "kind": "text", "date": date,
        })
    }

    fn event(chat: i64, mid: i64, sender: i64, text: &str, kind: &str) -> ChatEvent {
        ChatEvent {
            chat_id: chat,
            mid,
            sender_id: sender,
            name: if sender == 0 {
                String::new()
            } else {
                format!("User {sender}")
            },
            text: text.to_string(),
            kind: kind.to_string(),
            emoji: String::new(),
            media: String::new(),
            media_kind: String::new(),
            reply_to: 0,
            date: 100,
            ts: "10:00".to_string(),
            outgoing: false,
            entities: Vec::new(),
        }
    }

    #[test]
    fn messages_are_isolated_per_chat() {
        let (app, dir) = app();
        app.touch_chat(OTHER, Some("Second group"), None);
        app.push_message(HOME, msg(1, "11", "hello A", 10));
        app.push_message(OTHER, msg(2, "22", "hello B", 20));
        assert_eq!(app.chat_msgs(HOME).len(), 1);
        assert_eq!(app.chat_msgs(OTHER)[0]["text"], "hello B");
        assert!(app.drop_message(HOME, 1));
        assert!(app.chat_msgs(HOME).is_empty());
        assert_eq!(app.chat_msgs(OTHER).len(), 1);
        // Deleting an id that belongs to the other group must not touch this one.
        assert!(!app.drop_message(HOME, 2));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn deleting_a_message_unlinks_replies() {
        let (app, dir) = app();
        app.push_message(HOME, msg(1, "11", "original", 10));
        app.push_message(HOME, json!({
            "id": "m2", "mid": 2, "from": "22", "name": "22",
            "text": "re", "kind": "text", "date": 20,
            "reply": { "mid": 1, "name": "11", "text": "original" },
        }));
        assert!(app.drop_message(HOME, 1));
        let rest = app.chat_msgs(HOME);
        assert_eq!(rest.len(), 1);
        assert!(rest[0].get("reply").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn clear_and_forget_leave_other_chats_alone() {
        let (app, dir) = app();
        app.touch_chat(OTHER, Some("Second group"), None);
        app.push_message(HOME, msg(1, "11", "a", 10));
        app.push_message(OTHER, msg(2, "22", "b", 20));
        app.clear_chat(HOME);
        assert!(app.chat_msgs(HOME).is_empty());
        assert_eq!(app.chat_msgs(OTHER).len(), 1);
        app.forget_chat(OTHER);
        assert!(app.chat_msgs(OTHER).is_empty());
        assert!(!app.chat_known(OTHER));
        assert!(app.chat_known(HOME));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn home_chat_is_cleared_but_never_forgotten() {
        let (app, dir) = app();
        app.push_message(HOME, msg(1, "11", "a", 10));
        app.forget_chat(HOME);
        assert!(app.chat_known(HOME));
        assert_eq!(app.chat_for(None), HOME);
        assert!(app.chat_msgs(HOME).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn switcher_is_per_viewer_and_active_chat_is_first() {
        let (app, dir) = app();
        let a = viewer(7);
        let b = viewer(8);
        app.touch_chat(OTHER, Some("Second group"), None);
        app.note_chat_activity(HOME, "newer", "12:00", 200);
        app.note_chat_activity(OTHER, "older", "11:00", 100);
        app.view_chat
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(7, OTHER);
        assert_eq!(app.chat_for(Some(&a)), OTHER);
        assert_eq!(app.chat_for(Some(&b)), HOME);
        let list = app.chat_list(Some(&a));
        assert_eq!(list[0]["id"], OTHER.to_string());
        assert_eq!(list[0]["active"], true);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unread_stamps_are_per_chat_and_per_viewer() {
        let (app, dir) = app();
        let a = viewer(7);
        app.touch_chat(OTHER, Some("Second group"), None);
        app.push_message(HOME, msg(1, "99", "a", 10));
        app.push_message(OTHER, msg(2, "99", "b", 20));
        assert_eq!(app.unread_for(HOME, Some(&a)), 1);
        assert_eq!(app.unread_for(OTHER, Some(&a)), 1);
        app.mark_read(HOME, Some(&a));
        assert_eq!(app.unread_for(HOME, Some(&a)), 0);
        assert_eq!(app.unread_for(OTHER, Some(&a)), 1);
        // Your own messages never count as unread.
        app.push_message(HOME, msg(3, "7", "mine", 30));
        assert_eq!(app.unread_for(HOME, Some(&a)), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn each_chat_persists_to_its_own_file() {
        let dir = unique_dir();
        let first = app_in(HOME, dir.clone());
        first.touch_chat(OTHER, Some("Second group"), None);
        first.push_message(HOME, msg(1, "11", "a", 10));
        first.push_message(OTHER, msg(2, "22", "b", 20));
        assert!(dir.join("mirror--1001.json").exists());
        assert!(dir.join("mirror--1002.json").exists());
        // Cold boot: registry first, then each queue on first touch.
        let second = app_in(HOME, dir.clone());
        second.load_chats();
        assert_eq!(second.chat_msgs(HOME)[0]["text"], "a");
        assert_eq!(second.chat_msgs(OTHER)[0]["text"], "b");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn chat_history_is_capped() {
        let (app, dir) = app();
        for i in 1..=(CHAT_CAP as i64 + 5) {
            app.push_message(HOME, msg(i, "11", "x", i));
        }
        let list = app.chat_msgs(HOME);
        assert_eq!(list.len(), CHAT_CAP);
        assert_eq!(list[0]["mid"], 6);
        assert_eq!(list[CHAT_CAP - 1]["mid"], (CHAT_CAP + 5) as i64);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_new_group_gets_its_own_chat_on_the_first_message() {
        let (app, dir) = app();
        app.push_message(HOME, msg(1, "11", "home only", 10));

        // A group the app has never seen before sends its first message.
        app.push_incoming(event(OTHER, 900, 22, "hello from the new group", "text"));

        let v = viewer(22);
        let list = app.chat_list(Some(&v));
        assert!(
            list.iter()
                .any(|r| r["id"].as_str() == Some(OTHER.to_string().as_str())),
            "the new group must appear in the switcher"
        );
        assert_eq!(app.chat_msgs(HOME).len(), 1);
        assert_eq!(app.chat_msgs(HOME)[0]["text"], "home only");
        assert_eq!(app.chat_msgs(OTHER).len(), 1);
        assert_eq!(app.chat_msgs(OTHER)[0]["text"], "hello from the new group");

        // Later messages stay in their own timelines, never in one shared feed.
        app.push_incoming(event(HOME, 901, 11, "back home", "text"));
        assert_eq!(app.chat_msgs(HOME).len(), 2);
        assert_eq!(app.chat_msgs(OTHER).len(), 1);
        assert_eq!(app.chat_msgs(OTHER)[0]["text"], "hello from the new group");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_service_update_registers_the_group_without_an_empty_bubble() {
        let (app, dir) = app();
        // "bot was added to the group" has no author and no text. It must make
        // the group visible in the switcher, but must not paint an empty bubble.
        app.push_incoming(event(OTHER, 0, 0, "", "service"));

        assert!(app.chat_known(OTHER), "the group must be registered");
        let list = app.chat_list(None);
        assert!(
            list.iter()
                .any(|r| r["id"].as_str() == Some(OTHER.to_string().as_str())),
            "the group must be listed before any real message"
        );
        assert!(
            app.chat_msgs(OTHER).is_empty(),
            "a textless service update must not create a message"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn refresh_chat_is_idempotent_and_keeps_disk_authoritative() {
        let (app, dir) = app();
        app.push_message(HOME, msg(1, "11", "one", 10));
        app.push_message(HOME, msg(2, "11", "two", 20));
        app.push_message(HOME, msg(3, "11", "three", 30));
        assert_eq!(app.chat_msgs(HOME).len(), 3);

        // "Refresh chat" re-reads the on-disk mirror. An append-based reload
        // used to duplicate every bubble each time the button was pressed.
        app.refresh_chat(HOME);
        app.refresh_chat(HOME);
        let after = app.chat_msgs(HOME);
        assert_eq!(after.len(), 3, "refresh must not duplicate messages");
        assert_eq!(after[0]["text"], "one");
        assert_eq!(after[2]["text"], "three");

        // The loaded flag survives a refresh, so a message arriving right
        // afterwards is not undone by a later ensure_chat() re-read.
        app.push_message(HOME, msg(4, "11", "four", 40));
        assert_eq!(app.chat_msgs(HOME).len(), 4, "new messages survive refresh");

        // When the mirror is gone, refresh drops the in-memory copy: disk is
        // authoritative, so a stale bubble must not be resurrected.
        let _ = std::fs::remove_file(app.chat_file(HOME));
        app.refresh_chat(HOME);
        assert!(
            app.chat_msgs(HOME).is_empty(),
            "a missing mirror must clear the in-memory copy"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
