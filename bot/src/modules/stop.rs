//! /stop and /end (stop confirmation markup when paused/muted).
use parvati_mtproto::messaging::TgMessage;

use crate::modules::common::{f, f_args, mention_of};
use crate::modules::{loc, Registry};
use crate::mod_tg::Bot;

pub fn register(r: &mut Registry) {
    r.command("stop", |b, m| handle_stop(b, m, false));
    r.command("end", |b, m| handle_stop(b, m, false));
    r.command("cstop", |b, m| handle_stop(b, m, true));
    r.command("cend", |b, m| handle_stop(b, m, true));
}

/// handle_stop: paused/muted rooms get a confirm card; healthy rooms die.
pub fn handle_stop(bot: &mut dyn Bot, m: &TgMessage, cplay: bool) {
    let l = loc();
    let chat_id = m.chat_id;
    if m.is_private || m.is_channel {
        let _ = bot.reply_text(m, &f(l, chat_id, "only_supergroup"), None);
        return;
    }
    let room_id = if cplay {
        let linked = crate::db::linked_channel(chat_id);
        if linked == 0 {
            let _ = bot.reply_text(m, &f(l, chat_id, "cplay_id_not_set"), None);
            return;
        }
        linked
    } else {
        chat_id
    };
    let Some(rooms) = crate::modules::rooms() else {
        let _ = bot.reply_text(m, &f(l, chat_id, "room_no_active"), None);
        return;
    };
    let Some(r) = rooms.room_for(room_id) else {
        let _ = bot.reply_text(m, &f(l, chat_id, "room_no_active"), None);
        return;
    };
    let g = r.lock().unwrap_or_else(|e| e.into_inner());
    if !g.active() {
        let _ = bot.reply_text(m, &f(l, chat_id, "room_no_active"), None);
        return;
    }

    let is_paused = g.paused;
    let is_muted = g.muted;
    if is_paused || is_muted {
        let key = if is_muted { "stop_confirm_muted" } else { "stop_confirm_paused" };
        let text = f(l, chat_id, key);
        let markup = crate::buttons::stop_confirm_markup(l, &crate::db::language(chat_id), &g.view());
        let _ = bot.reply_text(m, &text, Some(&markup));
        return;
    }

    let id = g.id;
    drop(g);
    rooms.drop_room(id);
    let _ = bot.reply_text(m, &f_args(l, chat_id, "stopped", &[("user", &mention_of(m))]), None);
}
