//! Queue mutation group for tests and registration parity: skip.
use parvati_mtproto::messaging::TgMessage;

use crate::modules::common::{f, f_args, mention_of, now_playing_text, show_track, LiveMsg};
use crate::modules::play::PlaySink;
use crate::modules::{loc, Registry};
use crate::mod_tg::Bot;
use crate::room::PlayAction;
use crate::utils::trim;

pub fn register(r: &mut Registry) {
    r.command("skip", plain);
    r.command("cskip", cplay);
}

fn plain(bot: &mut dyn Bot, m: &TgMessage) {
    handle_skip(bot, m, false, &super::play::RealSink);
}

fn cplay(bot: &mut dyn Bot, m: &TgMessage) {
    handle_skip(bot, m, true, &super::play::RealSink);
}

/// stoi semantics: parse the leading integer, ignore trailing junk.
fn parse_int_prefix(s: &str) -> Option<i64> {
    let (sign, digits_start) = match s.strip_prefix('-') {
        Some(rest) => (-1i64, rest),
        None => (1, s.strip_prefix('+').unwrap_or(s)),
    };
    let digits: String = digits_start.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<i64>().ok().map(|v| sign * v)
}

/// handle_skip: `n` pops the first n-1 queued tracks and starts the new head.
pub fn handle_skip(bot: &mut dyn Bot, m: &TgMessage, cplay: bool, sink: &dyn PlaySink) {
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
    let mut g = r.lock().unwrap_or_else(|e| e.into_inner());
    if !g.active() {
        let _ = bot.reply_text(m, &f(l, chat_id, "room_no_active"), None);
        return;
    }

    let mention = mention_of(m);
    let mut skip_count: i64 = 1;
    let rest = m
        .text
        .split_once(' ')
        .map(|(_, rest)| trim(rest))
        .unwrap_or("");
    if !rest.is_empty() {
        let parsed = match parse_int_prefix(rest) {
            Some(v) => v,
            None => {
                let _ = bot.reply_text(m, &f(l, chat_id, "skip_invalid_number"), None);
                return;
            }
        };
        let queued = g.queue.len() as i64;
        if queued == 0 {
            let _ = bot.reply_text(m, &f(l, chat_id, "skip_queue_empty_for_count"), None);
            return;
        }
        if parsed < 1 || parsed > queued {
            let _ = bot.reply_text(
                m,
                &f_args(
                    l,
                    chat_id,
                    "skip_count_exceeds_queue",
                    &[("requested", &parsed.to_string()), ("available", &queued.to_string())],
                ),
                None,
            );
            return;
        }
        skip_count = parsed + 1;
    }

    if g.queue.is_empty() {
        let id = g.id;
        drop(g);
        rooms.drop_room(id);
        let _ = bot.reply_text(m, &f_args(l, chat_id, "skip_stopped", &[("user", &mention)]), None);
        return;
    }

    g.set_loop(0);
    for _ in 1..skip_count {
        if g.queue.is_empty() {
            let id = g.id;
            drop(g);
            rooms.drop_room(id);
            let _ = bot.reply_text(m, &f_args(l, chat_id, "skip_stopped", &[("user", &mention)]), None);
            return;
        }
        g.next_track();
    }
    if g.queue.is_empty() {
        let id = g.id;
        drop(g);
        rooms.drop_room(id);
        let _ = bot.reply_text(m, &f_args(l, chat_id, "skip_stopped", &[("user", &mention)]), None);
        return;
    }

    let Some(t) = g.next_track() else {
        let id = g.id;
        drop(g);
        rooms.drop_room(id);
        let _ = bot.reply_text(m, &f_args(l, chat_id, "skip_stopped", &[("user", &mention)]), None);
        return;
    };

    let mut lm = LiveMsg::new();
    lm.chat_id = chat_id;
    lm.send(bot, &f(l, chat_id, "stream_downloading_next"), None);

    let mut dl_err = String::new();
    let path = sink.download_track(chat_id, &t, &mut dl_err);
    if path.is_empty() {
        let err = if dl_err.is_empty() { "download".to_string() } else { dl_err };
        lm.eor(bot, &f_args(l, chat_id, "stream_download_fail", &[("error", &err)]), None);
        let id = g.id;
        drop(g);
        rooms.drop_room(id);
        return;
    }
    let video = t.video;
    let engine_path = path.clone();
    if let PlayAction::Failed = g.play(t.clone(), path, true) {
        lm.eor(bot, &f(l, chat_id, "stream_play_fail"), None);
        let id = g.id;
        drop(g);
        rooms.drop_room(id);
        return;
    }
    // Hand the freshly downloaded file to the voice engine: without this the
    // card advances but the old audio keeps streaming.
    let Some(assists) = crate::modules::assistants() else {
        lm.eor(bot, &f(l, chat_id, "stream_play_fail"), None);
        let id = g.id;
        drop(g);
        rooms.drop_room(id);
        return;
    };
    if !crate::monitor::start_track(g.id, &mut g, assists, &engine_path, video) {
        lm.eor(bot, &f(l, chat_id, "stream_play_fail"), None);
        let id = g.id;
        drop(g);
        rooms.drop_room(id);
        return;
    }
    lm.id = 0;
    let text = now_playing_text(l, chat_id, &t);
    show_track(&mut lm, bot, chat_id, &mut g, &t, &text, false);
}
