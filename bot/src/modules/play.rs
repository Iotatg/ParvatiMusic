//! Task 12 — /play command family ( +
//! plus the registered command aliases and the assistant membership guard
//! (chat_state.rs).
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use parvati_mtproto::messaging::TgMessage;

use crate::buttons::cancel_keyboard;
use crate::modules::chat_state::ensure_voice_chat_ready;
use crate::modules::common::{f, f_args, mention_of, now_playing_text, show_track, show_track_queued, LiveMsg};
use crate::modules::{loc, Registry};
use crate::mod_tg::Bot;
use crate::platforms::registry;
use crate::room::{PlayAction, RoomState};
use crate::utils::{escape_html, short_title};

/// handle_play(m, force, cplay, video) options.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlayOpts {
    pub force: bool,
    pub cplay: bool,
    pub video: bool,
}

/// Platform seam so tests can script resolve/download results (mirrors the
/// monitor DownloadSink precedent); the real paths use the global registry.
pub trait PlaySink {
    fn resolve_query(&self, query: &str, video: bool) -> Vec<crate::models::Track>;
    fn download_track(&self, chat_id: i64, t: &crate::models::Track, err: &mut String) -> String;
}

/// Registry-backed sink used by the registered commands.
pub struct RealSink;

impl PlaySink for RealSink {
    fn resolve_query(&self, query: &str, video: bool) -> Vec<crate::models::Track> {
        registry::resolve_query(query, video)
    }
    fn download_track(&self, chat_id: i64, t: &crate::models::Track, err: &mut String) -> String {
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;
        let cancel = Arc::new(AtomicBool::new(false));
        super::common::dl_register_cancel(chat_id, cancel.clone());
        let progress: std::sync::Arc<crate::platforms::iota::ProgressFn> =
            std::sync::Arc::new(|_: f64, _: f64, _: i64, _: f64| {});
        let path = registry::download_track(t, err, &cancel, progress.as_ref());
        super::common::dl_unregister_cancel(chat_id, &cancel);
        path
    }
}

/// command_of.
fn command_of(m: &TgMessage) -> String {
    match m.text.find(' ') {
        Some(sp) => m.text[..sp].to_string(),
        None => m.text.clone(),
    }
}

/// extract_query.
fn extract_query(text: &str) -> String {
    match text.find(' ') {
        Some(sp) => text[sp + 1..].trim().to_string(),
        None => String::new(),
    }
}

/// flood_wait_seconds (tolerant: non-numeric -> 0).
fn flood_wait_seconds(err: &str) -> i64 {
    let Some(p) = err.find("FLOOD_WAIT_") else { return 0 };
    err[p + 11..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}

/// flood_wait_text.
fn flood_wait_text(chat_id: i64, wait: i64) -> String {
    let loc = loc();
    if wait >= 60 {
        f_args(loc, chat_id, "flood_minutes", &[("duration", &(wait / 60).to_string())])
    } else {
        f_args(loc, chat_id, "flood_seconds", &[("duration", &wait.to_string())])
    }
}

/// handle_play_attempt_error.
/// Returns true when the caller should retry (Retry), false when it must stop.
fn handle_play_attempt_error(err: &str, attempt: i32, chat_id: i64, lm: &mut LiveMsg, bot: &mut dyn Bot) -> bool {
    let loc = loc();
    let wait = flood_wait_seconds(err);
    if wait > 0 {
        crate::parv_warn!("play", "FloodWait detected ({wait}s). Retrying... (attempt {attempt})");
        lm.eor(bot, &flood_wait_text(chat_id, wait), None);
        thread::sleep(Duration::from_secs(wait as u64));
        return true;
    }
    if err.contains("connection timeout") || err.contains("no response from ntgcalls") || err.contains("timed out") {
        crate::parv_error!("play", "Voice connection timeout. Stopping call session...");
        lm.eor(bot, &f(loc, chat_id, "err_connection_timeout"), None);
        crate::modules::rooms().map(|r| r.drop_room(chat_id));
        return false;
    }
    if err.contains("Streaming is not supported when using RTMP") {
        crate::parv_error!("play", "RTMP/live-stream voice chat detected, cannot play. Cleaning up...");
        crate::modules::rooms().map(|r| r.drop_room(chat_id));
        lm.eor(bot, &f(loc, chat_id, "rtmp_play_unsupported"), None);
        return false;
    }
    if err.contains("group call") && err.contains("is closed") {
        crate::parv_error!("play", "No active voice chat: {err}");
        lm.eor(bot, &f(loc, chat_id, "err_no_active_voicechat"), None);
        return false;
    }
    if err.contains("GROUPCALL_INVALID") {
        crate::parv_error!("play", "GROUPCALL_INVALID err occurred. Returning...");
        crate::modules::rooms().map(|r| r.drop_room(chat_id));
        lm.eor(bot, &f(loc, chat_id, "play_unable"), None);
        return false;
    }
    if err.contains("INTERDC_X_CALL_ERROR") {
        crate::parv_warn!("play", "INTERDC_X_CALL_ERROR occurred. Retrying... (attempt {attempt})");
        thread::sleep(Duration::from_secs(2));
        return true;
    }
    true
}

/// track_in_queue.
fn track_in_queue(r: &RoomState, t: &crate::models::Track) -> bool {
    r.queue().iter().any(|q| q.url == t.url || q.id == t.id)
}

/// skipped_tracks_text.
fn skipped_tracks_text(chat_id: i64, skipped: &[String]) -> String {
    let loc = loc();
    let limit_mins = crate::modules::cfg().duration_limit / 60;
    let mut out = f_args(
        loc,
        chat_id,
        "play_multiple_tracks_too_long_header",
        &[("count", &skipped.len().to_string()), ("limit_mins", &limit_mins.to_string())],
    ) + "\n";
    for (i, title) in skipped.iter().enumerate() {
        if i < 5 {
            out += &f_args(loc, chat_id, "play_multiple_tracks_too_long_item", &[("title", title)]);
            out += "\n";
            continue;
        }
        out += &f_args(
            loc,
            chat_id,
            "play_multiple_tracks_too_long_more",
            &[("remaining", &(skipped.len() - i).to_string())],
        );
        out += "\n";
        break;
    }
    out
}

/// multi_added_text.
fn multi_added_text(chat_id: i64, count: i64, mention: &str, available_slots: i64, track_count: i64) -> String {
    let loc = loc();
    let queue_limit = crate::modules::cfg().queue_limit;
    let mut out =
        f_args(loc, chat_id, "play_added_multiple_header", &[("count", &count.to_string()), ("user", mention)]);
    out += "\n\n";
    if queue_limit > 0 && available_slots <= track_count {
        out += &f(loc, chat_id, "play_queue_limit_hint");
        out += "\n";
    }
    out += &f(loc, chat_id, "play_queue_view_hint");
    out
}

/// play_room: cplay mirrors onto the linked channel; the
/// room keeps the original chat id for card edits.
fn play_room(m: &TgMessage, cplay: bool) -> Result<(crate::room::Rooms, std::sync::Arc<Mutex<RoomState>>, i64), String> {
    let chat_id = m.chat_id;
    let orig = chat_id;
    let room_id = if cplay {
        let linked = crate::db::linked_channel(chat_id);
        if linked == 0 {
            return Err(f(loc(), orig, "cplay_id_not_set"));
        }
        linked
    } else {
        chat_id
    };
    let rooms = crate::modules::rooms().cloned().unwrap_or_default();
    let r = rooms.create_room(room_id, crate::db::autoplay(room_id));
    Ok((rooms, r, orig))
}

/// handle_play.
pub fn handle_play(bot: &mut dyn Bot, m: &TgMessage, opts: PlayOpts, sink: &dyn PlaySink) {
    if m.is_channel {
        return;
    }
    let loc = loc();
    let chat_id = m.chat_id;

    if crate::db::play_mode_admins_only(chat_id)
        && !crate::db::is_authorized(chat_id, m.sender_id)
        && !crate::db::is_sudo(m.sender_id)
    {
        let _ = bot.reply_text(m, &f(loc, chat_id, "playmode_restricted"), None);
        return;
    }

    let (rooms, room, orig) = match play_room(m, opts.cplay) {
        Ok(v) => v,
        Err(e) => {
            let _ = bot.reply_text(m, &e, None);
            return;
        }
    };
    let _ = orig;
    let cfg = crate::modules::cfg();
    {
        let guard = room.lock().unwrap_or_else(|e| e.into_inner());
        if cfg.queue_limit > 0 && guard.queue().len() as i64 >= cfg.queue_limit {
            drop(guard);
            let _ = bot.reply_text(
                m,
                &f_args(loc, chat_id, "queue_limit_reached", &[("limit", &cfg.queue_limit.to_string())]),
                None,
            );
            return;
        }
    }

    let query = extract_query(&m.text);
    if query.is_empty() && m.reply_to == 0 {
 // Deviation note: also falls back to the replied media caption.
        let _ = bot.reply_text(m, &f_args(loc, chat_id, "no_song_query", &[("cmd", &command_of(m))]), None);
        return;
    }

    let mut lm = LiveMsg::new();
    lm.reply(
        bot,
        chat_id,
        &if query.is_empty() {
            f(loc, chat_id, "searching")
        } else {
            f_args(loc, chat_id, "searching_query", &[("query", &escape_html(&query))])
        },
        None,
        0,
    );

    let tracks = sink.resolve_query(&query, opts.video);
    crate::parv_info!("play", "resolve done: {} track(s)", tracks.len());
    if tracks.is_empty() {
        lm.eor(bot, &f(loc, chat_id, "no_song_found"), None);
        return;
    }

 // fetchTracksAndCheckStatus order: after tracks resolve, make
    // sure the assistant is actually a chat member before anything is queued.
    let assistants = crate::modules::assistants();
    let guard_err = match assistants {
        Some(a) => ensure_voice_chat_ready(bot, &room, a, chat_id),
        None => f(loc, chat_id, "err_assistant_get_failed"),
    };
    if !guard_err.is_empty() {
        lm.eor(bot, &guard_err, None);
        return;
    }

    let room_id = room.lock().unwrap_or_else(|e| e.into_inner()).id;
    let guard = room.lock().unwrap_or_else(|e| e.into_inner());
    if tracks.len() == 1 && !opts.force && track_in_queue(&guard, &tracks[0]) {
        drop(guard);
        lm.eor(
            bot,
            &f_args(
                loc,
                chat_id,
                "play_already_in_queue",
                &[("title", &escape_html(&short_title(&tracks[0].title, 35)))],
            ),
            None,
        );
        return;
    }

    let is_active = guard.active();
    if cfg.queue_limit == 0 && is_active && !opts.force {
        drop(guard);
        lm.eor(bot, &f(loc, chat_id, "play_already_playing"), None);
        return;
    }

    let mut kept = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for t in &tracks {
        if cfg.duration_limit > 0 && t.duration > cfg.duration_limit {
            skipped.push(escape_html(&short_title(&t.title, 35)));
            continue;
        }
        kept.push(t.clone());
    }
    if !skipped.is_empty() {
        if tracks.len() == 1 {
            drop(guard);
            lm.eor(
                bot,
                &f_args(
                    loc,
                    chat_id,
                    "play_single_track_too_long",
                    &[
                        ("limit_mins", &crate::utils::format_duration(cfg.duration_limit)),
                        ("title", &skipped[0]),
                    ],
                ),
                None,
            );
            return;
        }
        lm.eor(bot, &skipped_tracks_text(chat_id, &skipped), None);
        thread::sleep(Duration::from_secs(1));
    }
    if kept.is_empty() {
        drop(guard);
        lm.eor(bot, &f(loc, chat_id, "play_all_tracks_skipped"), None);
        return;
    }

    let mut available_slots = kept.len() as i64;
    if cfg.queue_limit >= 0 {
        available_slots = cfg.queue_limit - guard.queue().len() as i64;
        if available_slots < 0 {
            available_slots = 0;
        }
    }
    if available_slots == 0 && !is_active {
        available_slots = 1;
    }
    if available_slots < kept.len() as i64 {
        kept.truncate(available_slots as usize);
        crate::parv_warn!("play", "Queue full - adding only {available_slots} tracks out of requested.");
    }

    let mention = mention_of(m);
    let mut first = kept[0].clone();
    first.requester = mention.clone();
    first.video = opts.video;

    let mut path = String::new();
    let mut was_downloaded = false;
    if !is_active || opts.force {
        let title = escape_html(&short_title(&first.title, 25));
        let dl_markup = (first.duration > 600).then(|| cancel_keyboard(loc, &crate::db::language(chat_id)));
        lm.eor(
            bot,
            &f_args(loc, chat_id, "play_downloading_song", &[("title", &title)]),
            dl_markup.as_ref(),
        );
        crate::parv_info!("play", "download start: {}", first.title);
        let mut dl_err = String::new();
        path = sink.download_track(chat_id, &first, &mut dl_err);
        crate::parv_info!("play", "download done: path={} err={}", path.len(), dl_err);
        if dl_err == "canceled" {
            drop(guard);
            lm.eor(bot, &f_args(loc, chat_id, "play_download_canceled", &[("user", &mention)]), None);
            return;
        }
        if path.is_empty() {
            crate::parv_error!("play", "Download failed: {dl_err}");
            drop(guard);
            lm.eor(bot, &f(loc, chat_id, "play_download_failed"), None);
            return;
        }
        was_downloaded = true;
        crate::parv_info!("play", "Downloaded track to {path}");
    }

    drop(guard);
    let mut played = false;
    let mut play_err = String::new();
    for attempt in 1..=3 {
        // Re-lock per attempt: the error handler drops rooms, and a held room
        // lock would deadlock drop_room (std Mutex is not reentrant).
        let destroyed = {
            let guard = room.lock().unwrap_or_else(|e| e.into_inner());
            guard.destroyed()
        };
        if destroyed {
            crate::parv_info!("play", "Room destroyed during retry, aborting");
            return;
        }
        play_err.clear();
        let mut action = room
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .play(first.clone(), if was_downloaded { path.clone() } else { String::new() }, opts.force);
 // RoomState::play returns true for queued AND started; only a
        // native-stream failure makes it false (with the err filled in).
        played = matches!(action, PlayAction::Started | PlayAction::Queued);
        if played && was_downloaded {
 // parity: play_native fires inside play when a path was
            // downloaded, inside the retry loop so a native failure retries
            // the pair (data play + streaming) the same way.
            let native = {
                let mut guard = room.lock().unwrap_or_else(|e| e.into_inner());
                match assistants {
                    Some(a) => crate::monitor::play_native_at(room_id, &mut guard, a, &path, 0, 1.0, first.video),
                    None => Err("no assistant configured for this chat".to_string()),
                }
            };
            if let Err(e) = native {
                play_err = e;
 // play rolls the track back before returning false.
                room.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .roll_back_failed_start();
                action = PlayAction::Failed;
                played = false;
            }
        }
        if played {
            if attempt > 1 {
                crate::parv_info!("play", "Successfully played after retry attempt {attempt}");
            }
            break;
        }
        if !handle_play_attempt_error(&play_err, attempt, chat_id, &mut lm, bot) {
            return;
        }
    }
    if !played {
        // Nothing matched or retries ran out: report the underlying reason.
        let why = if play_err.is_empty() { "playback failed" } else { &play_err };
        crate::parv_error!("play", "Failed to play after 3 attempts. Error: {why}");
        lm.eor(bot, &f_args(loc, chat_id, "play_failed", &[("error", why)]), None);
        return;
    }

    if !is_active || opts.force {
        let text = now_playing_text(loc, chat_id, &first);
        let mut guard = room.lock().unwrap_or_else(|e| e.into_inner());
        show_track(&mut lm, bot, chat_id, &mut guard, &first, &text, false);
        if kept.len() > 1 {
            for t in kept.iter().skip(1) {
                let mut t = t.clone();
                t.requester = mention.clone();
                t.video = opts.video;
                guard.play(t, String::new(), false);
            }
            drop(guard);
            let text = multi_added_text(chat_id, kept.len() as i64 - 1, &mention, available_slots, kept.len() as i64);
            let _ = bot.send_text_sync(chat_id, &text, None, lm.id);
        }
        let _ = rooms;
        return;
    }

    // `first` was already queued by the retry loop above when something was
    // playing; queueing it again here would add the same song twice.
    let mut guard = room.lock().unwrap_or_else(|e| e.into_inner());
    if kept.len() == 1 {
        let text = f_args(
            loc,
            chat_id,
            "play_added_to_queue_single",
            &[
                ("index", &guard.queue().len().to_string()),
                ("url", &first.url),
                ("title", &escape_html(&short_title(&first.title, 25))),
                ("duration", &crate::utils::format_duration(first.duration)),
                ("by", &mention),
            ],
        );
        // Queued reply only: edit the /play message, never the now-playing card.
        show_track_queued(&mut lm, bot, chat_id, &guard, &text);
        return;
    }
    for t in kept.iter().skip(1) {
        let mut t = t.clone();
        t.requester = mention.clone();
        t.video = opts.video;
        guard.play(t, String::new(), false);
    }
    drop(guard);
    lm.eor(
        bot,
        &multi_added_text(chat_id, kept.len() as i64, &mention, available_slots, kept.len() as i64),
        None,
    );
}

fn play_plain(bot: &mut dyn Bot, m: &TgMessage) {
    handle_play(bot, m, PlayOpts::default(), &RealSink);
}
fn play_force(bot: &mut dyn Bot, m: &TgMessage) {
    handle_play(bot, m, PlayOpts { force: true, ..Default::default() }, &RealSink);
}
fn play_cplay(bot: &mut dyn Bot, m: &TgMessage) {
    handle_play(bot, m, PlayOpts { cplay: true, ..Default::default() }, &RealSink);
}
fn play_cplay_force(bot: &mut dyn Bot, m: &TgMessage) {
    handle_play(bot, m, PlayOpts { force: true, cplay: true, ..Default::default() }, &RealSink);
}
fn play_vplay(bot: &mut dyn Bot, m: &TgMessage) {
    handle_play(bot, m, PlayOpts { video: true, ..Default::default() }, &RealSink);
}
fn play_vplay_force(bot: &mut dyn Bot, m: &TgMessage) {
    handle_play(bot, m, PlayOpts { force: true, video: true, ..Default::default() }, &RealSink);
}
fn play_vcplay(bot: &mut dyn Bot, m: &TgMessage) {
    handle_play(bot, m, PlayOpts { cplay: true, video: true, ..Default::default() }, &RealSink);
}
fn play_vcplay_force(bot: &mut dyn Bot, m: &TgMessage) {
    handle_play(bot, m, PlayOpts { force: true, cplay: true, video: true }, &RealSink);
}

/// play aliases (order preserved).
pub fn register(r: &mut Registry) {
    r.command("play", play_plain);
    r.command("fplay", play_force);
    r.command("playforce", play_force);
    r.command("cplay", play_cplay);
    r.command("cfplay", play_cplay_force);
    r.command("fcplay", play_cplay_force);
    r.command("cforceplay", play_cplay_force);
    r.command("cplayforce", play_cplay_force);
    r.command("vplay", play_vplay);
    r.command("fvplay", play_vplay_force);
    r.command("vfplay", play_vplay_force);
    r.command("vplayforce", play_vplay_force);
    r.command("vcplay", play_vcplay);
    r.command("cvplay", play_vcplay);
    r.command("fvcplay", play_vcplay_force);
    r.command("vcplayforce", play_vcplay_force);
}
