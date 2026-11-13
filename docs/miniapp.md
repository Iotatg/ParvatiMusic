# Mini App — integration notes

The mini app is a static web bundle (`miniapp/`) plus a bridge module
(`bot/src/webapp.rs`). The bridge reads the **same** `Rooms` map the chat
commands use, so Telegram and the app never disagree.

```
        ┌──────────────────────────── Telegram ────────────────────────────┐
        │  voice chat + /play commands            Mini App webview         │
        └───────────────┬──────────────────────────────┬───────────────────┘
                        │ MTProto                      │ HTTPS  /  SSE
                ┌───────▼────────┐              ┌──────▼─────────┐
                │  modules/*     │              │  webapp.rs     │
                │  play, skip…   │              │  HTTP + JSON   │
                └───────┬────────┘              └──────┬─────────┘
                        │        Rooms (Arc<Mutex<…>>) │
                        └───────────┬──────────────────┘
                                    │
                          ┌─────────▼──────────┐
                          │  RoomState         │──► monitor::engine_*
                          │  track, queue, …   │      (ntgcalls)
                          └────────────────────┘
```

## Wiring

`bot/src/lib.rs`

```rust
pub mod webapp;
```

`bot/src/main.rs`, right after `modules::init(...)` so `modules::assistants()`
is populated for engine calls:

```rust
// Bot API sidecar: names, avatars, admins, sticker packs, premium emoji.
parvati_bot::webapp::set_bot_id(tg.me_id());
// Assistant sessions: the full member list (Bot API only lists admins).
parvati_bot::webapp::install_assistants(assists.clone());
parvati_bot::webapp::install(rooms.clone());
```

`.env`

```ini
MINIAPP_PORT=8080
MINIAPP_CHAT_ID=-1001234567890
MINIAPP_ROOM_TITLE=chingu gang
MINIAPP_ME_NAME=Iota
MINIAPP_ME_USERNAME=iota
MINIAPP_DIR=miniapp
# optional shared secret for trusted integrations (control plane only)
MINIAPP_TOKEN=
# require a real Telegram identity (default on). The server verifies the
# WebApp initData HMAC with TOKEN; a bare URL is refused with 401.
MINIAPP_REQUIRE_MEMBER=1
# optional allow-list, comma separated Telegram user ids
MINIAPP_ALLOWED_IDS=
# how long a signed initData is accepted, seconds
MINIAPP_AUTH_MAX_AGE=86400
# dev only: serve /api/* without Telegram (never enable in production)
MINIAPP_ALLOW_OPEN=0
```

`MINIAPP_PORT` must differ from `PORT` (the health server binds that one).

## Authentication (who is allowed in)

The mini app is a Telegram Web App, so every request carries `initData` —
signed by Telegram with this bot's token. The server verifies it on **every**
`/api/*` call:

1. `secret = HMAC_SHA256(key = "WebAppData", msg = bot_token)`
2. `hash   = HMAC_SHA256(key = secret, msg = sorted "key=value" lines)`
3. compare in constant time, then require a fresh `auth_date`
   (`MINIAPP_AUTH_MAX_AGE`, default 24 h).

The verified `user.id` becomes the request identity: the `me` field the client
sends is ignored, so nobody can impersonate another member. With
`MINIAPP_REQUIRE_MEMBER=1` the user must also belong to the mirrored chat
(roster first, then one `getChatMember` call cached for 5 min).

A plain browser that opens the tunnel URL has no `initData` and gets
`401 {"error":"unauthorized","reason":"none","hint":"Open this app from Telegram."}`.
The page shows a verification screen instead of the room.

The credential travels three ways so header-less requests work too: the
`X-Telegram-Init-Data` header (fetch), `?init=` (EventSource, `<video>`), and
the `parvati_auth` cookie the page sets once (covers `<img>`, `<video>`, SSE).

## Identity: roster, avatars, roles

Two sources, merged on a 45 s refresh (and after any moderation action):

| Data | Source | Why |
| --- | --- | --- |
| names, @username, status, custom titles | Bot API `getChatMember` / `getChatAdministrators` | exact, and works for any user the bot can see |
| **every** member id | assistant session, `channels.getParticipants` | the Bot API has no "list members" method |
| avatars | Bot API `getUserProfilePhotos` → `/api/avatar/<uid>` | cached in memory + `cache/tg` |
| stickers, premium emoji | `getStickerSet` / `getCustomEmojiStickers` → `/api/tgimg`, `/api/emoji/<id>` | real packs, not a local emoji font |

Roles map to coloured tags in the UI: `OWNER` (creator, gold), `HOST` (the
bot/assistant that sits in the call, violet), `ADMIN` (red), custom title
(teal), `MEMBER` (grey). `HOST` is deliberately *not* "any bot in the group" —
it is the account that actually streams.

The assistant side needs the session to be a member of the group; if it is not,
the roster silently degrades to admins + everyone who has spoken.

## Chat mirroring

`ferogram_bot::mirror_to_miniapp` feeds every group message into the hub:
text with entities (bold/italic/underline/strike/code/pre/spoiler/blockquote/
link/mention/hashtag/custom emoji), replies, photos, voice notes and stickers.
Media is fetched over MTProto on a background task and served from
`/api/tgmedia/<key>`.

### One chat per group

The hub keeps **one message queue per Telegram group**
(`Mutex<HashMap<i64, VecDeque<Value>>>`). It used to keep a single global
queue, which mixed every group into one timeline and persisted whichever chat
happened to be "playing" at save time — a second group therefore looked like it
had no chat at all, and switching rooms showed somebody else's messages.

| Piece | Stored at | Notes |
| --- | --- | --- |
| Message queue | `cache/mirror-<chat_id>.json` | One file per group, capped at 300 messages, written atomically (tmp + rename) |
| Chat registry | `cache/chats.json` | id, title, member count, last-message preview, activity stamp; capped at 40 rows |
| Viewing position | in memory, `user id → chat id` | Each viewer switches chats independently |
| Read stamps | in memory, `uid:chat → date` | Unread badges are per viewer *and* per chat |

* `note_chat(chat_id)` registers a group on the **first update of any kind**,
  including textless service messages (bot added, member joined). A newly
  joined group therefore appears in the switcher before anyone types. Its real
  title is resolved from Telegram once, on a background thread.
* `push_incoming` writes to `ev.chat_id`, so a message can never be mirrored
  into the wrong group. Reply previews are resolved against that group's own
  queue only.
* `/api/chat/send`, `/react`, `/delete`, `/clear` and `/read` all operate on
  the chat the **verified viewer** selected — never on the room that happens
  to be playing. The client cannot pass a `chat` id to override this.
* SSE frames carry a `chat` id; the app discards a frame for a chat it just
  left, so a late message cannot be painted into the new one.
* Ending a voice chat clears **only that group's** mirror. Other groups keep
  their history.

The switcher (`chatSwitch` → **Group chats** sheet) lists every known group
with its live flag, a last-message preview and an unread badge; the selected
chat is pinned to the top. Tapping a row switches instantly. Each row can be
deleted (removed from the app, not from Telegram) and the current chat can be
cleared. Both actions are admin/owner-only and confirmed with
`WebApp.showPopup`.

Two things made messages silently disappear and are now handled:

* **The update stream ends.** A network blip or an unappliable `getDifference`
  closes `stream_updates()`; the pump used to be a single `while let`, so every
  message after that point — including everything typed inside the voice chat —
  never reached the app. It now re-subscribes in a loop and logs
  `update stream ended, resubscribing in 3s`.
* **Service messages.** Join/pin/call notices arrive with no text and no media.
  They used to render as empty bubbles authored by "member"; `ingest` drops
  them (and `load_chat` drops the ones already persisted).

Sending from the app goes through the Bot API (`/api/chat/send`), so the message
appears in the real group with the bot as the author; `local: true` stores it for
the running instance only (demos, screenshots).

`entities[].custom_emoji_id` is forwarded as a real `custom_emoji` entity, which
is what makes premium emoji render in Telegram rather than as a plain glyph.

### Stickers

Telegram has three sticker formats and they are **not** interchangeable:

| Format | Magic | Renderer |
| --- | --- | --- |
| static `.webp` | `RIFF….WEBP` | `<img>` |
| video `.webm` | `1A 45 DF A3` | `<video autoplay loop muted playsinline>` |
| animated `.tgs` | gzip `1F 8B` | gunzip in the browser (`DecompressionStream`) → **lottie-web** |

`mirror_to_miniapp` sniffs the downloaded bytes and stores `media_kind` on the
message (`image` / `video` / `tgs`), and `/api/tgmedia/<key>` answers with the
matching MIME type. An `<img>` pointed at a `.tgs` or `.webm` is exactly why
animated stickers used to show up blank. The Lottie player is vendored at
`miniapp/assets/vendor/lottie_light.min.js`, so nothing is fetched from a CDN
at runtime; if decoding ever fails the bubble falls back to the sticker's emoji.

The picker reads real packs through `getStickerSet` (`/api/stickers?set=`) and
premium emoji through `getCustomEmojiStickers` (`/api/emoji/<id>`), using the
`thumbnail` the Bot API returns (WebP for all three formats).

## Social & chat interactions

The chat now behaves like Telegram's: tapping a person shows a card, stickers
can be saved, and every message has a reply / react / copy / forward / delete
menu. All of it is additive — no existing playback or mirroring path changed.

| Interaction | Where | Behaviour |
| --- | --- | --- |
| Tap an avatar / name / member row | chat bubble, member list, `•••` menu | `GET /api/user` profile card |
| Long-press a bubble, or tap a reaction chip | chat | message action sheet |
| Swipe a bubble right | chat | reply (same bar as tap-to-reply) |
| Tap a sticker or its small `+` | chat | sticker sheet → **Add to Stickers** |
| Saved tab in the picker | sticker panel | per-viewer saved stickers, tap to resend, long-press to remove |
| Quick actions on the card | profile sheet | Message privately / Mention / Open in Telegram |

The action sheet has a reaction bar (`❤️ 👍 🔥 🥰 😂 😮 😢 🎉`) and a grid of
Reply, Copy text, Forward, Add to Stickers, React, View profile and Delete.
Delete uses `WebApp.showPopup` for confirmation when available, calls
`/api/chat/delete` for the **current chat**, and removes the row from that
chat's persisted mirror so the next SSE frame cannot paint it back. The server
makes the permission decision from the signed identity (author, admin of that
chat, or deployment owner) — the client's own `mine` flag is only a UI hint.
A local-only bubble (`local: true`, `mid: 0`) never reached Telegram, so it is
purged from the mirror directly with the same author/admin check. Reactions are
persisted too, so a heart survives a restart.

Admin answers for groups other than the playing room are cached for 120 s
(`/api/chats` refreshes them when the switcher opens); the 1.5 s state stream
only reads the cache. That keeps the delete buttons stable without putting a
Telegram bridge call on every tick.

**Gesture arbitration.** Tap-to-reply fires after 350 ms, double-tap-to-❤️
inside 340 ms, long-press after 470 ms and swipe after 12 px of horizontal
travel (reply at 54 px). A long-press or a swipe sets `suppressUntil` so the
`click` that follows it cannot also fire the plain tap action. Vertical moves
are handed back to the browser (`touch-action: pan-y`); native image dragging
is disabled on chat media because an HTML5 drag fires `pointercancel` and used
to silently kill the swipe on sticker and photo bubbles. Avatars, names and
quoted replies stay ordinary tap targets and are excluded from the gesture
handler.

**New HTTP endpoints** (all under the existing `/api/*` token check):

| Endpoint | Purpose |
| --- | --- |
| `GET /api/user?id=` | profile card: roster role/tags → `getChat` public fields (name, username, bio, premium, verified) → local message count / last stamp / in-voice flag |
| `GET /api/avatar/<uid>?s=32..320` | profile photo at a requested size (default 96) |
| `GET /api/stickers/saved?me=` | the viewer's saved stickers |
| `POST /api/stickers/saved` | `{me, action: add\|remove\|clear, sticker\|id}` — per-viewer `cache/stickers-<me>.json`, deduped, capped at 120 |
| `POST /api/chat/delete` | `{mid, id?}` — `deleteMessage` + drop from that chat's mirror; `mid: 0` removes a local bubble |
| `GET /api/chats` | chat list with unread counts and delete permissions (also refreshes group titles) |
| `POST /api/chat/select` | `{id}` — switch the viewer to another group chat |
| `POST /api/chat/forget` | `{id}` — delete/clear a chat in the app, Telegram untouched |
| `POST /api/chat/clear` | wipe the current chat's mirrored history (admin only) |
| `POST /api/chat/read` | mark the current chat read for this viewer |
| `POST /api/chat/send` | now also accepts `mediaKey` for stickers; posts to the selected chat |

The Bot API sidecar (`tools/tgapi.py`) gained `GET /user` (`getChat` for a
private-chat object plus `getUserProfilePhotos`) and multipart `sendSticker`.

**Why `mediaKey` exists.** A sticker mirrored over MTProto carries a document
id, not a Bot API `file_id`; the bot caches its bytes at `cache/tg/<key>.bin`
and the bubble loads them from `/api/tgmedia/<key>`. `sendSticker` cannot accept
that document id, so a saved mirrored sticker is re-sent by uploading the exact
cached bytes with `multipart/form-data` (`send_cached_sticker` sniffs
`.webp` / `.tgs` / `.webm` from the magic bytes). Telegram issues a real
`file_id` in the reply, so the same sticker works from then on. The frontend
keeps both `file_id` and `mediaKey` in each saved record, which is why pack
stickers and mirrored chat stickers are equally resendable.

Forward sends a private copy through `/api/dm/send` — it is never posted to the
group, matching the "Forward to…" sheet's promise.

## Share, invite and private chat

Every snapshot's `room` block carries links built from the bot's @username, so
the app never falls back to a bare `t.me/+…` group invite:

```json
"room": {
  "bot": "meow_clonerbot",
  "app_link":  "https://t.me/meow_clonerbot?startapp=p-100123_6ea64bf3c4c0",
  "share":     "https://t.me/share/url?url=…&text=Join%20my%20music%20room",
  "add_to_group": "https://t.me/meow_clonerbot?startgroup=true&admin=invite_users",
  "invite": "https://t.me/+NiXi7OASFFMzYmZl",
  "invite_share": "https://t.me/share/url?url=https%3A%2F%2Ft.me%2F%2BNiXi…&text=Join%20Hi%20on%20Telegram"
}
```

* **Invite & share** (members header, settings, and the paper-plane button on
  every member) opens a sheet with the room link, a Telegram share sheet, the
  "add bot to a group" link and the group invite.
* `t.me/share/url` is opened with `Telegram.WebApp.openTelegramLink`, so the
  native share dialog appears and the recipient lands inside this bot's mini
  app with `startapp=p<chat>_<token>`.

**Private chat** is a separate store (`cache/dm-<chat>.json`, capped at 2000
rows). A row is `{a, b, from, to, text, ts}`; a conversation is visible only to
the two ids in `a`/`b`, never pushed to the room chat and never sent to
Telegram. The lock button in the Chat header opens the thread list, the `+`
picks any member, and "Message privately" in a member's menu jumps straight
into a thread. Both participants see the same messages when they open the app
with their own id.

### Room chat vs. group chat

The Chat tab has a two-way switch:

* **In voice** (default while live) — only messages from members whose `vc`
  flag is set, plus your own. This keeps the room chat about the call.
* **Everyone** — the full mirrored group chat.

"Who is in the call" comes from Telegram, not from the member list: the roster
pass calls `phone.getGroupParticipants` and the snapshot exposes the ids as
`room.vc_ids` (plus `room.vc_known`, which is false when the RPC failed and the
app must not filter at all). The `vc` flag on each member row is derived from
the same set.

Filtering must never look like data loss, so when messages are hidden the chat
shows a banner — `3 messages from members outside the call · Show all` — and
tapping it switches to Everyone. Your own messages are always visible.

The Members tab always shows both groups: `In voice chat` first, then
`Group members` with an invite button per row, so any member can pull anyone
else into the room.

The Chat tab is persisted: every mirrored message is written to
`cache/mirror-<chat_id>.json` (atomic tmp + rename, capped at 300 rows) and
loaded again by `webapp::install`, so a restart does not wipe the room. The file
is deleted when a session ends and clears the chat. A bot token cannot call
`messages.getHistory`, so history is not re-fetched from Telegram — what the app
showed before the restart is exactly what it shows after.

## How actions travel

| Action | Path |
| --- | --- |
| pause / resume | `RoomState::pause/resume` + `monitor::engine_pause/resume` |
| seek | `RoomState::seek(delta)` + `monitor::engine_restart` (ffmpeg `-ss`) |
| loop | `RoomState::set_loop` |
| mute | `RoomState::mute/unmute` + `engine_mute/unmute` |
| queue add/remove/clear | `RoomState::queue` / `remove_at` / `clear_queue` |
| play | `registry::resolve_query` → `registry::download_track` (same video id only; a failed clip reports an error, never a different song and never a cover) → `RoomState::play` → `monitor::start_track` |
| skip | `RoomState::next_track` → download → `monitor::start_track` |
| end / stop | `webapp::end_session` → `rooms.drop_room` → `Context::stop` → `phone.leaveGroupCall` |

### Pick a song: audio or video

Every row that can start playback — search results, queue rows, recently
played — opens the same sheet: **Play audio** (192 kbps MP3, quickest) or
**Play video** (real 720p mp4, picture in the call and on screen). The row is selected by its index in
the rendered list, and the id travels with it, so tapping the 7th result plays
the 7th result; nothing falls back to "first hit".

`POST /api/play {id, kind}`:

| `kind` | download | in the call | in the app |
| --- | --- | --- | --- |
| `audio` | `140/bestaudio` → 192 kbps MP3 | sound | cover art |
| `video` | 720p H.264 DASH + m4a audio, merged to mp4 | sound + 1280×720 @ 30fps picture | real `<video>` over the cover |

**There is no cover-video fallback.** A video pick is always the actual music
video: the downloader prefers `bestvideo[height<=720][ext=mp4][vcodec^=avc1]`
merged with `bestaudio[ext=m4a]`, falls back to any 720p merge, then a
progressive mp4, and only then the 360p `18`. H.264/AAC is deliberate — AV1 or
VP9 in mp4 will not play in iOS Safari, which is what Telegram mini apps use.
The same file feeds both the voice engine and the browser: `media_from_file`
scales it to `1280:720`, pads to the exact frame and emits `fps=30` rawvideo,
while `VideoDescription { width: 1280, height: 720, fps: 30 }` tells NTG what
the call is receiving. If every client in the chain fails, the play reports the
error instead of quietly turning into audio or a slideshow.

Some music videos are gated by YouTube itself on datacenter IPs (the media URL
answers `403` for every player client even though the metadata resolves). The
downloader makes two freshly-extracted passes with a three-client ladder and
`--extractor-retries 3`, which clears the transient cases. For a persistently
gated video, export a **logged-in** cookie jar to `tools/cookies.txt` (that
file is gitignored); an anonymous jar is not enough for those. There is no
cover-image substitute — the UI shows the play error for that exact track.

`GET /api/media?id=<id>&kind=video|audio` streams the exact file the voice
engine is using, with `Range` support (Safari refuses to play a `<video>`
without it). The element is muted on purpose: the sound comes from the voice
chat, so playing it twice would echo. `syncVideo()` nudges `currentTime` back
onto the room clock when drift passes 1.5 s, and a 404 (the mp4 is written
after the DASH merge) is retried quietly up to ten times.

Downloads run on a detached thread: a YouTube fetch takes seconds and must
never block the HTTP accept loop or hold the room lock. The UI follows along on
`/api/events`, which pushes a snapshot every 1.5 s.

Search hits are cached server-side (300 ids). The UI plays and queues **by id**,
and a search result is not in the room yet — without that cache `/api/play`
and `/api/queue/add` would silently do nothing for anything picked from
Search. Both endpoints also create the room when it does not exist yet, and
queue-add is idempotent per track id.

`/api/session/end` never waits on Telegram: `Context::stop` runs inline, while
`phone.leaveGroupCall` is spawned on a thread. A busy MTProto connection (roster
fetch, download) used to make the End button hang for minutes.

`set_play_hook` / `set_skip_hook` exist as overrides if a deployment wants to
route these through the bot's own command handlers instead (for example to get
the same status card edits in chat).

## Session lifecycle

A **session** is one voice chat: from the first successful `play` to `/end`.
The hub keeps three fields for it — `live`, `session_started` and
`session_tracks` — and derives everything else from the room.

```
/play ──► begin_session()  live=true, started=now, tracks+=1
           │
           ├── skip / next ──► tracks+=1 (clock untouched)
           │
/end  ──► end_session(chat, by, name, reason)
           ├── rooms.drop_room(chat)        // state + flags
           ├── Context::stop(chat)          // WebRTC stream off (instant)
           ├── phone.leaveGroupCall(call)   // leaves the call, on a thread
           ├── messages.clear()             // chat wiped
           ├── cache/mirror-<chat>.json     // …and its on-disk copy deleted
           ├── members[*].vc = false        // everyone out of the call
           └── last_session = {…}           // who / when / duration / tracks
```

`/stop`, the stop button on the player card and `POST /api/session/end` all go
through `end_session`, so the app and Telegram can never drift apart. The
assistant leaving is what actually ends the voice chat for the group: when it
was the last participant Telegram closes the call.

When the queue runs out on its own the room disappears without `end_session`;
the next `/api/state` notices `live && room == none` after a 5 s grace window
and closes the session with `reason: "finished"` — chat is kept in that case,
because nothing was explicitly ended. `reason` is `end` for the command,
`app` for the in-app button, `finished` for a natural end.

The snapshot carries the summary so a freshly opened app still renders it:

```json
"session": {
  "live": false, "started_at": 1794374370, "duration": 52,
  "ended_at": 1794374422, "ended_hm": "05:20", "ended_by": "Nothing",
  "ended_by_id": "6998484205", "reason": "app", "tracks": 3,
  "last_track": "Kesariya …", "cleared": true
}
```

## Pushing Telegram events into the app

`webapp::app()` returns the live hub:

```rust
if let Some(hub) = parvati_bot::webapp::app() {
    hub.set_quote("I can't hate anyone, I just know where not to return.");
    hub.note_message("stalker", "stickers tool!");
    hub.set_members(vec![/* id, name, username, photo, role, me, online */]);
}
```

Call these from the message/join handlers to keep the Chat and Members tabs in
sync with what happens in Telegram.

## Security

* The API controls a live voice chat. Every `/api/*` request is authenticated
  with the Telegram WebApp `initData` signature (see **Authentication** above);
  the verified `user.id` replaces the client-supplied `me`, so a caller cannot
  act as someone else.
* `MINIAPP_REQUIRE_MEMBER=1` (default) additionally requires the user to belong
  to the mirrored chat. Set `MINIAPP_ALLOWED_IDS` for an explicit allow-list.
* `MINIAPP_TOKEN` remains for trusted machine integrations (scripts, the bot's
  own calls). It grants the control plane, never a user identity.
* `MINIAPP_ALLOW_OPEN=1` disables the gate for local development only — never
  in production.
* Static assets (HTML/CSS/JS) are public; only `/api/*` is gated. The HTML is
  served `no-store` so an auth change cannot be masked by a cached bundle.
* Private chats are stored under `cache/dm-<chat>.json`; every read is filtered
  by the *verified* Telegram user id, so a thread is only visible to its two
  participants.
* Destructive chat actions (`delete`, `clear`, `forget`) are authorized on the
  server from the verified id. A shared link cannot delete somebody else's
  message, and a client cannot choose a different group to act on: the target
  chat comes from the server-side `user id → chat id` map. The playing room is
  checked against its local roster, other groups against `getChatMember` via
  the Bot API bridge.
* Group messages, chat metadata and switcher state are all keyed by chat id.
  There is no global message queue left to leak one group's history into
  another viewer's session.
* Group-admin powers (`/api/members` promote/kick/ban, `/api/session/end`,
  `/api/chat/leave`, `/api/chat/unban`, `GET /api/chat/banned`) are re-checked
  server-side against the verified id and the target chat. A non-admin gets
  `403` before the request ever reaches the Telegram bridge.

## Reverse proxy (Caddy)

```caddy
music.example.com {
    encode zstd gzip
    reverse_proxy 127.0.0.1:8080
}
```

Telegram requires HTTPS for Mini Apps.

## Native voice engine

`ntg/build.rs` links a prebuilt `libntgcalls` — the C++ engine is not in the
repo (`ntg/native/` is gitignored). Fetch the archives instead of building
WebRTC from source:

```bash
./tools/setup-native.sh          # ntgcalls v2.2.5 static libs → ntg/native/
cargo build --release -p parvati_bot
```

The v2.2.x line is required: v3.x replaced the C API (`ntg_init` /
`ntg_create_call`), which does not match `ntg/src/lib.rs`. The script also
writes `resolver_compat.c` — the bundled c-ares calls unversioned
`__res_nquery` / `__dn_expand`, and glibc only exports them with a version
suffix, so the link fails without the shim.

## Sidecars

The mini app talks to one loopback service, both started by `tools/run.sh`:

| Port | Process | Purpose |
| --- | --- | --- |
| 8098 | `tools/tgapi.py` | Bot API bridge: chat info, admins, members, avatars, sticker sets, premium emoji, send/react/mod/delete |
| 8099 | `tools/downloader.py` | Iota-compatible yt-dlp service |

`tools/supervise.sh [--tunnel]` keeps tgapi, the downloader, the bot and (with
`--tunnel`) a cloudflared quick tunnel alive while you iterate. A quick tunnel
gets a **new hostname on every reconnect**, so the supervisor probes the public
URL (not just the process) every few seconds, follows the newest hostname in
`logs/tunnel.log`, re-points the chat menu button only when that URL actually
changes, and recycles the tunnel after three failed probes — the failure mode
users see as "cloudflare is down". Only hostnames written by the freshly started
tunnel are trusted, so a stale line in the log can never leave the menu button
pointing at a dead address. Use systemd in production.

**`TGAPI_KEY` must match on both sides.** The Rust hub loads `.env` and sends
its `TGAPI_KEY` as the `x-tgapi-key` header on every request to the bridge;
`tools/tgapi.py` resolves `TGAPI_KEY` the same way — an exported environment
variable wins, otherwise it falls back to `.env`, otherwise the default
`local-dev-key`. `TOKEN`, `TGAPI_KEY` and `TGAPI_PORT` all use this shared
resolver, so the two processes cannot disagree even when the supervisor (or a
systemd unit) does not export `.env`. This matters most when you replace the
default key with your own secret: changing it only in `.env` is enough — do
**not** export a different value into just one of the two processes, or every
bridge call (profile cards, saved stickers, message actions) will fail with
`401`. Verify with `curl -s -o /dev/null -w '%{http_code}' -H "x-tgapi-key: $TGAPI_KEY" http://127.0.0.1:8098/health` → `200`.

## Iota catalogue (upstream API)

The bot can also read a full [Iota-api](https://github.com/Iotatg/Iota-api)
instance. Its `/search` adds Deezer, Apple Music, MusicBrainz and SoundCloud
metadata, which is what fills the artwork and exact durations YouTube search
does not have.

```bash
# once
git clone https://github.com/Iotatg/Iota-api && cd Iota-api
python3 -m venv .venv && .venv/bin/pip install -r requirements.txt yt-dlp
# .env: DATABASE_URL=sqlite+aiosqlite:///./data/iota.db  REDIS_DISABLED=true
.venv/bin/uvicorn app.main:app --host 127.0.0.1 --port 8070 --env-file .env

# mint a key, then point the bot at it
curl -X POST http://127.0.0.1:8070/admin/keys \
  -H "X-Admin-Token: $ADMIN_TOKEN" -H 'content-type: application/json' \
  -d '{"name":"parvati"}'
```

```bash
IOTA_API_URL=http://127.0.0.1:8070
IOTA_API_FALLBACK=http://127.0.0.1:8099   # local yt-dlp sidecar, tried on failure
IOTA_API_KEY=iota_…
```

How the results are merged:

| Behaviour | Detail |
| --- | --- |
| Placement | 10 YouTube hits, then up to 6 catalogue rows, then the rest of YouTube (20 total) |
| Labels | `Deezer`, `Apple Music`, `MusicBrainz`, `SoundCloud` — the backend name never shows |
| Duplicates | `yt:<id>` rows are dropped when that video is already in the list |
| Playback | catalogue ids (`dz:…`, `am:…`) are resolved to a playable id by title match before download; the row keeps its own title/artwork |
| Search-only | a catalogue row is never substituted at search time, only resolved on play, so tapping a row always plays *that* song |

If the upstream instance cannot relay bytes from this host (`502` from
`/stream`), the local downloader still serves the same song, so catalogue rows
remain playable.

## Local downloader

Search and metadata are native (no API key), but audio bytes come from an
Iota-style endpoint. `tools/downloader.py` implements that contract on top of
yt-dlp so no external account is needed:

```bash
python3 tools/downloader.py &            # 127.0.0.1:8099
# .env
IOTA_API_URL=http://127.0.0.1:8099
IOTA_API_KEY=local-dev-key
```

YouTube refuses anonymous media fetches from most datacenter IPs, and even
with cookies a plain request now comes back with storyboards only. Three things
have to line up:

| Requirement | Why | How |
| --- | --- | --- |
| cookies | `Sign in to confirm you're not a bot` | export a Netscape jar, `COOKIES_FILE=…` |
| JS runtime | sig / n challenge solving | `--js-runtimes node` (already passed) |
| PO token | YouTube forces SABR otherwise | bgutil provider on `:4416` |

```bash
# 1. PO token provider (once)
git clone --depth 1 https://github.com/Brainicism/bgutil-ytdlp-pot-provider /root/bgutil
cd /root/bgutil/server && npm install && npx tsc
# yt-dlp loads it from the plugin dir:
mkdir -p ~/.config/yt-dlp/plugins && cp -r /root/bgutil/plugin ~/.config/yt-dlp/plugins/bgutil

# 2. run everything
COOKIES_FILE=/etc/parvati/cookies.txt ./tools/run.sh
```

`tools/run.sh` starts the provider, then the downloader, then the bot.

## Public URL during a trial run

Telegram only opens HTTPS Mini Apps. For a quick test without a domain:

```bash
cloudflared tunnel --url http://localhost:8080
curl -X POST "https://api.telegram.org/bot$TOKEN/setChatMenuButton" \
  -H 'content-type: application/json' \
  -d '{"menu_button":{"type":"web_app","text":"Open Player","web_app":{"url":"https://<tunnel>"}}}'
```

Rotate to a real domain + reverse proxy for anything permanent.

## Library, timers and device settings

These live entirely in the mini app and its snapshot; none of them need a bot
command.

| Feature | Where | Storage |
| --- | --- | --- |
| Recently played (last 20) | Queue tab, under "Up next" | `cache/recent-<chat>.json` |
| Favourites | Heart on the Song tab, heart on a search result | `cache/favs-<chat>.json` |
| Sleep timer | `Sleep` chip under the transport | in-memory, `POST /api/sleep {minutes}` |
| Queue reorder | ↑ / ↓ per row | `POST /api/queue/move {from,to}` |
| Recent searches | Search overlay, above Trending | `localStorage.pm-searches` |
| Lite mode | Profile → Lite mode | `localStorage.pm-lite` |

`playback.liked` rides along in every snapshot, so the heart is correct on
first paint. The sleep timer is enforced server-side: when it reaches zero the
room pauses, which every client sees through the normal state frame.

## Performance

The app is used inside Telegram's WebView, where `backdrop-filter` is the most
expensive thing on the page. Three things keep it smooth:

* **Lite mode** — solid panels instead of live blur. Enabled automatically when
  `navigator.deviceMemory <= 4` or `hardwareConcurrency <= 4`, remembered per
  device, and toggleable in Profile. `?lite=1` / `?lite=0` forces it.
* **Coalesced rendering** — SSE frames are merged and painted at most once per
  animation frame (`queueRender`), and every view compares a signature before
  touching `innerHTML`. An idle room causes zero DOM mutations.
* **Background pause** — a hidden tab skips rendering entirely and pauses CSS
  animations (`html.app-hidden`), then does one catch-up render on return.
  Avatars and covers use `loading="lazy"`; the Lottie player is only fetched
  when an animated sticker actually appears.

The blur radii on the dock, tab bar and context menu were also reduced
(26px → 14px), because they sit above scrolling lists and are re-blurred on
every frame of that scroll.

### Playback must never be interrupted by a repaint

Audio/video streams into a single `<audio>` / `<video>` element that survives
renders. Three guards keep it from stuttering:

* **No page reloads.** "Clear media cache" busts same-origin image URLs in
  place; the old implementation called `location.replace()`, which tore down
  the media element and restarted the stream mid-song.
* **Smooth playhead.** `merge()` ignores the server's 1 Hz `position` unless
  the track changed, it is the first frame, or the drift exceeds 4 s. The
  local 250 ms ticker interpolates between drift corrections.
* **Seek cooldown.** `syncAudio()` / `syncVideo()` only correct a >3 s drift,
  only when `readyState >= 2`, and at most once every 5 s. A seek forces a new
  HTTP Range request, so doing it on every tick used to re-buffer constantly.
* **One event stream.** A single guarded `EventSource` with exponential
  backoff; `/api/state` polling runs only while SSE is down. Returning to the
  foreground uses `queueRender()` instead of a full `renderAll()`.

## Gear vs "…" menus

The gear holds **preferences only**; the "…" sheet holds **actions**. They used
to open the same sheet, which mixed toggles with destructive operations.

| Gear (settings) | "…" (more) |
| --- | --- |
| Autoplay, Loop queue, Haptics, Now playing alerts | Group chats (switcher) |
| Lite mode (device-local) | Refresh chat, Clear chat |
| Clear media cache | Banned users (+ Unban) |
| Open bot in Telegram | Invite & share, End voice chat, Leave group |

| Action | Path | Notes |
| --- | --- | --- |
| Refresh chat | `POST /api/chat/refresh` | Re-reads `cache/mirror-<chat>.json`, refreshes group title and roster in the background, returns a complete state frame. The reload is a **replace**, not an append, so pressing it twice never duplicates bubbles; a missing/corrupt mirror clears the in-memory copy (disk is authoritative). |
| Banned users | `GET /api/chat/banned` | Admin only. `cache/banned-<chat>.json` holds bans made through this app; the Bot API cannot enumerate a group's full ban history. |
| Unban | `POST /api/chat/unban` | Admin only. Calls the bridge `/mod` with `action=unban`, then removes the local record. |
| Leave group | `POST /api/chat/leave` | Admin only. Bridge `/leave` → `leaveChat`; the home chat is cleared, other groups are forgotten locally. Requires a confirmation popup in the client. |
| Clear chat | `POST /api/chat/clear` | Admin only. Local mirror only; Telegram history is untouched. |
| End voice chat | `POST /api/session/end` | Admin only. Stops the stream and leaves the call. |

`GET /api/chats` and `GET /api/chat/banned` are handled in the GET branch
*before* the POST-only router — a GET that falls through there returns 404 and
the sheet silently loses its permissions.

## Troubleshooting

| Symptom | Cause |
| --- | --- |
| `bind 0.0.0.0:8080 failed` in the log | port already used — change `MINIAPP_PORT` |
| App loads, everything is empty | `MINIAPP_CHAT_ID` doesn't match a live room |
| `401 unauthorized` | `MINIAPP_TOKEN` set but the URL lacks `?token=` |
| Play does nothing | no downloader reachable — check `IOTA_API_URL`/`IOTA_API_KEY`, or YouTube blocked the host IP (needs `COOKIES_FILE`) |
| Artwork missing | generated gradient covers are used when `artwork` is empty |
