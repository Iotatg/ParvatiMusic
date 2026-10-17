//! Welcome system, notes and filters.
//!
//! Port of the reference moderation module (welcome/save/get/notes/clearnote/
//! filter family) plus the #hashtag and filter group-text hook. All state lives
//! in the shared db (notes/filters/welcome tables already exist); these
//! handlers only add the command surface.

use parvati_mtproto::messaging::TgMessage;

use crate::db;
use crate::mod_tg::Bot;
use crate::modules::admin::require_restrict_right;
use crate::modules::Registry;
use crate::utils::{escape_html, stylish, to_lower, trim};

/// Text after the command token (args_of parity).
fn args_of(m: &TgMessage) -> String {
    let t = trim(&m.text);
    if t.is_empty() || !t.starts_with('/') {
        return t.to_string();
    }
    match t.find(' ') {
        Some(sp) => t[sp + 1..].to_string(),
        None => String::new(),
    }
}

fn fields(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_string).collect()
}

/// Content after the first whitespace-separated token.
fn args_after_first(m: &TgMessage) -> String {
    let toks = fields(&args_of(m));
    if toks.is_empty() {
        return String::new();
    }
    let first = &toks[0];
    let a = args_of(m);
    match a.find(first.as_str()) {
        Some(p) => a[p + first.len()..].trim().to_string(),
        None => String::new(),
    }
}

/// Welcome text parity: "<b>Welcome</b> {user} to {title}". The chat title
/// lookup op is not wired, so the chat id is used as the destination label.
fn welcome_text(_bot: &mut dyn Bot, chat_id: i64, user_id: i64) -> String {
    let mention = crate::utils::html_mention(&format!("user {user_id}"), user_id);
    stylish(&format!(
        "<b>Welcome</b> {mention} to <code>{chat_id}</code>"
    ))
}

/// /welcome on|off|preview
fn welcome_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if m.is_private {
        let _ = bot.reply_text(m, &stylish("This command can only be used in a group."), None);
        return;
    }
    let toks = fields(&args_of(m));
    if toks.is_empty() {
        let state = if db::welcome_on(m.chat_id) { "ON" } else { "OFF" };
        let _ = bot.reply_text(
            m,
            &stylish(&format!(
                "Welcome is currently <b>{state}</b>.\n/welcome on — enable\n/welcome off — disable\n/welcome preview — show it"
            )),
            None,
        );
        return;
    }
    match to_lower(&toks[0]).as_str() {
        "on" => {
            if !require_restrict_right(bot, m) {
                return;
            }
            db::set_welcome_on(m.chat_id, true);
            let _ = bot.reply_text(m, &stylish("Welcome enabled."), None);
        }
        "off" => {
            if !require_restrict_right(bot, m) {
                return;
            }
            db::set_welcome_on(m.chat_id, false);
            let _ = bot.reply_text(m, &stylish("Welcome disabled."), None);
        }
        "preview" => {
            let text = welcome_text(bot, m.chat_id, m.sender_id);
            let _ = bot.reply_text(m, &text, None);
        }
        _ => {
            let _ = bot.reply_text(m, &stylish("Usage: /welcome on|off|preview"), None);
        }
    }
}

/// note_key_from_message: #hashtag or first arg.
fn note_key_from_message(m: &TgMessage) -> String {
    let t = trim(&m.text);
    if !t.is_empty() && t.starts_with('#') {
        let tks = fields(&t[1..]);
        if tks.is_empty() {
            return String::new();
        }
        return to_lower(&tks[0]);
    }
    let toks = fields(&args_of(m));
    if toks.is_empty() {
        return String::new();
    }
    to_lower(&toks[0])
}

fn save_note_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_restrict_right(bot, m) {
        return;
    }
    let toks = fields(&args_of(m));
    if toks.is_empty() {
        let _ = bot.reply_text(m, &stylish("Usage: /save <name> <content>\nOr reply: /save <name>"), None);
        return;
    }
    let key = to_lower(&toks[0]);
    let mut content = args_after_first(m);
    if content.is_empty() && m.reply_to != 0 {
        // Replied-message text fetch op is not wired: fall back to an empty
        // body, matching the "reply only" v1 deviation of the admin family.
        content = String::new();
    }
    if content.is_empty() {
        let _ = bot.reply_text(m, &stylish("Provide content or reply to a message."), None);
        return;
    }
    db::set_note(m.chat_id, &key, &content);
    let _ = bot.reply_text(
        m,
        &stylish(&format!("Note <b>{}</b> saved.", escape_html(&key))),
        None,
    );
}

fn get_note_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if m.is_private {
        return;
    }
    let key = note_key_from_message(m);
    if key.is_empty() {
        let _ = bot.reply_text(m, &stylish("Usage: /get <name> or #name"), None);
        return;
    }
    let content = db::get_note(m.chat_id, &key);
    if content.is_empty() {
        let _ = bot.reply_text(m, &stylish(&format!("No note named <b>{}</b>.", escape_html(&key))), None);
        return;
    }
    let _ = bot.reply_text(
        m,
        &stylish(&format!("<b>{}</b>\n\n{}", escape_html(&key), escape_html(&content))),
        None,
    );
}

fn notes_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if m.is_private {
        let _ = bot.reply_text(m, &stylish("This command can only be used in a group."), None);
        return;
    }
    let names = db::list_notes(m.chat_id);
    if names.is_empty() {
        let _ = bot.reply_text(m, &stylish("No notes saved."), None);
        return;
    }
    let mut text = format!("<b>Notes ({})</b>", names.len());
    for n in &names {
        text.push_str(&format!("\n• #{}", escape_html(n)));
    }
    text.push_str("\n\nUse #name or /get name");
    let _ = bot.reply_text(m, &stylish(&text), None);
}

fn clear_note_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_restrict_right(bot, m) {
        return;
    }
    let toks = fields(&args_of(m));
    if toks.is_empty() {
        let _ = bot.reply_text(m, &stylish("Usage: /clearnote <name>"), None);
        return;
    }
    let key = to_lower(&toks[0]);
    if !db::delete_note(m.chat_id, &key) {
        let _ = bot.reply_text(m, &stylish(&format!("No note named <b>{}</b>.", escape_html(&key))), None);
        return;
    }
    let _ = bot.reply_text(m, &stylish(&format!("Note <b>{}</b> deleted.", escape_html(&key))), None);
}

fn filter_save_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_restrict_right(bot, m) {
        return;
    }
    let toks = fields(&args_of(m));
    if toks.is_empty() {
        let _ = bot.reply_text(
            m,
            &stylish("Usage: /filter <keyword> <reply>\nOr reply to a message: /filter <keyword>"),
            None,
        );
        return;
    }
    let keyword = toks[0].clone();
    let mut text = args_after_first(m);
    if text.is_empty() && m.reply_to != 0 {
        // Replied-message text fetch op is not wired (v1 deviation).
        text = String::new();
    }
    if text.is_empty() {
        let _ = bot.reply_text(m, &stylish("Provide reply text or reply to a message."), None);
        return;
    }
    db::save_filter(m.chat_id, &keyword, &text);
    let _ = bot.reply_text(
        m,
        &stylish(&format!("Filter saved for <code>{}</code>.", escape_html(&to_lower(&keyword)))),
        None,
    );
}

fn filters_list_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if m.is_private {
        let _ = bot.reply_text(m, &stylish("This command can only be used in a group."), None);
        return;
    }
    let rows = db::list_filters(m.chat_id);
    if rows.is_empty() {
        let _ = bot.reply_text(m, &stylish("No filters set.\nAdd one with /filter <keyword> <reply>"), None);
        return;
    }
    let mut text = format!("<b>Filters ({})</b>", rows.len());
    for r in &rows {
        text.push_str(&format!("\n• <code>{}</code>", escape_html(&r.keyword)));
    }
    text.push_str("\n\nDelete with /stopfilter <keyword>");
    let _ = bot.reply_text(m, &stylish(&text), None);
}

fn stop_filter_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_restrict_right(bot, m) {
        return;
    }
    let toks = fields(&args_of(m));
    if toks.is_empty() {
        let _ = bot.reply_text(m, &stylish("Usage: /stopfilter <keyword>"), None);
        return;
    }
    if !db::delete_filter(m.chat_id, &toks[0]) {
        let _ = bot.reply_text(m, &stylish(&format!("No filter named <code>{}</code>.", escape_html(&toks[0]))), None);
        return;
    }
    let _ = bot.reply_text(
        m,
        &stylish(&format!("Filter removed: <code>{}</code>", escape_html(&to_lower(&toks[0])))),
        None,
    );
}

fn clear_filters_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_restrict_right(bot, m) {
        return;
    }
    if m.is_private {
        let _ = bot.reply_text(m, &stylish("This command can only be used in a group."), None);
        return;
    }
    let n = db::clear_filters(m.chat_id);
    let _ = bot.reply_text(m, &stylish(&format!("Removed {n} filter(s) from this chat.")), None);
}

/// Group-text extras parity: #hashtag notes and keyword filters on plain group
/// texts. DB-only, no network.
pub fn group_text_extras(bot: &mut dyn Bot, m: &TgMessage) {
    if m.is_private {
        return;
    }
    let t = trim(&m.text);
    if t.is_empty() {
        return;
    }
    let is_command = t.starts_with('/');
    if !is_command && t.starts_with('#') {
        let tks = fields(&t[1..]);
        if !tks.is_empty() {
            let key = to_lower(&tks[0]);
            let content = db::get_note(m.chat_id, &key);
            if !content.is_empty() {
                let _ = bot.send_text(
                    m.chat_id,
                    &stylish(&format!("<b>{}</b>\n\n{}", escape_html(&key), escape_html(&content))),
                    None,
                );
                return;
            }
        }
    }
    if is_command {
        return;
    }
    if let Some(r) = db::match_filter(m.chat_id, t) {
        let _ = bot.send_text(m.chat_id, &stylish(&r.text), None);
    }
}

/// Registered by init. Message extras (#hashtag notes / filters) ride the
/// registry's Message events via `group_text_extras`.
pub fn register(r: &mut Registry) {
    r.command("welcome", welcome_handler);
    r.command("save", save_note_handler);
    r.command("get", get_note_handler);
    r.command("notes", notes_handler);
    r.command("clearnote", clear_note_handler);
    r.command("filter", filter_save_handler);
    r.command("filters", filters_list_handler);
    r.command("stopfilter", stop_filter_handler);
    r.command("clearfilters", clear_filters_handler);
}
