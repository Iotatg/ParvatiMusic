//! monitor — the advance loop, the 4s
//! room watchdog, and the stream-end wiring that feeds both.
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::assistant::AssistantManager;
use crate::locales::Locales;
use crate::models::Track;
use parvati_mtproto::messaging::Markup;

use crate::mod_tg::Bot;
use crate::modules::common::{card_edit, f, f_args, send_now_playing, set_status};
use crate::platforms::registry::{autoplay_tracks, download_track, golden_zone_pick};
use crate::platforms::iota::ProgressFn;
use crate::room::{PlayAction, Rooms, RoomState};
use crate::{parv_error, parv_info, parv_warn};

/// Injectable platform facade: the monitor never hits the network directly so
/// tests can script downloads and autoplay pools.
pub trait DownloadSink: Send + Sync {
    fn download_track(&self, t: &Track, err: &mut String) -> String;
    fn autoplay_tracks(&self, last: &Track, limit: i64, history: &[String]) -> Vec<Track>;
}

/// Production sink: registry download (no cancel hook — the monitor's
/// downloads are unstoppable, only /play's carry one) + autoplay tiers.
pub struct RealSink;

impl DownloadSink for RealSink {
    fn download_track(&self, t: &Track, err: &mut String) -> String {
        let cancel = Arc::new(AtomicBool::new(false));
        let progress: Arc<ProgressFn> = Arc::new(|_p: f64, _t: f64, _d: i64, _s: f64| {});
        download_track(t, err, &cancel, progress.as_ref())
    }
    fn autoplay_tracks(&self, last: &Track, limit: i64, history: &[String]) -> Vec<Track> {
        autoplay_tracks(last, limit, history)
    }
}

/// What the locked half of advance_room decided to do next.
enum Outcome {
    Replayed,
    Started,
    /// a mini-app play is downloading; the room was left untouched.
    Deferred,
    /// status already emitted; caller drops the room.
    Finished,
    /// Nothing playable right now, but the call stays up (download failure /
    /// engine refusal). The room is kept, not dropped.
    Stalled,
    Gone,
}

/// advance_room: stream-end callback + watchdog entry point. Serialized
/// per room by the advance lock (both sources can call this concurrently).
pub fn advance_room(
    chat_id: i64,
    rooms: &Rooms,
    assists: &AssistantManager,
    bot: &mut dyn Bot,
    loc: &Locales,
    sink: &dyn DownloadSink,
) -> bool {
    let Some(room) = rooms.room_for(chat_id) else {
        return false;
    };
    let mut r = room.lock().unwrap_or_else(|e| e.into_inner());
    if r.destroyed() || !r.try_lock_advance() {
        return false;
    }
    let outcome = advance_locked(&mut r, chat_id, assists, bot, loc, sink);
    r.unlock_advance();
    drop(r);
    if matches!(outcome, Outcome::Finished) {
        rooms.drop_room(chat_id);
    }
    true
}

fn advance_locked(
    r: &mut RoomState,
    chat_id: i64,
    assists: &AssistantManager,
    bot: &mut dyn Bot,
    loc: &Locales,
    sink: &dyn DownloadSink,
) -> Outcome {
    // Loop mode repeats the current track (Go: loopCount decrement).
    if crate::webapp::play_pending() {
        // A mini-app tap already chose the next song and its download is in
        // flight. Advancing now (queue or autoplay) would start a different
        // track that the user's pick then interrupts — the "it played another
        // song" report. Leave the room alone; the play thread takes over.
        parv_info!("monitor", "advance chat {chat_id}: deferred, mini app play pending");
        return Outcome::Deferred;
    }
    if r.loop_n > 0 {
        r.set_loop(r.loop_n - 1);
        if r.replay() {
 // replay runs play_native internally.
            let path = r.file_path.clone();
            if !play_native(chat_id, r, assists, &path, 0) {
                r.position = 0;
                return Outcome::Replayed;
            }
            let text = f(loc, chat_id, "cb_replaying");
            if r.status_msg == 0 {
                let id = bot.send_text_sync(chat_id, &text, None, 0).unwrap_or(0);
                r.status_msg = id;
                r.status_is_photo = false;
            } else {
                let _ = card_edit(bot, chat_id, r.status_msg, &text, None, r.status_is_photo);
            }
        }
        return Outcome::Replayed;
    }

    let mut next = r.next_track();
    if next.is_none() && r.autoplay {
        if let Some(last) = r.track.clone() {
            let history = r.played_history();
            let pool: Vec<Track> = sink
                .autoplay_tracks(&last, 5, &history)
                .into_iter()
                .filter(|c| !(c.id.is_empty() && c.url.is_empty()))
                .collect();
            if let Some(mut pick) = golden_zone_pick(&pool) {
                pick.mark_autoplay(&f(loc, chat_id, "autoplay_requester"), &r.engine_token);
                next = Some(pick);
            }
        }
    }

    let Some(first_next) = next else {
        parv_info!("monitor", "advance chat {chat_id}: queue finished");
        set_status(bot, r, &f(loc, chat_id, "stream_queue_finished"));
        return Outcome::Finished;
    };
    let mut next: Option<Track> = Some(first_next);

    // A failed download must NOT end the voice chat. The old `Failed` outcome
    // dropped the room, so one 502 from the catalogue closed the call and
    // looked exactly like a crash. Try the next queued track (3 attempts),
    // then stay in the call idle instead of hanging up.
    let mut last_err = String::new();
    let mut tries = 0u32;
    while let Some(candidate) = next.take() {
        tries += 1;
        let mut track = candidate;
        parv_info!(
            "monitor",
            "advance chat {chat_id}: next track \"{}\" (attempt {tries})",
            track.title
        );
        set_status(bot, r, &f(loc, chat_id, "stream_downloading_next"));

        let mut dl_err = String::new();
        let mut path = sink.download_track(&track, &mut dl_err);
        if path.is_empty() && track.is_autoplay {
            // Re-pick pass for autoplay on download failure (Go retry tiers):
            // 8 candidates this time, tried in relevance order.
            if let Some(last) = r.track.clone() {
                let history = r.played_history();
                for c in sink.autoplay_tracks(&last, 8, &history) {
                    if c.id.is_empty() && c.url.is_empty() {
                        continue;
                    }
                    dl_err.clear();
                    path = sink.download_track(&c, &mut dl_err);
                    if !path.is_empty() {
                        let mut c = c;
                        c.mark_autoplay(&f(loc, chat_id, "autoplay_requester"), &r.engine_token);
                        track = c;
                        break;
                    }
                }
            }
        }

        if path.is_empty() {
            last_err = if dl_err.is_empty() {
                "download".to_string()
            } else {
                dl_err.clone()
            };
            parv_warn!(
                "monitor",
                "advance chat {chat_id}: attempt {tries} failed: {last_err}"
            );
            if tries >= 3 {
                break;
            }
            next = r.next_track();
            if next.is_none() && r.autoplay {
                if let Some(last) = r.track.clone() {
                    let history = r.played_history();
                    let pool: Vec<Track> = sink
                        .autoplay_tracks(&last, 5, &history)
                        .into_iter()
                        .filter(|c| !(c.id.is_empty() && c.url.is_empty()))
                        .collect();
                    if let Some(mut pick) = golden_zone_pick(&pool) {
                        pick.mark_autoplay(
                            &f(loc, chat_id, "autoplay_requester"),
                            &r.engine_token,
                        );
                        next = Some(pick);
                    }
                }
            }
            continue;
        }

        if r.destroyed() {
            return Outcome::Gone;
        }
        if !matches!(r.play(track.clone(), path.clone(), true), PlayAction::Started) {
            set_status(bot, r, &f(loc, chat_id, "stream_play_fail"));
            return Outcome::Stalled;
        }

        // Context glue (RoomState::play called play_native directly).
        let speed = r.speed();
        let video = track.video;
        if play_native_at(chat_id, r, assists, &path, 0, speed, video).is_err() {
            set_status(bot, r, &f(loc, chat_id, "stream_play_fail"));
            return Outcome::Stalled;
        }
        r.playing = true;
        send_now_playing(bot, chat_id, r, &track, loc);
        return Outcome::Started;
    }

    let err = if last_err.is_empty() {
        "download".to_string()
    } else {
        last_err
    };
    set_status(
        bot,
        r,
        &f_args(loc, chat_id, "stream_download_fail", &[("error", err.as_str())]),
    );
    // Stay in the call: the bot remains joined, the app shows the error and a
    // new tap (or /play) starts the next song.
    r.playing = false;
    Outcome::Stalled
}

/// play_native: hand the file to the chat's assistant voice engine.
fn play_native(
    chat_id: i64,
    r: &mut RoomState,
    assists: &AssistantManager,
    path: &str,
    pos: i64,
) -> bool {
    let video = r.track.as_ref().map(|t| t.video).unwrap_or(false);
    let speed = r.speed();
    play_native_at(chat_id, r, assists, path, pos, speed, video).is_ok()
}

pub(crate) fn play_native_at(
    chat_id: i64,
    r: &mut RoomState,
    assists: &AssistantManager,
    path: &str,
    pos: i64,
    speed: f64,
    video: bool,
) -> Result<(), String> {
    match assists.for_chat(chat_id) {
        Some(a) => {
            let mut ctx = a.ntg();
            if let Err(e) = ctx.play_file_mode(chat_id, path, pos, speed, video) {
                parv_error!("monitor", "advance chat {chat_id}: play failed: {e}");
                Err(e)
            } else {
                // The playhead must start when the engine *accepted* the
                // stream, not when `RoomState::play` reserved the slot: the
                // connect/handshake can take seconds, and a clock started too
                // early made the 4 s watchdog believe the song had ended and
                // cut it off before the last verse.
                r.position = pos;
                r.updated_at = now_secs() as i64;
                r.playing = true;
                Ok(())
            }
        }
        None => {
            parv_error!("monitor", "advance chat {chat_id}: no assistant configured");
            Err("no assistant configured for this chat".into())
        }
    }
}

/// start_track: hand an already-`play()`-ed track's file to the chat's voice
/// engine. Shared by the monitor advance loop, `/skip`, the skip card button
/// and `/askip` so every "start the next song" path actually switches the
/// stream (and not just the RoomState/card). On a native failure the caller's
/// just-started track is rolled back so the queue is not silently consumed.
pub fn start_track(
    chat_id: i64,
    r: &mut RoomState,
    assists: &AssistantManager,
    path: &str,
    video: bool,
) -> bool {
    // No assistant configured for this chat: there is no voice engine to
    // drive, so the state-only advance is the whole job (this is also the
    // test-double path). A real engine error still fails the start.
    if assists.for_chat(chat_id).is_none() {
        r.playing = true;
        return true;
    }
    let speed = r.speed();
    if play_native_at(chat_id, r, assists, path, 0, speed, video).is_ok() {
        r.playing = true;
        return true;
    }
    r.roll_back_failed_start();
    false
}

/// Pause the chat's voice engine. A missing assistant is a no-op success so
/// state-only deployments (and tests without a configured engine) still work.
pub fn engine_pause(chat_id: i64) -> bool {
    match crate::modules::assistants().and_then(|a| a.for_chat(chat_id)) {
        Some(a) => a.ntg().pause(chat_id),
        None => true,
    }
}

/// Resume the chat's voice engine (see `engine_pause` for the None contract).
pub fn engine_resume(chat_id: i64) -> bool {
    match crate::modules::assistants().and_then(|a| a.for_chat(chat_id)) {
        Some(a) => a.ntg().resume(chat_id),
        None => true,
    }
}

/// Mute the chat's voice engine (see `engine_pause` for the None contract).
pub fn engine_mute(chat_id: i64) -> bool {
    match crate::modules::assistants().and_then(|a| a.for_chat(chat_id)) {
        Some(a) => a.ntg().mute(chat_id),
        None => true,
    }
}

/// Unmute the chat's voice engine (see `engine_pause` for the None contract).
pub fn engine_unmute(chat_id: i64) -> bool {
    match crate::modules::assistants().and_then(|a| a.for_chat(chat_id)) {
        Some(a) => a.ntg().unmute(chat_id),
        None => true,
    }
}

/// Re-start the current stream at `RoomState::position`. libntgcalls has no
/// seek primitive, so a seek/speed/replay change re-issues `set_stream_sources`
/// with `ffmpeg -ss <pos>` (the reference bot's `play()` does the same).
pub fn engine_restart(chat_id: i64, r: &mut RoomState) -> Result<(), String> {
    if r.file_path.is_empty() {
        return Ok(());
    }
    let path = r.file_path.clone();
    let video = r.track.as_ref().map(|t| t.video).unwrap_or(false);
    let (pos, speed) = (r.position, r.speed);
    match crate::modules::assistants() {
        Some(assists) => play_native_at(chat_id, r, assists, &path, pos, speed, video),
        None => Ok(()),
    }
}

/// Watchdog decision: "past the end" for three
/// consecutive 4s ticks (12s) advances manually. Strict `>= duration` so
/// seeking near the end cannot false-fire.
pub fn end_streak_hit(position: i64, duration: i64, streak: i32) -> bool {
    duration > 0 && position >= duration && streak >= 3
}

/// The 4s room watchdog: refresh positions from the
/// voice engine, rescue stalled queues, and keep the now-playing card's
/// progress row fresh.
pub fn start_room_monitor(
    rooms: Rooms,
    assists: Arc<AssistantManager>,
    bot: Arc<Mutex<Box<dyn Bot>>>,
    loc: Arc<Locales>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut end_streak: HashMap<i64, i32> = HashMap::new();
        loop {
            thread::sleep(Duration::from_secs(4));
            for (chat_id, room) in rooms.all_rooms() {
                tick_room(chat_id, room, &rooms, &assists, &bot, &loc, &mut end_streak);
            }
        }
    })
}

/// One room per 4s tick. Extracted so the advance call never runs while the
/// room lock is held (drop_room re-locks it).
fn tick_room(
    chat_id: i64,
    room: Arc<Mutex<RoomState>>,
    rooms: &Rooms,
    assists: &Arc<AssistantManager>,
    bot: &Arc<Mutex<Box<dyn Bot>>>,
    loc: &Locales,
    end_streak: &mut HashMap<i64, i32>,
) {
    let mut r = room.lock().unwrap_or_else(|e| e.into_inner());
    if !r.active() || r.paused || r.destroyed() {
        return;
    }
    let Some(t) = r.track.clone() else { return };
 //: the room's own position advances by elapsed*speed;
    // the voice engine is never polled.
    r.tick_position();
    let pos = r.position;
    if t.duration > 0 && pos >= t.duration {
        let s = end_streak.entry(chat_id).or_insert(0);
        *s += 1;
        if end_streak_hit(pos, t.duration, *s) {
            end_streak.remove(&chat_id);
            parv_info!("monitor", "end watchdog: advancing chat {chat_id}");
            drop(r);
            let mut b = bot.lock().unwrap_or_else(|e| e.into_inner());
            let bb: &mut dyn Bot = b.as_mut();
            advance_room(chat_id, rooms, assists, bb, loc, &RealSink);
            return;
        }
    } else {
        end_streak.remove(&chat_id);
    }
    let mid = r.status_msg;
    if mid == 0 || (t.duration > 0 && pos >= t.duration) {
        return;
    }
    let text = crate::modules::common::now_playing_text(loc, chat_id, &t);
    let mk = play_markup(loc, chat_id, &r.view(), false);
    let status_is_photo = r.status_is_photo;
    // Drop the room lock before taking the bot lock: the main pump thread
    // holds the bot lock while a command handler takes the room lock, so
    // holding room -> bot here would deadlock them against each other.
    drop(r);
    let mut b = bot.lock().unwrap_or_else(|e| e.into_inner());
    card_edit(b.as_mut(), chat_id, mid, &text, Some(&mk), status_is_photo);
}

fn play_markup(loc: &Locales, chat_id: i64, view: &crate::buttons::RoomView, queued: bool) -> Markup {
    let lang = crate::db::language(chat_id);
    crate::buttons::play_markup(loc, &lang, view, queued)
}

/// One native stream-end event: chat, stream mode and the device whose source
/// ended. A video track owns *two* ffmpeg pipes (microphone + camera); only the
/// microphone ending means the song is over.
#[derive(Clone, Copy, Debug)]
pub struct StreamEnd {
    pub chat: i64,
    pub mode: i32,
    pub device: i32,
}

/// A real "song finished" arrives within this many seconds of the track
/// duration. Anything earlier is a closed/dropped source (EPIPE), not the end.
const END_TOLERANCE_SECS: i64 = 8;
/// A premature EOF is healed by restarting the source at the current position.
/// After this many heals on the same track, stop fighting and advance.
const MAX_HEALS: u32 = 3;
/// The heal budget resets when the track changes or after this long.
const HEAL_WINDOW_SECS: u64 = 180;
/// Two microphone ends for the same chat inside this window are one event.
const END_DEBOUNCE_SECS: u64 = 2;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// chat → (first-heal-at, heals-used, track-id).
fn heals() -> &'static Mutex<HashMap<i64, (u64, u32, String)>> {
    static H: std::sync::OnceLock<Mutex<HashMap<i64, (u64, u32, String)>>> = std::sync::OnceLock::new();
    H.get_or_init(|| Mutex::new(HashMap::new()))
}

/// chat → last microphone-end timestamp (debounce).
fn last_end() -> &'static Mutex<HashMap<i64, u64>> {
    static L: std::sync::OnceLock<Mutex<HashMap<i64, u64>>> = std::sync::OnceLock::new();
    L.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Wire every assistant's stream-end callback to a shared channel; the caller
/// (main) drains it and advances the affected room off the ntg thread.
pub fn wire_stream_end(assists: &AssistantManager) -> std::sync::mpsc::Receiver<StreamEnd> {
    let (tx, rx) = std::sync::mpsc::channel();
    assists.for_each(|a| {
        let mut ctx = a.ntg();
        // Never block the native audio thread: the context forwards the event
        // to the monitor thread, which decides what it means.
        ctx.on_stream_end(tx.clone());
    });
    parv_info!("monitor", "stream-end handlers wired for assistants");
    rx
}

/// Drive stream-end events into the room, filtering out the ones that must not
/// end playback (camera/screen EOF, duplicate reports) and healing a premature
/// audio EOF instead of dropping the call.
pub fn run_stream_end(
    rx: std::sync::mpsc::Receiver<StreamEnd>,
    rooms: Rooms,
    assists: Arc<AssistantManager>,
    bot: Arc<Mutex<Box<dyn Bot>>>,
    loc: Arc<Locales>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        for ev in rx {
            // A video track's camera pipe ends independently of the song. Its
            // EOF (or a broken pipe when the encoder closes) used to advance
            // the whole room and drop the voice chat mid-song.
            if ev.device == parvati_ntg::DEVICE_CAMERA || ev.device == parvati_ntg::DEVICE_SCREEN {
                parv_info!(
                    "monitor",
                    "stream end chat {}: video source closed (device {}), ignored",
                    ev.chat,
                    ev.device
                );
                continue;
            }
            if ev.mode != parvati_ntg::STREAM_CAPTURE {
                continue;
            }
            {
                let now = now_secs();
                let mut last = last_end().lock().unwrap_or_else(|e| e.into_inner());
                if let Some(prev) = last.get(&ev.chat) {
                    if now.saturating_sub(*prev) < END_DEBOUNCE_SECS {
                        continue;
                    }
                }
                last.insert(ev.chat, now);
            }
            let bot = bot.clone();
            let rooms = rooms.clone();
            let assists = assists.clone();
            let loc = loc.clone();
            thread::spawn(move || {
                let mut b = bot.lock().unwrap_or_else(|e| e.into_inner());
                let bb: &mut dyn Bot = b.as_mut();
                handle_stream_end(ev, &rooms, &assists, bb, &loc);
            });
        }
    })
}

/// Decide what a microphone stream-end means for the room.
fn handle_stream_end(
    ev: StreamEnd,
    rooms: &Rooms,
    assists: &AssistantManager,
    bot: &mut dyn Bot,
    loc: &Locales,
) {
    if crate::webapp::play_pending() {
        return;
    }
    let Some(room) = rooms.room_for(ev.chat) else { return };
    let (premature, pos, dur, track_id) = {
        let mut r = room.lock().unwrap_or_else(|e| e.into_inner());
        if r.destroyed() || !r.active() || r.paused {
            return;
        }
        // Bring the playhead up to date before comparing it to the duration;
        // the 4 s tick may be stale by a few seconds.
        r.tick_position();
        let dur = r.track.as_ref().map(|t| t.duration).unwrap_or(0);
        let pos = r.position;
        let id = r.track.as_ref().map(|t| t.id.clone()).unwrap_or_default();
        (dur > 0 && pos + END_TOLERANCE_SECS < dur, pos, dur, id)
    };

    if premature {
        parv_warn!(
            "monitor",
            "stream end chat {}: premature EOF at {pos}s/{dur}s — recovering, not advancing",
            ev.chat
        );
        if heal_stream(ev.chat, &room, assists, pos, &track_id) {
            return;
        }
        parv_warn!(
            "monitor",
            "stream end chat {}: recovery exhausted, advancing",
            ev.chat
        );
    }

    advance_room(ev.chat, rooms, assists, bot, loc, &RealSink);
}

/// Restart the source at the current position. Returns true when the stream is
/// live again (or the room is gone), false when the heal budget is spent.
fn heal_stream(
    chat: i64,
    room: &Arc<Mutex<RoomState>>,
    assists: &AssistantManager,
    pos: i64,
    track_id: &str,
) -> bool {
    let now = now_secs();
    let attempt = {
        let mut h = heals().lock().unwrap_or_else(|e| e.into_inner());
        let e = h.entry(chat).or_insert((0, 0, String::new()));
        if e.2 != track_id || now.saturating_sub(e.0) > HEAL_WINDOW_SECS {
            *e = (now, 0, track_id.to_string());
        }
        if e.1 >= MAX_HEALS {
            return false;
        }
        e.1 += 1;
        e.1
    };

    let (path, speed, video) = {
        let r = room.lock().unwrap_or_else(|e| e.into_inner());
        if r.destroyed() || !r.active() {
            return true;
        }
        (
            r.file_path.clone(),
            r.speed(),
            r.track.as_ref().map(|t| t.video).unwrap_or(false),
        )
    };
    if path.is_empty() {
        return false;
    }

    parv_warn!(
        "monitor",
        "stream end chat {chat}: restarting at {pos}s (heal {attempt}/{MAX_HEALS})"
    );
    let mut r = room.lock().unwrap_or_else(|e| e.into_inner());
    match play_native_at(chat, &mut r, assists, &path, pos, speed, video) {
        Ok(()) => {
            r.playing = true;
            true
        }
        Err(e) => {
            parv_error!("monitor", "stream end chat {chat}: heal failed: {e}");
            false
        }
    }
}
