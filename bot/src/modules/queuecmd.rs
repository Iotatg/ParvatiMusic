//! /queue listing: header, now-playing cell, up-next block.

use parvati_mtproto::messaging::TgMessage;

use crate::modules::common::f_args;
use crate::modules::{loc, Registry};
use crate::mod_tg::Bot;
use crate::utils::{escape_html, format_duration, short_title};

pub fn register(r: &mut Registry) {
    r.command("queue", |b, m| handle_queue(b, m, false));
    r.command("cqueue", |b, m| handle_queue(b, m, true));
}

/// handle_queue.
pub fn handle_queue(bot: &mut dyn Bot, m: &TgMessage, cplay: bool) {
    let l = loc();
    let chat_id = m.chat_id;
    if m.is_private || m.is_channel {
        let _ = bot.reply_text(m, &crate::modules::common::f(l, chat_id, "only_supergroup"), None);
        return;
    }
    let room_id = if cplay {
        let linked = crate::db::linked_channel(chat_id);
        if linked == 0 {
            let _ = bot.reply_text(m, &crate::modules::common::f(l, chat_id, "cplay_id_not_set"), None);
            return;
        }
        linked
    } else {
        chat_id
    };
    let Some(rooms) = crate::modules::rooms() else {
        let _ = bot.reply_text(m, &crate::modules::common::f(l, chat_id, "queue_no_active"), None);
        return;
    };
    let Some(r) = rooms.room_for(room_id) else {
        let _ = bot.reply_text(m, &crate::modules::common::f(l, chat_id, "queue_no_active"), None);
        return;
    };
    let g = r.lock().unwrap_or_else(|e| e.into_inner());
    let Some(t) = g.track.as_ref() else {
        let _ = bot.reply_text(m, &crate::modules::common::f(l, chat_id, "queue_no_active"), None);
        return;
    };
    if !g.active() {
        let _ = bot.reply_text(m, &crate::modules::common::f(l, chat_id, "queue_no_active"), None);
        return;
    }

    let count = g.queue.len().to_string();
    let mut b = String::new();
    b += &crate::modules::common::f_args(l, chat_id, "queue_header", &[("count", &count)]);
    b += "\n\n";
    b += &crate::modules::common::f(l, chat_id, "queue_now_playing");
    b += "\n";
    b += "\u{1F3A7} ";
    b += &escape_html(&short_title(&t.title, 35));
    b += " \u{2014} ";
    b += &t.requester;
    b += " [";
    b += &format_duration(t.duration);
    b += "]\n\n";

    let q = g.queue.len() as i64;
    let use_quote = q >= 3;
    if q > 0 {
        b += &crate::modules::common::f(l, chat_id, "queue_up_next");
        b += if use_quote { "\n" } else { "\n\n" };
        if use_quote {
            b += "<blockquote>";
        }
        for (i, track) in g.queue.iter().enumerate().take(10) {
            b += &format!(
                "{}. \u{1F3B5} {} \u{2014} {} [{}]\n",
                i + 1,
                escape_html(&short_title(&track.title, 35)),
                track.requester,
                format_duration(track.duration)
            );
        }
        if use_quote {
            b += "</blockquote>";
        }
        if q > 10 {
            let remaining = (q - 10).to_string();
            b += &f_args(l, chat_id, "queue_more_line", &[("remaining", &remaining)]);
        }
    } else {
        b += &crate::modules::common::f(l, chat_id, "queue_empty_tail");
    }
    let _ = bot.reply_text(m, &b, None);
}
