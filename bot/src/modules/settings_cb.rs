//! Language picker and the settings card.
//!
//! cmddelete/cleanmode/cleanduration/adminmode are process-local maps like
//! process-local globals (no db API for these flags); playmode and
//! nothumb persist through the db.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use parvati_mtproto::messaging::{Markup, TgCallback, TgMessage};

use crate::buttons::{data_btn, rows};
use crate::db;
use crate::locales::LocaleArg;
use crate::mod_tg::Bot;
use crate::modules::common::{f, f_args};
use crate::modules::{cfg, loc, Registry};

static CMD_DELETE: LazyLock<Mutex<HashMap<i64, bool>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static CLEAN_MODE: LazyLock<Mutex<HashMap<i64, bool>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static CLEAN_DURATION: LazyLock<Mutex<HashMap<i64, i64>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static ADMINMODE: LazyLock<Mutex<HashMap<i64, String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Test/state accessors with the map defaults.
pub fn cmddelete(chat_id: i64) -> bool {
    *CMD_DELETE.lock().unwrap().get(&chat_id).unwrap_or(&false)
}

pub fn cleanmode(chat_id: i64) -> bool {
    *CLEAN_MODE.lock().unwrap().get(&chat_id).unwrap_or(&false)
}

pub fn clean_duration(chat_id: i64) -> i64 {
    let v = *CLEAN_DURATION.lock().unwrap().get(&chat_id).unwrap_or(&15);
    if v <= 0 {
        15
    } else {
        v
    }
}

pub fn adminmode(chat_id: i64) -> String {
    ADMINMODE
        .lock()
        .unwrap()
        .get(&chat_id)
        .cloned()
        .unwrap_or_else(|| "adminauth".into())
}

/// Sets adminmode directly (the /adminmode command; the card cycles instead).
pub fn set_adminmode(chat_id: i64, mode: &str) {
    ADMINMODE.lock().unwrap().insert(chat_id, mode.to_string());
}

/// Sets cleanmode directly (the /cleanmode command).
pub fn set_cleanmode(chat_id: i64, v: bool) {
    CLEAN_MODE.lock().unwrap().insert(chat_id, v);
}

fn adminmode_set(chat_id: i64, mode: &str) {
    ADMINMODE.lock().unwrap().insert(chat_id, mode.to_string());
}

/// admins -> adminauth -> everyone -> admins cycle.
fn adminmode_cycle(chat_id: i64) {
    let mode = adminmode(chat_id);
    let next = match mode.as_str() {
        "admins" => "adminauth",
        "adminauth" => "everyone",
        _ => "admins",
    };
    adminmode_set(chat_id, next);
}

fn adminmode_status_word(chat_id: i64) -> String {
    let key = match adminmode(chat_id).as_str() {
        "admins" => "adminmode_status_admin",
        "everyone" => "adminmode_status_everyone",
        _ => "adminmode_status_adminauth",
    };
    f(loc(), chat_id, key)
}

/// The settings card: label -> info: btn, state -> set: btn, per row.
pub fn settings_markup(chat_id: i64) -> Markup {
    let l = loc();
    let word = |v: bool| f_args(l, chat_id, if v { "enabled" } else { "disabled" }, &[]);
    rows(vec![
        vec![
            data_btn(&f(l, chat_id, "settings_btn_adminmode"), "info:adminmode"),
            data_btn(&adminmode_status_word(chat_id), "set:adminmode"),
        ],
        vec![
            data_btn(&f(l, chat_id, "settings_btn_playmode"), "info:playmode"),
            data_btn(
                &f(l, chat_id, if db::play_mode_admins_only(chat_id) { "playmode_status_admins" } else { "playmode_status_everyone" }),
                "set:playmode",
            ),
        ],
        vec![
            data_btn(&f(l, chat_id, "settings_btn_cmddelete"), "info:cmddelete"),
            data_btn(&word(cmddelete(chat_id)), "set:cmddelete"),
        ],
        vec![
            data_btn(&f(l, chat_id, "settings_btn_cleanmode"), "info:cleanmode"),
            data_btn(&word(cleanmode(chat_id)), "set:cleanmode"),
        ],
        vec![
            data_btn(&f(l, chat_id, "settings_btn_cleanduration"), "info:cleanduration"),
            data_btn(&format!("{}m", clean_duration(chat_id)), "set:cleanduration"),
        ],
        vec![
            data_btn(&f(l, chat_id, "settings_btn_nothumb"), "info:nothumb"),
            data_btn(&word(!db::thumbnails_disabled(chat_id)), "set:nothumb"),
        ],
        vec![
            data_btn(&f(l, chat_id, "settings_btn_lang"), "info:lang"),
            data_btn(&f(l, chat_id, "name"), "lang:select"),
        ],
        vec![data_btn(&f(l, chat_id, "CLOSE_BTN"), "close")],
    ])
}

fn settings_main_text(chat_id: i64) -> String {
    f_args(loc(), chat_id, "settings_main", &[("title", "Chat"), ("id", &chat_id.to_string())])
}

/// group_gate: private chats get pointed at supergroups, channels are
/// silently ignored; groups register as served.
fn group_gate(bot: &mut dyn Bot, m: &TgMessage) -> bool {
    let loc = loc();
    if m.is_private {
        let _ = bot.reply_text(m, &f(loc, m.chat_id, "only_supergroup"), None);
        db::add_served_user(m.chat_id);
        return false;
    }
    if m.is_channel {
        return false;
    }
    db::add_served_chat(m.chat_id);
    true
}

/// privileged_gate: owner/sudo/authorized pass (the client exposes no
/// live chat-admin lookup).
fn privileged_gate(chat_id: i64, user_id: i64) -> bool {
    let owner = cfg().owner_id;
    if owner != 0 && user_id == owner {
        return true;
    }
    if db::is_sudo(user_id) {
        return true;
    }
    db::is_authorized(chat_id, user_id)
}

fn privileged_msg(bot: &mut dyn Bot, m: &TgMessage) -> bool {
    if privileged_gate(m.chat_id, m.sender_id) {
        return true;
    }
    let _ = bot.reply_text(m, &f(loc(), m.chat_id, "only_admin_or_auth"), None);
    false
}

/// /settings — the settings card.
pub fn handle_settings(bot: &mut dyn Bot, m: &TgMessage) {
    if !group_gate(bot, m) {
        return;
    }
    if !privileged_msg(bot, m) {
        return;
    }
    let chat_id = m.chat_id;
    let _ = bot.reply_text(m, &settings_main_text(chat_id), Some(&settings_markup(chat_id)));
}

/// set:/info: dispatcher — every action re-renders the card after toggling;
/// info: rows only ack.
pub fn handle_settings_cb(bot: &mut dyn Bot, cb: &TgCallback) {
    let chat_id = cb.chat_id;
    let data = cb.data.as_str();
    let Some(p) = data.find(':') else {
        let _ = bot.answer_callback(cb.query_id, "", false);
        return;
    };
    let rest = &data[p + 1..];
    let action = match rest.find(':') {
        Some(q) => &rest[..q],
        None => rest,
    };
    if !privileged_gate(chat_id, cb.user_id) {
        let _ = bot.answer_callback(cb.query_id, "", false);
        return;
    }
    if data.starts_with("info:") {
        let _ = bot.answer_callback(cb.query_id, "", false);
        return;
    }
    if action == "main" {
        let _ = bot.answer_callback(cb.query_id, "", false);
        let _ = bot.edit_text(chat_id, cb.message_id, &settings_main_text(chat_id), Some(&settings_markup(chat_id)));
        return;
    }
    match action {
        "playmode" => {
            let next = !db::play_mode_admins_only(chat_id);
            db::set_play_mode_admins_only(chat_id, next);
        }
        "adminmode" => adminmode_cycle(chat_id),
        "cmddelete" => {
            let next = !cmddelete(chat_id);
            CMD_DELETE.lock().unwrap().insert(chat_id, next);
        }
        "cleanmode" => {
            let next = !cleanmode(chat_id);
            CLEAN_MODE.lock().unwrap().insert(chat_id, next);
        }
        "cleanduration" => {
            let opts = [15, 30, 60, 5];
            let cur = clean_duration(chat_id);
            let mut next = 15;
            for (i, o) in opts.iter().enumerate() {
                if *o == cur {
                    next = opts[(i + 1) % 4];
                    break;
                }
            }
            CLEAN_DURATION.lock().unwrap().insert(chat_id, next);
        }
        "nothumb" => {
            let next = !db::thumbnails_disabled(chat_id);
            db::set_thumbnails_disabled(chat_id, next);
        }
        _ => {}
    }
    let _ = bot.answer_callback(cb.query_id, "", false);
    let _ = bot.edit_text(chat_id, cb.message_id, &settings_main_text(chat_id), Some(&settings_markup(chat_id)));
}

/// lang: rows of two, the current language marked, optional back row to
/// set:main language markup.
pub fn lang_markup(chat_id: i64, current: &str, with_back: bool) -> Markup {
    let l = loc();
    let mut table: Vec<Vec<_>> = Vec::new();
    let mut row: Vec<_> = Vec::new();
    for code in l.available() {
        let mut name = l.get(&code, "name", &LocaleArg::new());
        if code == current {
            name = format!("\u{2714}\u{fe0f} {name}");
        }
        row.push(data_btn(&name, &format!("lang:{code}")));
        if row.len() == 2 {
            table.push(std::mem::take(&mut row));
        }
    }
    if !row.is_empty() {
        table.push(row);
    }
    if with_back {
        table.push(vec![data_btn(&f(l, chat_id, "BACK_BTN"), "set:main")]);
    }
    rows(table)
}

/// /lang, /language — the picker without a back row.
pub fn handle_lang_cmd(bot: &mut dyn Bot, m: &TgMessage) {
    if !group_gate(bot, m) {
        return;
    }
    let chat_id = m.chat_id;
    let mut current = db::language(chat_id);
    if current.is_empty() {
        current = cfg().default_lang.clone();
    }
    let text = f_args(loc(), chat_id, "lang_select", &[]);
    let _ = bot.reply_text(m, &text, Some(&lang_markup(chat_id, &current, false)));
}
/// lang: callback — select shows the inline picker with a back row; a valid
/// 2-letter code persists (same code only acks lang_same).
pub fn handle_lang_cb(bot: &mut dyn Bot, cb: &TgCallback) {
    let l = loc();
    let chat_id = cb.chat_id;
    if cb.data == "lang:select" {
        let mut current = db::language(chat_id);
        if current.is_empty() {
            current = cfg().default_lang.clone();
        }
        let _ = bot.answer_callback(cb.query_id, "", false);
        let _ = bot.edit_text(
            chat_id,
            cb.message_id,
            &f_args(l, chat_id, "lang_select", &[]),
            Some(&lang_markup(chat_id, &current, true)),
        );
        return;
    }
    if !cb.data.starts_with("lang:") {
        let _ = bot.answer_callback(cb.query_id, "", false);
        return;
    }
    let lang = &cb.data[5..];
    let valid = lang.len() == 2 && lang.bytes().all(|c| c.is_ascii_lowercase());
    if !valid {
        let _ = bot.answer_callback(cb.query_id, "", false);
        return;
    }
    let current = db::language(chat_id);
    if lang == current {
        let _ = bot.answer_callback(cb.query_id, "", false);
        let _ = bot.edit_text(chat_id, cb.message_id, &f_args(l, chat_id, "lang_same", &[]), None);
        return;
    }
    db::set_language(chat_id, lang);
    let lang_name = l.get(lang, "name", &LocaleArg::new());
    let _ = bot.answer_callback(cb.query_id, "", false);
    let _ = bot.edit_text(
        chat_id,
        cb.message_id,
        &f_args(l, chat_id, "lang_success", &[("lang_name", &lang_name)]),
        None,
    );
}

pub fn register(r: &mut Registry) {
    r.command("lang", handle_lang_cmd);
    r.command("language", handle_lang_cmd);
    r.command("settings", handle_settings);
    r.callback("set", handle_settings_cb);
    r.callback("info:", handle_settings_cb);
    r.callback("lang:", handle_lang_cb);
}
