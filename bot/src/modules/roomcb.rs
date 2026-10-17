//! Room card callbacks: room:{id}:{action} playback transport, the download
//! cancel button, and the card close button.
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use parvati_mtproto::messaging::{TgCallback, TgMessage};

use crate::db;
use crate::modules::common::{card_edit, f, f_args, send_now_playing};
use crate::modules::play::PlaySink;
use crate::modules::{loc, Registry};
use crate::mod_tg::Bot;
use crate::utils::{format_duration, html_mention, short_title, trim};

pub fn register(r: &mut Registry) {
    r.callback("room:", |b, cb| handle_room_cb(b, cb, &super::play::RealSink));
    r.callback("close", handle_close);
    r.callback("cancel", handle_cancel);
}

fn cb_mention(cb: &TgCallback) -> String {
    let full = format!("{} {}", cb.first_name, cb.last_name);
    let name = trim(&full);
    html_mention(if name.is_empty() { "user" } else { name }, cb.user_id)
}

/// Minimal message facade so card helpers can reply relative to the card.
fn cb_as_message(cb: &TgCallback) -> TgMessage {
    TgMessage {
        chat_id: cb.chat_id,
        id: cb.message_id,
        sender_id: cb.user_id,
        text: String::new(),
        first_name: cb.first_name.clone(),
        last_name: cb.last_name.clone(),
        is_private: false,
        is_channel: false,
        reply_to: 0,
        ..Default::default()
    }
}

/// Owner/sudo/authorized check for card presses (alert text flips with the
/// admins-only mode); runs before the flood guard charges the press.
fn room_cb_authorized(bot: &mut dyn Bot, cb: &TgCallback) -> bool {
    let owner = crate::modules::cfg().owner_id;
    if (owner != 0 && cb.user_id == owner) || db::is_sudo(cb.user_id) || db::is_authorized(cb.chat_id, cb.user_id)
    {
        return true;
    }
    let l = loc();
    let key = if db::play_mode_admins_only(cb.chat_id) {
        "only_admin_cb"
    } else {
        "only_admin_or_auth_cb"
    };
    let _ = bot.answer_callback(cb.query_id, &f(l, cb.chat_id, key), true);
    false
}

static FLOOD_WINDOW_SECS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(3);

/// Card-mash de-dup window; tests collapse it to 0.
pub fn set_flood_window(secs: u64) {
    FLOOD_WINDOW_SECS.store(secs, std::sync::atomic::Ordering::SeqCst);
}

/// Per-(user, chat) de-dup for card mashing (3s in production).
fn room_flood_check(bot: &mut dyn Bot, cb: &TgCallback) -> bool {
    static UNTIL: std::sync::LazyLock<Mutex<HashMap<String, Instant>>> =
        std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
    let key = format!("{}:{}", cb.user_id, cb.chat_id);
    let window = FLOOD_WINDOW_SECS.load(std::sync::atomic::Ordering::SeqCst);
    if window == 0 {
        return false;
    }
    let now = Instant::now();
    let mut g = UNTIL.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(until) = g.get(&key) {
        if *until > now {
            let secs = until.duration_since(now).as_secs() + 1;
            let _ = bot.answer_callback(
                cb.query_id,
                &f_args(loc(), cb.chat_id, "flood_seconds", &[("duration", &secs.to_string())]),
                true,
            );
            return true;
        }
    }
    g.insert(key, now + std::time::Duration::from_secs(3));
    false
}

/// handle_close: dismiss the card.
pub fn handle_close(bot: &mut dyn Bot, cb: &TgCallback) {
    let _ = bot.answer_callback(cb.query_id, "", false);
    let _ = bot.delete_message(cb.chat_id, cb.message_id);
}

/// handle_cancel: abort the chat's in-flight download.
pub fn handle_cancel(bot: &mut dyn Bot, cb: &TgCallback) {
    let l = loc();
    if !db::is_authorized(cb.chat_id, cb.user_id) && !db::is_sudo(cb.user_id) {
        let _ = bot.answer_callback(cb.query_id, &f(l, cb.chat_id, "only_admin_or_auth_cb"), true);
        return;
    }
    if crate::modules::common::cancel_download(cb.chat_id) {
        let _ = bot.answer_callback(cb.query_id, &f(l, cb.chat_id, "download_cancelled"), true);
    } else {
        let _ = bot.answer_callback(cb.query_id, &f(l, cb.chat_id, "no_download_to_cancel"), true);
    }
}

/// In-place card refresh after a transport state change (keeps the photo
/// card a photo card).
fn update_playback_message(bot: &mut dyn Bot, cb: &TgCallback, r: &crate::room::RoomState, state: &str) {
    let Some(t) = r.track.as_ref() else { return };
    let l = loc();
    let chat_id = cb.chat_id;
    let safe_title = crate::utils::escape_html(&short_title(&t.title, 25));
    let user = cb_mention(cb);
    let text = if state == "paused" {
        f_args(
            l,
            chat_id,
            "cb_pause_message",
            &[
                ("url", &t.url),
                ("title", &safe_title),
                ("position", &format_duration(r.position)),
                ("duration", &format_duration(t.duration)),
                ("user", &user),
            ],
        )
    } else if state == "muted" {
        f_args(l, chat_id, "cb_mute_message", &[("url", &t.url), ("title", &safe_title), ("user", &user)])
    } else {
        f_args(
            l,
            chat_id,
            "cb_resume_message",
            &[("url", &t.url), ("title", &safe_title), ("duration", &format_duration(t.duration)), ("user", &user)],
        )
    };
    let photo_card = cb.message_id == r.status_msg && r.status_is_photo;
    let mk = crate::buttons::play_markup(l, &crate::db::language(chat_id), &r.view(), false);
    card_edit(bot, chat_id, cb.message_id, &text, Some(&mk), photo_card);
}

/// handle_room_cb: the now-playing card's transport buttons.
pub fn handle_room_cb(bot: &mut dyn Bot, cb: &TgCallback, sink: &dyn PlaySink) {
    let l = loc();
    let chat_id = cb.chat_id;
    let data = &cb.data;
    let p1 = match data.find(':') {
        Some(p) => p,
        None => {
            let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "invalid_request"), true);
            let _ = bot.delete_message(chat_id, cb.message_id);
            return;
        }
    };
    let p2 = match data[p1 + 1..].find(':') {
        Some(p) => p1 + 1 + p,
        None => {
            let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "invalid_request"), true);
            let _ = bot.delete_message(chat_id, cb.message_id);
            return;
        }
    };
    let room_id_str = &data[p1 + 1..p2];
    let action = &data[p2 + 1..];
    let Ok(room_id) = room_id_str.parse::<i64>() else {
        let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "invalid_request"), true);
        let _ = bot.delete_message(chat_id, cb.message_id);
        return;
    };

    let Some(rooms) = crate::modules::rooms() else {
        let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "room_not_active_cb"), true);
        card_edit(bot, chat_id, cb.message_id, &f(l, chat_id, "room_no_active"), None, false);
        return;
    };
    let Some(r) = rooms.room_for(room_id) else {
        let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "room_not_active_cb"), true);
        card_edit(bot, chat_id, cb.message_id, &f(l, chat_id, "room_no_active"), None, false);
        return;
    };
    {
        let g = r.lock().unwrap_or_else(|e| e.into_inner());
        if !g.active() {
            let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "room_not_active_cb"), true);
            card_edit(bot, chat_id, cb.message_id, &f(l, chat_id, "room_no_active"), None, false);
            return;
        }
    }
    if !room_cb_authorized(bot, cb) {
        return;
    }
    if room_flood_check(bot, cb) {
        return;
    }

    if action.starts_with("seek") {
        let is_back = action.starts_with("seekback_");
        let num_part = &action[if is_back { 9 } else { 5 }..];
        let Ok(seconds) = num_part.parse::<i64>() else {
            let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "invalid_request"), true);
            return;
        };
        let mut g = r.lock().unwrap_or_else(|e| e.into_inner());
        let Some(t) = g.track.clone() else {
            let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "room_not_active_cb"), true);
            return;
        };
        if !g.active() {
            let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "room_not_active_cb"), true);
            return;
        }
        if is_back {
            if g.position <= seconds {
                let pos = g.position;
                g.seek(-pos);
            } else {
                g.seek(-seconds);
            }
            if let Err(e) = crate::monitor::engine_restart(g.id, &mut g) {
                let _ = bot.answer_callback(
                    cb.query_id,
                    &f_args(l, chat_id, "cb_seek_failed", &[("error", &e)]),
                    true,
                );
                return;
            }
            let _ = bot.answer_callback(
                cb.query_id,
                &f_args(l, chat_id, "cb_seekback_success", &[("seconds", &seconds.to_string())]),
                true,
            );
            let _ = bot.reply_text(
                &cb_as_message(cb),
                &f_args(
                    l,
                    chat_id,
                    "cb_seekback_edited",
                    &[("seconds", &seconds.to_string()), ("user", &cb_mention(cb))],
                ),
                None,
            );
        } else {
            if t.duration - g.position <= seconds {
                let _ = bot.answer_callback(
                    cb.query_id,
                    &f_args(l, chat_id, "cb_seek_near_end", &[("seconds", &seconds.to_string())]),
                    true,
                );
                return;
            }
            g.seek(seconds);
            if let Err(e) = crate::monitor::engine_restart(g.id, &mut g) {
                let _ = bot.answer_callback(
                    cb.query_id,
                    &f_args(l, chat_id, "cb_seek_failed", &[("error", &e)]),
                    true,
                );
                return;
            }
            let _ = bot.answer_callback(
                cb.query_id,
                &f_args(l, chat_id, "cb_seek_success", &[("seconds", &seconds.to_string())]),
                true,
            );
            let _ = bot.reply_text(
                &cb_as_message(cb),
                &f_args(
                    l,
                    chat_id,
                    "cb_seek_edited",
                    &[("seconds", &seconds.to_string()), ("user", &cb_mention(cb))],
                ),
                None,
            );
        }
        return;
    }

    match action {
        "pause" => {
            let mut g = r.lock().unwrap_or_else(|e| e.into_inner());
            if g.paused {
                let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "room_already_paused"), true);
                return;
            }
            // Silence the actual stream; the state flag alone changes nothing.
            if !crate::monitor::engine_pause(g.id) {
                let _ = bot.answer_callback(
                    cb.query_id,
                    &f_args(l, chat_id, "room_pause_failed", &[("error", "pause")]),
                    true,
                );
                return;
            }
            if !g.pause() {
                let _ = bot.answer_callback(
                    cb.query_id,
                    &f_args(l, chat_id, "room_pause_failed", &[("error", "pause")]),
                    true,
                );
                return;
            }
            if g.muted {
                g.unmute();
            }
            let _ = bot.answer_callback(
                cb.query_id,
                &f_args(l, chat_id, "cb_pause_success", &[("position", &format_duration(g.position))]),
                true,
            );
            update_playback_message(bot, cb, &g, "paused");
        }
        "resume" => {
            let mut g = r.lock().unwrap_or_else(|e| e.into_inner());
            if !g.paused {
                let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "cb_already_playing"), true);
                return;
            }
            // Un-silence the actual stream before flipping the state flag.
            if !crate::monitor::engine_resume(g.id) {
                let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "cb_resume_failed"), true);
                return;
            }
            if !g.resume() {
                let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "cb_resume_failed"), true);
                return;
            }
            let _ = bot.answer_callback(
                cb.query_id,
                &f_args(l, chat_id, "cb_resume_success", &[("position", &format_duration(g.position))]),
                true,
            );
            update_playback_message(bot, cb, &g, "playing");
        }
        "replay" => {
            let _ = bot.reply_text(&cb_as_message(cb), &f(l, chat_id, "cb_replaying"), None);
            let mut g = r.lock().unwrap_or_else(|e| e.into_inner());
            if !g.replay() {
                let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "cb_replay_failed"), true);
                return;
            }
            let Some(t) = g.track.clone() else {
                let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "cb_replay_failed"), true);
                return;
            };
            if let Err(e) = crate::monitor::engine_restart(g.id, &mut g) {
                let _ = bot.answer_callback(
                    cb.query_id,
                    &f_args(l, chat_id, "cb_replay_failed", &[("error", &e)]),
                    true,
                );
                return;
            }
            send_now_playing(bot, chat_id, &mut g, &t, l);
            let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "cb_replay_success"), true);
            let photo_card = cb.message_id == g.status_msg && g.status_is_photo;
            card_edit(
                bot,
                chat_id,
                cb.message_id,
                &f_args(l, chat_id, "cb_replay_edited", &[("user", &cb_mention(cb))]),
                None,
                photo_card,
            );
        }
        "skip" => {
            let mut g = r.lock().unwrap_or_else(|e| e.into_inner());
            if g.queue.is_empty() {
                let id = g.id;
                drop(g);
                rooms.drop_room(id);
                card_edit(
                    bot,
                    chat_id,
                    cb.message_id,
                    &f_args(l, chat_id, "skip_stopped", &[("user", &cb_mention(cb))]),
                    None,
                    false,
                );
                let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "cb_skip_queue_empty"), true);
                return;
            }
            g.set_loop(0);
            let Some(t) = g.next_track() else {
                let id = g.id;
                drop(g);
                rooms.drop_room(id);
                card_edit(
                    bot,
                    chat_id,
                    cb.message_id,
                    &f_args(l, chat_id, "skip_stopped", &[("user", &cb_mention(cb))]),
                    None,
                    false,
                );
                let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "cb_skip_queue_empty"), true);
                return;
            };
            let _ = bot.reply_text(&cb_as_message(cb), &f(l, chat_id, "stream_downloading_next"), None);
            let mut dl_err = String::new();
            let path = sink.download_track(chat_id, &t, &mut dl_err);
            if path.is_empty() {
                let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "cb_skip_download_failed"), true);
                let id = g.id;
                drop(g);
                rooms.drop_room(id);
                return;
            }
            if let crate::room::PlayAction::Failed = g.play(t.clone(), path.clone(), true) {
                let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "cb_skip_play_failed"), true);
                let id = g.id;
                drop(g);
                rooms.drop_room(id);
                return;
            }
            // Actually switch the voice stream, not just the room state.
            let video = t.video;
            let started = match crate::modules::assistants() {
                Some(assists) => crate::monitor::start_track(g.id, &mut g, assists, &path, video),
                None => false,
            };
            if !started {
                let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "cb_skip_play_failed"), true);
                let id = g.id;
                drop(g);
                rooms.drop_room(id);
                return;
            }
            let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "cb_skip_success"), true);
            let _ = bot.delete_message(chat_id, cb.message_id);
            send_now_playing(bot, chat_id, &mut g, &t, l);
            // cb_skip_edited rides on the fresh card (or the old id).
            let ref_id = if g.status_msg != 0 { g.status_msg } else { cb.message_id };
            let mut refm = cb_as_message(cb);
            refm.id = ref_id;
            let _ = bot.reply_text(
                &refm,
                &f_args(l, chat_id, "cb_skip_edited", &[("user", &cb_mention(cb))]),
                None,
            );
        }
        "stop" => {
            let id = r.lock().unwrap_or_else(|e| e.into_inner()).id;
            rooms.drop_room(id);
            let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "cb_stop_success"), true);
            card_edit(
                bot,
                chat_id,
                cb.message_id,
                &f_args(l, chat_id, "stopped", &[("user", &cb_mention(cb))]),
                None,
                false,
            );
        }
        "mute" => {
            let mut g = r.lock().unwrap_or_else(|e| e.into_inner());
            if g.muted {
                let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "mute_already_muted"), true);
                return;
            }
            if !crate::monitor::engine_mute(g.id) {
                let _ = bot.answer_callback(
                    cb.query_id,
                    &f_args(l, chat_id, "mute_failed", &[("error", "mute")]),
                    true,
                );
                return;
            }
            if !g.mute() {
                let _ = bot.answer_callback(
                    cb.query_id,
                    &f_args(l, chat_id, "mute_failed", &[("error", "mute")]),
                    true,
                );
                return;
            }
            let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "cb_mute_success"), true);
            update_playback_message(bot, cb, &g, "muted");
        }
        "unmute" => {
            let mut g = r.lock().unwrap_or_else(|e| e.into_inner());
            if !g.muted {
                let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "unmute_already"), true);
                return;
            }
            if !crate::monitor::engine_unmute(g.id) {
                let _ = bot.answer_callback(
                    cb.query_id,
                    &f_args(l, chat_id, "unmute_failed", &[("error", "unmute")]),
                    true,
                );
                return;
            }
            if !g.unmute() {
                let _ = bot.answer_callback(
                    cb.query_id,
                    &f_args(l, chat_id, "unmute_failed", &[("error", "unmute")]),
                    true,
                );
                return;
            }
            let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "cb_unmute_success"), true);
            update_playback_message(bot, cb, &g, "playing");
        }
        "autoplay_toggle" => {
            let mut g = r.lock().unwrap_or_else(|e| e.into_inner());
            let enabled = !g.autoplay;
            g.set_autoplay(enabled);
            db::set_autoplay(g.chat_id, enabled);
            let _ = bot.answer_callback(
                cb.query_id,
                &f(l, chat_id, if enabled { "autoplay_enabled" } else { "autoplay_disabled" }),
                false,
            );
            let mk = crate::buttons::play_markup(l, &crate::db::language(chat_id), &g.view(), false);
            let _ = bot.edit_markup(chat_id, cb.message_id, Some(&mk), false);
        }
        _ => {
            let _ = bot.answer_callback(cb.query_id, &f(l, chat_id, "unknown_action"), true);
        }
    }
}
