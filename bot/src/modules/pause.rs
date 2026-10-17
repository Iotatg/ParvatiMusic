//! /pause and /resume (playback state cards).
use std::sync::{Arc, Mutex};

use parvati_mtproto::messaging::TgMessage;

use crate::modules::common::{f, f_args, mention_of};
use crate::modules::{loc, Registry};
use crate::mod_tg::Bot;
use crate::room::RoomState;
use crate::utils::{escape_html, format_duration, short_title};

pub fn register(r: &mut Registry) {
    r.command("pause", |b, m| handle_pause(b, m, false));
    r.command("cpause", |b, m| handle_pause(b, m, true));
    r.command("resume", |b, m| handle_resume(b, m, false));
    r.command("cresume", |b, m| handle_resume(b, m, true));
}

fn fmt_speed(v: f64) -> String {
    format!("{v:.2}")
}

/// cplay room resolution (no room creation; the error text is pre-rendered).
fn room(m: &TgMessage, cplay: bool) -> Result<Arc<Mutex<RoomState>>, String> {
    let l = loc();
    let chat_id = m.chat_id;
    let room_id = if cplay {
        let linked = crate::db::linked_channel(chat_id);
        if linked == 0 {
            return Err(f(l, chat_id, "cplay_id_not_set"));
        }
        linked
    } else {
        chat_id
    };
    crate::modules::rooms()
        .and_then(|rs| rs.room_for(room_id))
        .ok_or_else(|| f(l, chat_id, "room_no_active"))
}

/// handle_pause.
pub fn handle_pause(bot: &mut dyn Bot, m: &TgMessage, cplay: bool) {
    let l = loc();
    let chat_id = m.chat_id;
    if m.is_private || m.is_channel {
        let _ = bot.reply_text(m, &f(l, chat_id, "only_supergroup"), None);
        return;
    }
    let r = match room(m, cplay) {
        Ok(r) => r,
        Err(e) => {
            let _ = bot.reply_text(m, &e, None);
            return;
        }
    };
    let mut g = r.lock().unwrap_or_else(|e| e.into_inner());
    if !g.active() {
        let _ = bot.reply_text(m, &f(l, chat_id, "room_no_active"), None);
        return;
    }
    if g.paused {
        let _ = bot.reply_text(m, &f(l, chat_id, "pause_already"), None);
        return;
    }
    // Silence the stream itself; flipping the flag alone leaves audio playing.
    if !crate::monitor::engine_pause(g.id) {
        let _ = bot.reply_text(m, &f_args(l, chat_id, "room_pause_failed", &[("error", "pause")]), None);
        return;
    }
    if !g.pause() {
        let _ = bot.reply_text(m, &f_args(l, chat_id, "room_pause_failed", &[("error", "pause")]), None);
        return;
    }
    let title = g.track.as_ref().map(|t| escape_html(&short_title(&t.title, 25))).unwrap_or_default();
    let mut msg_text = f_args(
        l,
        chat_id,
        "pause_success",
        &[
            ("title", &title),
            ("position", &format_duration(g.position)),
            ("duration", &format_duration(g.track.as_ref().map(|t| t.duration).unwrap_or(0))),
            ("user", &mention_of(m)),
        ],
    );
    if g.speed != 1.0 {
        msg_text += &format!("\n{}", f_args(l, chat_id, "speed_line", &[("speed", &fmt_speed(g.speed))]));
    }
    let _ = bot.reply_text(m, &msg_text, None);
}

/// handle_resume.
pub fn handle_resume(bot: &mut dyn Bot, m: &TgMessage, cplay: bool) {
    let l = loc();
    let chat_id = m.chat_id;
    if m.is_private || m.is_channel {
        let _ = bot.reply_text(m, &f(l, chat_id, "only_supergroup"), None);
        return;
    }
    let r = match room(m, cplay) {
        Ok(r) => r,
        Err(e) => {
            let _ = bot.reply_text(m, &e, None);
            return;
        }
    };
    let mut g = r.lock().unwrap_or_else(|e| e.into_inner());
    if !g.active() {
        let _ = bot.reply_text(m, &f(l, chat_id, "room_no_active"), None);
        return;
    }
    if !g.paused {
        let _ = bot.reply_text(m, &f(l, chat_id, "resume_already_playing"), None);
        return;
    }
    if !crate::monitor::engine_resume(g.id) {
        let _ = bot.reply_text(m, &f_args(l, chat_id, "resume_failed", &[("error", "resume")]), None);
        return;
    }
    if !g.resume() {
        let _ = bot.reply_text(m, &f_args(l, chat_id, "resume_failed", &[("error", "resume")]), None);
        return;
    }
    let title = g.track.as_ref().map(|t| escape_html(&short_title(&t.title, 25))).unwrap_or_default();
    let speed_line = if g.speed != 1.0 {
        f_args(l, chat_id, "speed_line", &[("speed", &fmt_speed(g.speed))])
    } else {
        String::new()
    };
    let _ = bot.reply_text(
        m,
        &f_args(
            l,
            chat_id,
            "resume_success",
            &[
                ("title", &title),
                ("position", &format_duration(g.position)),
                ("duration", &format_duration(g.track.as_ref().map(|t| t.duration).unwrap_or(0))),
                ("user", &mention_of(m)),
                ("speed_line", &speed_line),
            ],
        ),
        None,
    );
}
