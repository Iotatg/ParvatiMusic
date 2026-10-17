//! VC control commands: position/replay/loop/speed/shuffle/seek/jump/
//! remove/clear/move/mute/unmute/askip/active.

use std::collections::HashMap;
use std::sync::Mutex;

use parvati_mtproto::messaging::TgMessage;

use crate::db;
use crate::models::Track;
use crate::modules::common::{command_of, f, f_args, mention_of, now_playing_text, show_track, LiveMsg};
use crate::modules::play::PlaySink;
use crate::modules::{loc, rooms, Registry};
use crate::mod_tg::Bot;
use crate::room::RoomState;
use crate::utils::{escape_html, format_duration, short_title};

/// constructor for the registered wrappers (RealSink + real autoplay pool).
pub fn register(r: &mut Registry) {
    r.command("position", |b, m| handle_position(b, m, false));
    r.command("pos", |b, m| handle_position(b, m, false));
    r.command("cposition", |b, m| handle_position(b, m, true));
    r.command("cpos", |b, m| handle_position(b, m, true));
    r.command("replay", |b, m| handle_replay(b, m, false));
    r.command("creplay", |b, m| handle_replay(b, m, true));
    r.command("loop", |b, m| handle_loop(b, m, false));
    r.command("setloop", |b, m| handle_loop(b, m, false));
    r.command("cloop", |b, m| handle_loop(b, m, true));
    r.command("csetloop", |b, m| handle_loop(b, m, true));
    r.command("speed", |b, m| handle_speed(b, m, false));
    r.command("setspeed", |b, m| handle_speed(b, m, false));
    r.command("speedup", |b, m| handle_speed(b, m, false));
    r.command("cspeed", |b, m| handle_speed(b, m, true));
    r.command("csetspeed", |b, m| handle_speed(b, m, true));
    r.command("cspeedup", |b, m| handle_speed(b, m, true));
    r.command("shuffle", |b, m| handle_shuffle(b, m, false));
    r.command("cshuffle", |b, m| handle_shuffle(b, m, true));
    r.command("seek", |b, m| handle_seek(b, m, false, false));
    r.command("cseek", |b, m| handle_seek(b, m, true, false));
    r.command("seekback", |b, m| handle_seek(b, m, false, true));
    r.command("cseekback", |b, m| handle_seek(b, m, true, true));
    r.command("jump", |b, m| handle_jump(b, m, false));
    r.command("cjump", |b, m| handle_jump(b, m, true));
    r.command("remove", |b, m| handle_remove(b, m, false));
    r.command("cremove", |b, m| handle_remove(b, m, true));
    r.command("clear", |b, m| handle_clear(b, m, false));
    r.command("cclear", |b, m| handle_clear(b, m, true));
    r.command("move", |b, m| handle_move(b, m, false));
    r.command("cmove", |b, m| handle_move(b, m, true));
    r.command("mute", |b, m| handle_mute(b, m, false));
    r.command("cmute", |b, m| handle_mute(b, m, true));
    r.command("unmute", |b, m| handle_unmute(b, m, false));
    r.command("cunmute", |b, m| handle_unmute(b, m, true));
    r.command("askip", askip_plain);
    r.command("caskip", askip_cplay);
    r.command("ac", handle_active);
    r.command("active", handle_active);
    r.command("activevc", handle_active);
    r.command("activevoice", handle_active);
}

/// registry closures are plain fns in Registry.command? no: command takes a
/// fn pointer, so the sink-bearing commands route through these wrappers.
fn askip_plain(bot: &mut dyn Bot, m: &TgMessage) {
    handle_autoplay_skip(bot, m, false, &super::play::RealSink, real_pool);
}

fn askip_cplay(bot: &mut dyn Bot, m: &TgMessage) {
    handle_autoplay_skip(bot, m, true, &super::play::RealSink, real_pool);
}

/// The real autoplay pool comes from the platform registry.
fn real_pool(last: &Track, limit: i64, history: &[String]) -> Vec<Track> {
    crate::platforms::registry::autoplay_tracks(last, limit, history)
}

/// Pool seam for autoplay-skip: candidate tracks for the room's last track.
pub type AutoplayPool = fn(&Track, i64, &[String]) -> Vec<Track>;

fn fmt_speed(v: f64) -> String {
    format!("{v:.2}")
}

fn parse_int_strict(s: &str) -> Option<i32> {
    s.parse::<i32>().ok()
}

fn parse_double_strict(s: &str) -> Option<f64> {
    match s.parse::<f64>() {
        Ok(v) if v.is_finite() => Some(v),
        _ => None,
    }
}

fn is_owner_or_sudo(user_id: i64) -> bool {
    let owner = crate::modules::cfg().owner_id;
    if owner != 0 && user_id == owner {
        return true;
    }
    db::is_sudo(user_id)
}

fn can_use_admin_command(m: &TgMessage) -> bool {
    if is_owner_or_sudo(m.sender_id) {
        return true;
    }
    db::is_authorized(m.chat_id, m.sender_id)
}

/// Auth gate for the VC control family: privileged or authorized users pass;
/// everyone else is told whether the room is admins-only.
fn filter_auth_users(bot: &mut dyn Bot, m: &TgMessage) -> bool {
    if can_use_admin_command(m) {
        return true;
    }
    let l = loc();
    let key = if db::play_mode_admins_only(m.chat_id) {
        "only_admin"
    } else {
        "only_admin_or_auth"
    };
    let _ = bot.reply_text(m, &f(l, m.chat_id, key), None);
    false
}

/// Per-chat shuffle mirror (the room exposes a one-shot shuffle; the flag is
/// the durable state the speed of following reorders depends on).
static SHUFFLE_FLAGS: std::sync::LazyLock<Mutex<HashMap<i64, bool>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

fn shuffle_flag(chat_id: i64) -> bool {
    SHUFFLE_FLAGS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&chat_id)
        .copied()
        .unwrap_or(false)
}

fn set_shuffle_flag(chat_id: i64, v: bool) {
    SHUFFLE_FLAGS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(chat_id, v);
}

/// Resolve the room without creating one: cplay mirrors onto the linked
/// channel; errors carry the cplay notice when relevant, else room_no_active.
fn resolve_room(m: &TgMessage, cplay: bool) -> Result<std::sync::Arc<Mutex<RoomState>>, String> {
    let l = loc();
    let chat_id = m.chat_id;
    let room_id = if cplay {
        let linked = db::linked_channel(chat_id);
        if linked == 0 {
            return Err(f(l, chat_id, "cplay_id_not_set"));
        }
        linked
    } else {
        chat_id
    };
    match crate::modules::rooms() {
        Some(rs) => rs.room_for(room_id).ok_or_else(|| f(l, m.chat_id, "room_no_active")),
        None => Err(f(l, m.chat_id, "room_no_active")),
    }
}

/// Locks the resolved room or replies the resolution error.
macro_rules! guard_or {
    ($bot:expr, $m:expr, $cplay:expr, mut $name:ident) => {
        let $name = match resolve_room($m, $cplay) {
            Ok(r) => r,
            Err(e) => {
                let _ = $bot.reply_text($m, &e, None);
                return;
            }
        };
        let mut $name = $name.lock().unwrap_or_else(|e| e.into_inner());
    };
    ($bot:expr, $m:expr, $cplay:expr, $name:ident) => {
        let $name = match resolve_room($m, $cplay) {
            Ok(r) => r,
            Err(e) => {
                let _ = $bot.reply_text($m, &e, None);
                return;
            }
        };
        let $name = $name.lock().unwrap_or_else(|e| e.into_inner());
    };
}

fn only_supergroup(bot: &mut dyn Bot, m: &TgMessage) -> bool {
    if m.is_private || m.is_channel {
        let _ = bot.reply_text(m, &f(loc(), m.chat_id, "only_supergroup"), None);
        return false;
    }
    true
}

/// handle_position.
pub fn handle_position(bot: &mut dyn Bot, m: &TgMessage, cplay: bool) {
    if !only_supergroup(bot, m) {
        return;
    }
    if cplay && !filter_auth_users(bot, m) {
        return;
    }
    let l = loc();
    let chat_id = m.chat_id;
    guard_or!(bot, m, cplay, r);
        let Some(t) = r.track.clone() else {
            let _ = bot.reply_text(m, &f(l, chat_id, "room_no_active"), None);
            return;
        };
        if !r.active() || t.id.is_empty() {
            let _ = bot.reply_text(m, &f(l, chat_id, "room_no_active"), None);
            return;
        }
        let text = f_args(
            l,
            chat_id,
            "position_now",
            &[
                ("title", &escape_html(&short_title(&t.title, 25))),
                ("position", &format_duration(r.position)),
                ("duration", &format_duration(t.duration)),
                ("speed", &fmt_speed(r.speed)),
            ],
        );
        let _ = bot.reply_text(m, &text, None);
}

/// handle_replay.
pub fn handle_replay(bot: &mut dyn Bot, m: &TgMessage, cplay: bool) {
    if !only_supergroup(bot, m) {
        return;
    }
    if !filter_auth_users(bot, m) {
        return;
    }
    let l = loc();
    let chat_id = m.chat_id;
    guard_or!(bot, m, cplay, mut r);
        if !r.active() {
            let _ = bot.reply_text(m, &f(l, chat_id, "room_no_active"), None);
            return;
        }
        let (title, duration) = match r.track.as_ref() {
            Some(t) => (escape_html(&short_title(&t.title, 25)), format_duration(t.duration)),
            None => (String::new(), format_duration(0)),
        };
        if !r.replay() {
            let _ = bot.reply_text(m, &f_args(l, chat_id, "replay_failed", &[("error", "replay")]), None);
            return;
        }
        let speed = fmt_speed(r.speed);
        let text = f_args(
            l,
            chat_id,
            "replay_success",
            &[("title", &title), ("duration", &duration), ("speed", &speed)],
        );
        let _ = bot.reply_text(m, &text, None);
}

/// handle_loop.
pub fn handle_loop(bot: &mut dyn Bot, m: &TgMessage, cplay: bool) {
    if !only_supergroup(bot, m) {
        return;
    }
    if !filter_auth_users(bot, m) {
        return;
    }
    let l = loc();
    let chat_id = m.chat_id;
    guard_or!(bot, m, cplay, mut r);
        let fields: Vec<&str> = m.text.split_whitespace().collect();
        let current = r.loop_n;

        if !r.active() {
            let _ = bot.reply_text(m, &f(l, chat_id, "room_no_active"), None);
            return;
        }

        if fields.len() < 2 {
            let count_line = if current > 0 {
                format!("\n{}", f_args(l, chat_id, "loop_current", &[("count", &current.to_string())]))
            } else {
                String::new()
            };
            let cmd = command_of(m);
            let text = f_args(
                l,
                chat_id,
                "loop_usage",
                &[("cmd", &cmd), ("count_line", &count_line)],
            );
            let _ = bot.reply_text(m, &text, None);
            return;
        }

        let new_loop = match parse_int_strict(fields[1]) {
            Some(v) if (0..=10).contains(&v) => v as i64,
            _ => {
                let _ = bot.reply_text(m, &f(l, chat_id, "loop_invalid"), None);
                return;
            }
        };

        if new_loop == current {
            let _ = bot.reply_text(m, &f_args(l, chat_id, "loop_already_set", &[("count", &current.to_string())]), None);
            return;
        }

        r.set_loop(new_loop);
        let text = if new_loop == 0 {
            f_args(l, chat_id, "loop_disabled", &[("user", &mention_of(m))])
        } else {
            f_args(
                l,
                chat_id,
                "loop_set",
                &[("count", &new_loop.to_string()), ("user", &mention_of(m))],
            )
        };
        let _ = bot.reply_text(m, &text, None);
}

/// handle_speed.
pub fn handle_speed(bot: &mut dyn Bot, m: &TgMessage, cplay: bool) {
    if !only_supergroup(bot, m) {
        return;
    }
    if !filter_auth_users(bot, m) {
        return;
    }
    let l = loc();
    let chat_id = m.chat_id;
    guard_or!(bot, m, cplay, mut r);
        let Some(t) = r.track.clone() else {
            let _ = bot.reply_text(m, &f(l, chat_id, "room_no_active"), None);
            return;
        };
        if !r.active() {
            let _ = bot.reply_text(m, &f(l, chat_id, "room_no_active"), None);
            return;
        }

        let fields: Vec<&str> = m.text.split_whitespace().collect();
        let cmd = command_of(m);

        if fields.len() < 2 {
            if r.speed != 1.0 {
                let text = f_args(
                    l,
                    chat_id,
                    "speed_current",
                    &[
                        ("speed", &fmt_speed(r.speed)),
                        ("title", &escape_html(&short_title(&t.title, 25))),
                        ("cmd", &cmd),
                    ],
                );
                let _ = bot.reply_text(m, &text, None);
            } else {
                let _ = bot.reply_text(m, &f_args(l, chat_id, "speed_usage", &[("cmd", &cmd)]), None);
            }
            return;
        }

        let mut raw = fields[1].trim().to_lowercase();
        if raw.ends_with('x') {
            raw.pop();
        }
        if raw.ends_with('×') {
            raw.pop();
        }

        let new_speed = if raw == "normal" || raw == "reset" || raw == "default" {
            1.0
        } else {
            match parse_double_strict(&raw) {
                None => {
                    let _ = bot.reply_text(m, &f_args(l, chat_id, "speed_invalid_value", &[("cmd", &cmd)]), None);
                    return;
                }
                Some(v) if !(0.50..=4.0).contains(&v) => {
                    let _ = bot.reply_text(m, &f(l, chat_id, "speed_invalid_range"), None);
                    return;
                }
                Some(v) => v,
            }
        };

        if new_speed == r.speed {
            let text = f_args(
                l,
                chat_id,
                "speed_already_set",
                &[("speed", &fmt_speed(new_speed)), ("title", &escape_html(&short_title(&t.title, 25)))],
            );
            let _ = bot.reply_text(m, &text, None);
            return;
        }

        if r.set_speed(new_speed).is_none() {
            let text = f_args(
                l,
                chat_id,
                "speed_failed",
                &[("speed", &fmt_speed(new_speed)), ("error", "speed")],
            );
            let _ = bot.reply_text(m, &text, None);
            return;
        }

        // atempo is baked into the ffmpeg source, so restart at the current
        // position to make the new speed audible.
        if let Err(e) = crate::monitor::engine_restart(r.id, &mut r) {
            let text = f_args(
                l,
                chat_id,
                "speed_failed",
                &[("speed", &fmt_speed(new_speed)), ("error", &e)],
            );
            let _ = bot.reply_text(m, &text, None);
            return;
        }

        let text = if new_speed == 1.0 {
            f_args(l, chat_id, "speed_reset_success", &[("user", &mention_of(m))])
        } else {
            f_args(
                l,
                chat_id,
                "speed_set",
                &[("speed", &fmt_speed(new_speed)), ("user", &mention_of(m))],
            )
        };
        let _ = bot.reply_text(m, &text, None);
}

/// handle_shuffle: explicit on/off (toggle-free), with an immediate reorder
/// when switching on.
pub fn handle_shuffle(bot: &mut dyn Bot, m: &TgMessage, cplay: bool) {
    if !only_supergroup(bot, m) {
        return;
    }
    if !filter_auth_users(bot, m) {
        return;
    }
    let l = loc();
    let chat_id = m.chat_id;
    let arg = crate::utils::trim(m.text.strip_prefix(command_of(m).as_str()).unwrap_or(""))
        .to_lowercase();

    guard_or!(bot, m, cplay, mut r);
        if !r.active() {
            let _ = bot.reply_text(m, &f(l, chat_id, "room_no_active"), None);
            return;
        }

        let cur = shuffle_flag(chat_id);
        if arg.is_empty() {
            let state = f(l, chat_id, if cur { "enabled" } else { "disabled" });
            let sugg = format!("{}/shuffle {}", command_of(m), if cur { "off" } else { "on" });
            let text = f_args(l, chat_id, "shuffle_current_state", &[("state", &state), ("cmd", &sugg)]);
            let _ = bot.reply_text(m, &text, None);
            return;
        }

        let new_state = matches!(arg.as_str(), "on" | "enable" | "true" | "1");
        if new_state && !cur {
            r.shuffle_queue();
        }
        set_shuffle_flag(chat_id, new_state);

        let state = f(l, chat_id, if new_state { "enabled" } else { "disabled" });
        let text = f_args(l, chat_id, "shuffle_updated", &[("state", &state), ("user", &mention_of(m))]);
        let _ = bot.reply_text(m, &text, None);
}

/// handle_seek (shared by seekback).
pub fn handle_seek(bot: &mut dyn Bot, m: &TgMessage, cplay: bool, is_back: bool) {
    if !only_supergroup(bot, m) {
        return;
    }
    if !filter_auth_users(bot, m) {
        return;
    }
    let l = loc();
    let chat_id = m.chat_id;
    guard_or!(bot, m, cplay, mut r);
        let Some(t) = r.track.clone() else {
            let _ = bot.reply_text(m, &f(l, chat_id, "seek_no_active"), None);
            return;
        };
        if !r.active() {
            let _ = bot.reply_text(m, &f(l, chat_id, "seek_no_active"), None);
            return;
        }

        let fields: Vec<&str> = m.text.split_whitespace().collect();
        let cmd = command_of(m);
        if fields.len() < 2 {
            let _ = bot.reply_text(m, &f_args(l, chat_id, "seek_usage", &[("cmd", &cmd)]), None);
            return;
        }

        let seconds = match parse_int_strict(fields[1]) {
            Some(v) => v,
            None => {
                let _ = bot.reply_text(m, &f_args(l, chat_id, "seek_invalid_seconds", &[("cmd", &cmd)]), None);
                return;
            }
        };

        let (direction, emoji): (&str, &str) = if is_back {
            ("backward", "⏪")
        } else {
            ("forward", "⏩")
        };

        let ok = if is_back {
            if r.position - seconds as i64 <= 10 {
                let _ = bot.reply_text(
                    m,
                    &f_args(l, chat_id, "seek_too_close_start", &[("seconds", &seconds.to_string())]),
                    None,
                );
                return;
            }
            r.seek(-(seconds as i64))
        } else {
            if t.duration - (r.position + seconds as i64) <= 10 {
                let _ = bot.reply_text(
                    m,
                    &f_args(l, chat_id, "seek_too_close_end", &[("seconds", &seconds.to_string())]),
                    None,
                );
                return;
            }
            r.seek(seconds as i64)
        };

        match ok {
            None => {
                let text = f_args(
                    l,
                    chat_id,
                    "seek_failed",
                    &[("direction", direction), ("seconds", &seconds.to_string()), ("error", "seek")],
                );
                let _ = bot.reply_text(m, &text, None);
            }
            Some(_) => {
                // libntgcalls has no seek; restart the stream at the new position.
                if let Err(e) = crate::monitor::engine_restart(r.id, &mut r) {
                    let text = f_args(
                        l,
                        chat_id,
                        "seek_failed",
                        &[("direction", direction), ("seconds", &seconds.to_string()), ("error", &e)],
                    );
                    let _ = bot.reply_text(m, &text, None);
                    return;
                }
                let text = f_args(
                    l,
                    chat_id,
                    "seek_success",
                    &[
                        ("emoji", emoji),
                        ("direction", direction),
                        ("position", &format_duration(r.position)),
                        ("duration", &format_duration(t.duration)),
                    ],
                );
                let _ = bot.reply_text(m, &text, None);
            }
        }
}

/// handle_jump: absolute position seek.
pub fn handle_jump(bot: &mut dyn Bot, m: &TgMessage, cplay: bool) {
    if !only_supergroup(bot, m) {
        return;
    }
    if !filter_auth_users(bot, m) {
        return;
    }
    let l = loc();
    let chat_id = m.chat_id;
    guard_or!(bot, m, cplay, mut r);
        let Some(t) = r.track.clone() else {
            let _ = bot.reply_text(m, &f(l, chat_id, "jump_no_active"), None);
            return;
        };
        if !r.active() {
            let _ = bot.reply_text(m, &f(l, chat_id, "jump_no_active"), None);
            return;
        }

        let fields: Vec<&str> = m.text.split_whitespace().collect();
        let cmd = command_of(m);
        if fields.len() < 2 {
            let _ = bot.reply_text(m, &f_args(l, chat_id, "jump_usage", &[("cmd", &cmd)]), None);
            return;
        }

        let seconds = match parse_int_strict(fields[1]) {
            Some(v) if v >= 0 => v,
            _ => {
                let _ = bot.reply_text(m, &f_args(l, chat_id, "jump_invalid_position", &[("cmd", &cmd)]), None);
                return;
            }
        };

        if t.duration - seconds as i64 <= 10 {
            let _ = bot.reply_text(
                m,
                &f_args(l, chat_id, "jump_too_close_end", &[("position", &format_duration(seconds as i64))]),
                None,
            );
            return;
        }

        let delta = seconds as i64 - r.position;
        if r.seek(delta).is_none() {
            let _ = bot.reply_text(
                m,
                &f_args(
                    l,
                    chat_id,
                    "jump_failed",
                    &[("position", &format_duration(seconds as i64)), ("error", "seek")],
                ),
                None,
            );
            return;
        }
        if let Err(e) = crate::monitor::engine_restart(r.id, &mut r) {
            let _ = bot.reply_text(
                m,
                &f_args(
                    l,
                    chat_id,
                    "jump_failed",
                    &[("position", &format_duration(seconds as i64)), ("error", &e)],
                ),
                None,
            );
            return;
        }
        let text = f_args(
            l,
            chat_id,
            "jump_success",
            &[
                ("position", &format_duration(seconds as i64)),
                ("duration", &format_duration(t.duration)),
            ],
        );
        let _ = bot.reply_text(m, &text, None);
}

/// handle_remove.
pub fn handle_remove(bot: &mut dyn Bot, m: &TgMessage, cplay: bool) {
    if !only_supergroup(bot, m) {
        return;
    }
    if !filter_auth_users(bot, m) {
        return;
    }
    let l = loc();
    let chat_id = m.chat_id;
    guard_or!(bot, m, cplay, mut r);
        if !r.active() || r.track.is_none() {
            let _ = bot.reply_text(m, &f(l, chat_id, "queue_no_active"), None);
            return;
        }
        let total = r.queue.len() as i64;
        if total == 0 {
            let _ = bot.reply_text(m, &f(l, chat_id, "queue_empty"), None);
            return;
        }

        let fields: Vec<&str> = m.text.split_whitespace().collect();
        if fields.len() < 2 {
            let _ = bot.reply_text(m, &f_args(l, chat_id, "remove_usage", &[("cmd", &command_of(m))]), None);
            return;
        }

        let index = match parse_int_strict(fields[1]) {
            Some(v) => v,
            None => {
                let _ = bot.reply_text(m, &f(l, chat_id, "remove_invalid_index"), None);
                return;
            }
        };
        if index <= 0 {
            let _ = bot.reply_text(m, &f(l, chat_id, "remove_index_too_small"), None);
            return;
        }
        if index as i64 > total {
            let _ = bot.reply_text(m, &f_args(l, chat_id, "remove_index_too_big", &[("total", &total.to_string())]), None);
            return;
        }

        r.remove_at((index - 1) as usize);
        let text = f_args(
            l,
            chat_id,
            "remove_success",
            &[("index", &index.to_string()), ("user", &mention_of(m))],
        );
        let _ = bot.reply_text(m, &text, None);
}

/// handle_clear.
pub fn handle_clear(bot: &mut dyn Bot, m: &TgMessage, cplay: bool) {
    if !only_supergroup(bot, m) {
        return;
    }
    if !filter_auth_users(bot, m) {
        return;
    }
    let l = loc();
    let chat_id = m.chat_id;
    guard_or!(bot, m, cplay, mut r);
        if !r.active() || r.track.is_none() {
            let _ = bot.reply_text(m, &f(l, chat_id, "clear_no_active"), None);
            return;
        }
        if r.queue.is_empty() {
            let _ = bot.reply_text(m, &f(l, chat_id, "queue_empty"), None);
            return;
        }

        r.clear_queue();
        let _ = bot.reply_text(m, &f_args(l, chat_id, "clear_success", &[("user", &mention_of(m))]), None);
}

/// handle_move.
pub fn handle_move(bot: &mut dyn Bot, m: &TgMessage, cplay: bool) {
    if !only_supergroup(bot, m) {
        return;
    }
    if !filter_auth_users(bot, m) {
        return;
    }
    let l = loc();
    let chat_id = m.chat_id;
    guard_or!(bot, m, cplay, mut r);
        if !r.active() || r.track.is_none() {
            let _ = bot.reply_text(m, &f(l, chat_id, "queue_no_active"), None);
            return;
        }
        let queue_len = r.queue.len() as i64;
        if queue_len == 0 {
            let _ = bot.reply_text(m, &f(l, chat_id, "queue_empty"), None);
            return;
        }

        let fields: Vec<&str> = m.text.split_whitespace().collect();
        let cmd = command_of(m);
        if fields.len() < 3 {
            let _ = bot.reply_text(m, &f_args(l, chat_id, "move_usage", &[("cmd", &cmd)]), None);
            return;
        }

        let (from, to) = match (parse_int_strict(fields[1]), parse_int_strict(fields[2])) {
            (Some(fv), Some(tv)) => (fv, tv),
            _ => {
                let _ = bot.reply_text(m, &f_args(l, chat_id, "move_invalid_numbers", &[("cmd", &cmd)]), None);
                return;
            }
        };
        if from <= 0 || to <= 0 {
            let _ = bot.reply_text(m, &f(l, chat_id, "move_invalid_indexes_min"), None);
            return;
        }
        if from as i64 > queue_len || to as i64 > queue_len {
            let _ = bot.reply_text(
                m,
                &f_args(l, chat_id, "move_invalid_indexes_max", &[("queue_len", &queue_len.to_string())]),
                None,
            );
            return;
        }

        r.move_to((from - 1) as usize, (to - 1) as usize);
        let text = f_args(
            l,
            chat_id,
            "move_success",
            &[("from", &from.to_string()), ("to", &to.to_string()), ("user", &mention_of(m))],
        );
        let _ = bot.reply_text(m, &text, None);
}

/// handle_mute.
pub fn handle_mute(bot: &mut dyn Bot, m: &TgMessage, cplay: bool) {
    if !only_supergroup(bot, m) {
        return;
    }
    if !filter_auth_users(bot, m) {
        return;
    }
    if !command_args(m).is_empty() {
        return;
    }
    let l = loc();
    let chat_id = m.chat_id;
    guard_or!(bot, m, cplay, mut r);
        if !r.active() {
            let _ = bot.reply_text(m, &f(l, chat_id, "room_no_active"), None);
            return;
        }
        if r.muted {
            let _ = bot.reply_text(m, &f(l, chat_id, "mute_already_muted"), None);
            return;
        }

        let title = r.track.as_ref().map(|t| escape_html(&short_title(&t.title, 25))).unwrap_or_default();
        if !crate::monitor::engine_mute(r.id) {
            let _ = bot.reply_text(m, &f_args(l, chat_id, "mute_failed", &[("error", "mute")]), None);
            return;
        }
        if !r.mute() {
            let _ = bot.reply_text(m, &f_args(l, chat_id, "mute_failed", &[("error", "mute")]), None);
            return;
        }
        let user = mention_of(m);
        let _ = bot.reply_text(m, &f_args(l, chat_id, "mute_success", &[("title", &title), ("user", &user)]), None);
}

/// handle_unmute (arg check runs before the group guard, like the original).
pub fn handle_unmute(bot: &mut dyn Bot, m: &TgMessage, cplay: bool) {
    if !command_args(m).is_empty() {
        return;
    }
    if !only_supergroup(bot, m) {
        return;
    }
    if !filter_auth_users(bot, m) {
        return;
    }
    let l = loc();
    let chat_id = m.chat_id;
    guard_or!(bot, m, cplay, mut r);
        if !r.active() {
            let _ = bot.reply_text(m, &f(l, chat_id, "room_no_active"), None);
            return;
        }
        if !r.muted {
            let _ = bot.reply_text(m, &f(l, chat_id, "unmute_already"), None);
            return;
        }

        let title = r.track.as_ref().map(|t| escape_html(&short_title(&t.title, 25))).unwrap_or_default();
        if !crate::monitor::engine_unmute(r.id) {
            let _ = bot.reply_text(m, &f_args(l, chat_id, "unmute_failed", &[("error", "unmute")]), None);
            return;
        }
        if !r.unmute() {
            let _ = bot.reply_text(m, &f_args(l, chat_id, "unmute_failed", &[("error", "unmute")]), None);
            return;
        }

        let speed_line = if r.speed != 1.0 {
            f_args(l, chat_id, "speed_line", &[("speed", &fmt_speed(r.speed))])
        } else {
            String::new()
        };
        let user = mention_of(m);
        let _ = bot.reply_text(
            m,
            &f_args(
                l,
                chat_id,
                "unmute_success",
                &[("title", &title), ("user", &user), ("speed_line", &speed_line)],
            ),
            None,
        );
}

fn command_args(m: &TgMessage) -> String {
    let cmd = command_of(m);
    crate::utils::trim(m.text.strip_prefix(&cmd).unwrap_or("")).to_string()
}

/// handle_autoplay_skip: with autoplay on, replace the current track by a
/// registry-picked recommendation.
pub fn handle_autoplay_skip(
    bot: &mut dyn Bot,
    m: &TgMessage,
    cplay: bool,
    sink: &dyn PlaySink,
    pool: AutoplayPool,
) {
    if !only_supergroup(bot, m) {
        return;
    }
    if !filter_auth_users(bot, m) {
        return;
    }
    let l = loc();
    let chat_id = m.chat_id;

    let r = match resolve_room(m, cplay) {
        Ok(r) => r,
        Err(e) => {
            let _ = bot.reply_text(m, &e, None);
            return;
        }
    };
    let mut g = r.lock().unwrap_or_else(|e| e.into_inner());
    let all_rooms = crate::modules::rooms().cloned().unwrap_or_default();
    if !g.active() {
        let _ = bot.reply_text(m, &f(l, chat_id, "room_no_active"), None);
        return;
    }
    if !g.autoplay {
        let _ = bot.reply_text(m, &f(l, chat_id, "askip_disabled"), None);
        return;
    }
    let Some(last) = g.track.clone() else {
        let _ = bot.reply_text(m, &f(l, chat_id, "room_no_active"), None);
        return;
    };

    let mut lm = LiveMsg::new();
    lm.chat_id = chat_id;
    lm.send(bot, &f(l, chat_id, "stream_downloading_next"), None);

    let history = g.played_history.clone();
    let mut next: Option<Track> = None;
    for cand in pool(&last, 5, &history) {
        if cand.id.is_empty() && cand.url.is_empty() {
            continue;
        }
        next = Some(cand.clone());
        break;
    }
    let Some(mut next) = next else {
        lm.eor(bot, &f(l, chat_id, "askip_no_recommendations"), None);
        return;
    };

    let mut requester = f(l, chat_id, "autoplay_requester");
    if requester.is_empty() || requester.starts_with('[') {
        requester = "\u{1F3B5} \u{1D4E5}\u{1D4C9}\u{1D4C9}\u{1D4CE}\u{1D4DE}\u{1D4DB}\u{1D4DA}\u{1D4EC}".to_string();
    }
    next.mark_autoplay(&requester, &g.engine_token);
    g.set_loop(0);

    let mut dl_err = String::new();
    let path = sink.download_track(chat_id, &next, &mut dl_err);
    if path.is_empty() {
        let err = if dl_err.is_empty() { "download".to_string() } else { dl_err };
        lm.eor(bot, &f_args(l, chat_id, "stream_download_fail", &[("error", &err)]), None);
        let id = g.id;
        drop(g);
        all_rooms.drop_room(id);
        return;
    }
    let video = next.video;
    let engine_path = path.clone();
    if let crate::room::PlayAction::Failed = g.play(next.clone(), path, true) {
        lm.eor(bot, &f(l, chat_id, "stream_play_fail"), None);
        let id = g.id;
        drop(g);
        all_rooms.drop_room(id);
        return;
    }
    // Hand the file to the voice engine so the autoplay skip actually plays.
    let started = match crate::modules::assistants() {
        Some(assists) => crate::monitor::start_track(g.id, &mut g, assists, &engine_path, video),
        None => false,
    };
    if !started {
        lm.eor(bot, &f(l, chat_id, "stream_play_fail"), None);
        let id = g.id;
        drop(g);
        all_rooms.drop_room(id);
        return;
    }
    lm.id = 0;
    let text = now_playing_text(l, chat_id, &next);
    show_track(&mut lm, bot, chat_id, &mut g, &next, &text, false);
}

/// handle_active: owner/sudo listing of every live room.
pub fn handle_active(bot: &mut dyn Bot, m: &TgMessage) {
    let l = loc();
    let chat_id = m.chat_id;
    if !is_owner_or_sudo(m.sender_id) {
        let _ = bot.reply_text(m, &f(l, chat_id, "only_sudo"), None);
        return;
    }

    let mut ids: Vec<i64> = Vec::new();
    let all = rooms().map(|rs| rs.all_rooms()).unwrap_or_default();
    for (id, r) in &all {
        let g = r.lock().unwrap_or_else(|e| e.into_inner());
        if g.destroyed() || !g.active() {
            continue;
        }
        ids.push(*id);
    }

    if ids.is_empty() {
        let _ = bot.reply_text(
            m,
            "<b>\u{1F3B5} Active Voice Chats</b>\n\nNo active chat sessions found.",
            None,
        );
        return;
    }

    ids.sort();
    let mut msg_text = format!(
        "<b>\u{1F3B5} Active Voice Chats</b>\n\nThere are currently <b>{}</b> active voice/video chat(s) running.\n\n<b>\u{1F4CA} Active Chats</b>\n",
        ids.len()
    );
    for (i, id) in ids.iter().enumerate() {
        let Some((_, r)) = all.iter().find(|(rid, _)| rid == id) else { continue };
        let g = r.lock().unwrap_or_else(|e| e.into_inner());
        let status = if g.muted {
            "<b>\u{1F507} Muted</b>"
        } else if g.paused {
            "<b>\u{23F8} Paused</b>"
        } else if let Some(t) = g.track.as_ref() {
            if t.video {
                "<b>\u{25B6}\u{FE0F} Playing (Video)</b>"
            } else {
                "<b>\u{25B6}\u{FE0F} Playing (Audio)</b>"
            }
        } else {
            "<b>\u{23E9} Idle</b>"
        };
        let track_cell = match g.track.as_ref() {
            None => "<i>\u{1F507} No song playing.</i>".to_string(),
            Some(t) => format!(
                "{} <i>({})</i>",
                escape_html(&short_title(&t.title, 25)),
                format_duration(t.duration)
            ),
        };
        let queue_cell = if g.queue.is_empty() {
            "<i>Empty</i>".to_string()
        } else {
            format!("<b>{}</b>", g.queue.len())
        };
        msg_text += &format!(
            "{}. <code>{}</code> \u{2014} {status} \u{2014} {track_cell} \u{2014} {queue_cell}\n",
            i + 1,
            id
        );
    }
    msg_text += &format!("Total active: {}", ids.len());
    let _ = bot.reply_text(m, &msg_text, None);
}
