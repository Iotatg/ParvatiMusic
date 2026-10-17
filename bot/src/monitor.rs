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
    /// status already emitted; caller drops the room.
    Finished,
    Failed,
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
    if matches!(outcome, Outcome::Finished | Outcome::Failed) {
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

    let Some(mut next) = next else {
        parv_info!("monitor", "advance chat {chat_id}: queue finished");
        set_status(bot, r, &f(loc, chat_id, "stream_queue_finished"));
        return Outcome::Finished;
    };
    parv_info!("monitor", "advance chat {chat_id}: next track \"{}\"", next.title);
    set_status(bot, r, &f(loc, chat_id, "stream_downloading_next"));

    let mut dl_err = String::new();
    let mut path = sink.download_track(&next, &mut dl_err);
    if path.is_empty() && next.is_autoplay {
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
                    next = c;
                    break;
                }
            }
        }
    }
    if path.is_empty() {
        let failed = if dl_err.is_empty() { "unknown".to_string() } else { dl_err.clone() };
        parv_warn!("monitor", "advance chat {chat_id}: download failed: {failed}");
        let err = if dl_err.is_empty() { "download".to_string() } else { dl_err };
        set_status(bot, r, &f_args(loc, chat_id, "stream_download_fail", &[("error", err.as_str())]));
        return Outcome::Failed;
    }
    if r.destroyed() {
        return Outcome::Gone;
    }
    if !matches!(r.play(next.clone(), path.clone(), true), PlayAction::Started) {
        set_status(bot, r, &f(loc, chat_id, "stream_play_fail"));
        return Outcome::Failed;
    }

 // Context glue (RoomState::play called play_native directly).
    let speed = r.speed();
    let video = next.video;
    if play_native_at(chat_id, r, assists, &path, 0, speed, video).is_err() {
        set_status(bot, r, &f(loc, chat_id, "stream_play_fail"));
        return Outcome::Failed;
    }
    r.playing = true;
    send_now_playing(bot, chat_id, r, &next, loc);
    Outcome::Started
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
    _r: &mut RoomState,
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

/// Wire every assistant's stream-end callback to a shared channel; the caller
/// (main) drains it and advances the affected room off the ntg thread.
pub fn wire_stream_end(assists: &AssistantManager) -> std::sync::mpsc::Receiver<i64> {
    let (tx, rx) = std::sync::mpsc::channel();
    assists.for_each(|a| {
        let mut ctx = a.ntg();
        ctx.on_stream_end(tx.clone());
    });
    parv_info!("monitor", "stream-end handlers wired for assistants");
    rx
}

/// Drive stream-end events into advance_room on their own threads
pub fn run_stream_end(
    rx: std::sync::mpsc::Receiver<i64>,
    rooms: Rooms,
    assists: Arc<AssistantManager>,
    bot: Arc<Mutex<Box<dyn Bot>>>,
    loc: Arc<Locales>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        for chat_id in rx {
            let bot = bot.clone();
            let rooms = rooms.clone();
            let assists = assists.clone();
            let loc = loc.clone();
            thread::spawn(move || {
                let mut b = bot.lock().unwrap_or_else(|e| e.into_inner());
                let bb: &mut dyn Bot = b.as_mut();
                advance_room(chat_id, &rooms, &assists, bb, &loc, &RealSink);
            });
        }
    })
}
