//! Playlist commands, the autoplay menu callback, and the progress ack


use parvati_mtproto::messaging::{Markup, TgCallback, TgMessage};

use crate::buttons::{data_btn, rows};
use crate::db::{self, PlaylistEntry};
use crate::mod_tg::Bot;
use crate::modules::common::{command_of, f_args};
use crate::modules::play::{PlaySink, RealSink};
use crate::modules::{loc, Registry};
use crate::utils::{escape_html, trim};

const MAX_PLAYLIST_NAME_LEN: usize = 40;
const MAX_PLAYLISTS_PER_USER: usize = 10;

/// cmd_arg: everything after the command word, trimmed.
fn cmd_arg(m: &TgMessage) -> String {
    let Some(sp) = m.text.find(' ') else {
        return String::new();
    };
    trim(&m.text[sp + 1..]).to_string()
}

/// truncate at a rune boundary after max runes.
fn truncate_runes(s: &str, max_runes: usize) -> String {
    s.chars().take(max_runes).collect()
}

/// requester_name: display name or the raw id.
fn requester_name(m: &TgMessage) -> String {
    let name = trim(&format!("{} {}", m.first_name, m.last_name)).to_string();
    if name.is_empty() {
        m.sender_id.to_string()
    } else {
        name
    }
}

/// split_two: "name rest" — false unless both parts are non-empty.
fn split_two(args: &str) -> Option<(String, String)> {
    let sp = args.find(' ')?;
    let first = args[..sp].to_string();
    let second = trim(&args[sp + 1..]).to_string();
    if first.is_empty() || second.is_empty() {
        return None;
    }
    Some((first, second))
}

/// /createplaylist [name] — 10 per user, 40-rune names.
pub fn handle_create_playlist(bot: &mut dyn Bot, m: &TgMessage) {
    let loc = loc();
    let chat_id = m.chat_id;
    let mut name = cmd_arg(m);
    if name.is_empty() {
        let _ = bot.reply_text(
            m,
            &f_args(loc, chat_id, "playlist_usage_create", &[("cmd", &command_of(m))]),
            None,
        );
        return;
    }
    if db::playlists_of(m.sender_id).len() >= MAX_PLAYLISTS_PER_USER {
        let _ = bot.reply_text(m, &f_args(loc, chat_id, "playlist_limit_reached", &[]), None);
        return;
    }
    name = truncate_runes(&name, MAX_PLAYLIST_NAME_LEN);
    if !db::create_playlist(m.sender_id, &name) {
        let _ = bot.reply_text(
            m,
            &f_args(
                loc,
                chat_id,
                "playlist_create_fail",
                &[("error", &format!("a playlist named '{name}' already exists"))],
            ),
            None,
        );
        return;
    }
    let _ = bot.reply_text(
        m,
        &f_args(
            loc,
            chat_id,
            "playlist_create_success",
            &[("name", &escape_html(&name)), ("id", &escape_html(&name))],
        ),
        None,
    );
}

/// /deleteplaylist [name].
pub fn handle_delete_playlist(bot: &mut dyn Bot, m: &TgMessage) {
    let loc = loc();
    let chat_id = m.chat_id;
    let name = cmd_arg(m);
    if name.is_empty() {
        let _ = bot.reply_text(
            m,
            &f_args(loc, chat_id, "playlist_usage_delete", &[("cmd", &command_of(m))]),
            None,
        );
        return;
    }
    if !db::playlist_exists(m.sender_id, &name) {
        let _ = bot.reply_text(m, &f_args(loc, chat_id, "playlist_not_found", &[]), None);
        return;
    }
    if !db::delete_playlist(m.sender_id, &name) {
        let _ = bot.reply_text(
            m,
            &f_args(loc, chat_id, "playlist_delete_fail", &[("error", "delete failed")]),
            None,
        );
        return;
    }
    let _ = bot.reply_text(
        m,
        &f_args(loc, chat_id, "playlist_delete_success", &[("name", &escape_html(&name))]),
        None,
    );
}

/// /addtoplaylist [name] [song query or url] — first resolved track wins.
pub fn handle_add_to_playlist(bot: &mut dyn Bot, m: &TgMessage, sink: &dyn PlaySink) {
    let loc = loc();
    let chat_id = m.chat_id;
    let Some((name, query)) = split_two(&cmd_arg(m)) else {
        let _ = bot.reply_text(
            m,
            &f_args(loc, chat_id, "playlist_usage_add", &[("cmd", &command_of(m))]),
            None,
        );
        return;
    };
    if !db::playlist_exists(m.sender_id, &name) {
        let _ = bot.reply_text(m, &f_args(loc, chat_id, "playlist_not_found", &[]), None);
        return;
    }
    let tracks = sink.resolve_query(&query, false);
    if tracks.is_empty() {
        let _ = bot.reply_text(m, &f_args(loc, chat_id, "playlist_invalid_query", &[]), None);
        return;
    }
    let entry = PlaylistEntry {
        title: tracks[0].title.clone(),
        url: tracks[0].url.clone(),
        duration: tracks[0].duration,
        requester: requester_name(m),
    };
    if !db::add_to_playlist(m.sender_id, &name, &entry) {
        let _ = bot.reply_text(
            m,
            &f_args(loc, chat_id, "playlist_add_fail", &[("error", "add failed")]),
            None,
        );
        return;
    }
    let _ = bot.reply_text(
        m,
        &f_args(
            loc,
            chat_id,
            "playlist_add_success",
            &[("song", &escape_html(&entry.title)), ("playlist", &escape_html(&name))],
        ),
        None,
    );
}

fn add_plain(bot: &mut dyn Bot, m: &TgMessage) {
    handle_add_to_playlist(bot, m, &RealSink);
}

/// /removefromplaylist [name] [song number or url].
pub fn handle_remove_from_playlist(bot: &mut dyn Bot, m: &TgMessage) {
    let loc = loc();
    let chat_id = m.chat_id;
    let Some((name, ident)) = split_two(&cmd_arg(m)) else {
        let _ = bot.reply_text(
            m,
            &f_args(loc, chat_id, "playlist_usage_remove", &[("cmd", &command_of(m))]),
            None,
        );
        return;
    };
    if !db::playlist_exists(m.sender_id, &name) {
        let _ = bot.reply_text(m, &f_args(loc, chat_id, "playlist_not_found", &[]), None);
        return;
    }
    let entries = db::playlist_entries(m.sender_id, &name);

    // partial-consume parse: a fully-numeric ident (including
    // negatives/zero) takes the numeric branch; a number outside 1..=len is
    // refused outright; anything else falls back to the url search.
    let mut index = 0usize;
    let mut found = false;
    if let Ok(parsed) = ident.parse::<i64>() {
        if parsed >= 1 && parsed as usize <= entries.len() {
            index = parsed as usize - 1;
            found = true;
        } else {
            let _ = bot.reply_text(m, &f_args(loc, chat_id, "playlist_invalid_song_number", &[]), None);
            return;
        }
    }
    if !found {
        for (i, e) in entries.iter().enumerate() {
            if e.url == ident {
                index = i;
                found = true;
                break;
            }
        }
    }
    if !found {
        let _ = bot.reply_text(m, &f_args(loc, chat_id, "playlist_song_not_found", &[]), None);
        return;
    }
    if !db::remove_from_playlist(m.sender_id, &name, index) {
        let _ = bot.reply_text(
            m,
            &f_args(loc, chat_id, "playlist_remove_fail", &[("error", "remove failed")]),
            None,
        );
        return;
    }
    let _ = bot.reply_text(
        m,
        &f_args(loc, chat_id, "playlist_remove_success", &[("name", &escape_html(&name))]),
        None,
    );
}

/// /playlistinfo [name] — owner column is "Unknown" when unmapped.
pub fn handle_playlist_info(bot: &mut dyn Bot, m: &TgMessage) {
    let loc = loc();
    let chat_id = m.chat_id;
    let name = cmd_arg(m);
    if name.is_empty() {
        let _ = bot.reply_text(
            m,
            &f_args(loc, chat_id, "playlist_usage_info", &[("cmd", &command_of(m))]),
            None,
        );
        return;
    }
    if !db::playlist_exists(m.sender_id, &name) {
        let _ = bot.reply_text(m, &f_args(loc, chat_id, "playlist_not_found", &[]), None);
        return;
    }
    let entries = db::playlist_entries(m.sender_id, &name);
    let songs = if entries.is_empty() {
        f_args(loc, chat_id, "playlist_songs_empty", &[])
    } else {
        entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                f_args(
                    loc,
                    chat_id,
                    "playlist_song_line",
                    &[
                        ("index", &(i + 1).to_string()),
                        ("name", &escape_html(&e.title)),
                        ("url", &e.url),
                    ],
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let _ = bot.reply_text(
        m,
        &f_args(
            loc,
            chat_id,
            "playlist_info",
            &[
                ("name", &escape_html(&name)),
                ("owner", "Unknown"),
                ("count", &entries.len().to_string()),
                ("songs", &songs),
            ],
        ),
        None,
    );
}

/// /myplaylists.
pub fn handle_my_playlists(bot: &mut dyn Bot, m: &TgMessage) {
    let loc = loc();
    let chat_id = m.chat_id;
    let names = db::playlists_of(m.sender_id);
    if names.is_empty() {
        let _ = bot.reply_text(m, &f_args(loc, chat_id, "playlist_none", &[]), None);
        return;
    }
    let lines = names
        .iter()
        .map(|n| f_args(loc, chat_id, "playlist_list_line", &[("name", &escape_html(n)), ("id", &escape_html(n))]))
        .collect::<Vec<_>>()
        .join("\n");
    let _ = bot.reply_text(
        m,
        &f_args(loc, chat_id, "playlist_list", &[("playlists", &lines)]),
        None,
    );
}

/// progress: bare acknowledgment.
pub fn handle_progress_cb(bot: &mut dyn Bot, cb: &TgCallback) {
    let _ = bot.answer_callback(cb.query_id, "", false);
}

fn autoplay_menu_text(chat_id: i64, enabled: bool) -> String {
    let loc = loc();
    let state = if enabled {
        f_args(loc, chat_id, "enabled", &[])
    } else {
        f_args(loc, chat_id, "disabled", &[])
    };
    f_args(loc, chat_id, "autoplay_menu", &[("state", &state)])
}

fn autoplay_menu_markup(chat_id: i64, enabled: bool) -> Markup {
    let loc = loc();
    let state = if enabled {
        f_args(loc, chat_id, "enabled", &[])
    } else {
        f_args(loc, chat_id, "disabled", &[])
    };
    rows(vec![
        vec![data_btn(
            &f_args(loc, chat_id, "autoplay_btn", &[("state", &state)]),
            "autoplay:toggle",
        )],
        vec![data_btn(&f_args(loc, chat_id, "CLOSE_BTN", &[]), "close")],
    ])
}

/// autoplay:toggle — active rooms flip in-memory only; otherwise db (and the
/// idle room, when one exists, mirrors the new value).
pub fn handle_autoplay_toggle_cb(bot: &mut dyn Bot, cb: &TgCallback) {
    let chat_id = cb.chat_id;
    let room = crate::modules::rooms().and_then(|r| r.room_for(chat_id));
    let room_active = room.as_ref().map(|r| r.lock().unwrap().active()).unwrap_or(false);
    let enabled = if room_active {
        let mut g = room.as_ref().unwrap().lock().unwrap();
        let next = !g.autoplay;
        g.set_autoplay(next);
        g.autoplay
    } else {
        let enabled = !db::autoplay(chat_id);
        db::set_autoplay(chat_id, enabled);
        if let Some(r) = &room {
            r.lock().unwrap().set_autoplay(enabled);
        }
        enabled
    };
    let _ = bot.answer_callback(cb.query_id, "", false);
    let _ = bot.edit_text(
        chat_id,
        cb.message_id,
        &autoplay_menu_text(chat_id, enabled),
        Some(&autoplay_menu_markup(chat_id, enabled)),
    );
}

pub fn register(r: &mut Registry) {
    r.command("createplaylist", handle_create_playlist);
    r.command("deleteplaylist", handle_delete_playlist);
    r.command("addtoplaylist", add_plain);
    r.command("removefromplaylist", handle_remove_from_playlist);
    r.command("playlistinfo", handle_playlist_info);
    r.command("plistinfo", handle_playlist_info);
    r.command("myplaylists", handle_my_playlists);
    r.callback("progress", handle_progress_cb);
    r.callback("autoplay:toggle", handle_autoplay_toggle_cb);
}
