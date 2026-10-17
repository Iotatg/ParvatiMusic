//! Blacklist registry and the mode commands (adminmode/cleanmode/playmode/
//! nothumb/setcplay/autoplay).
//!
//! Port of the reference admin_lists module: the blacklist chat/user family,
//! the play-mode switches and the channel-play link setup. State lives in the
//! shared db; the mode commands read/write the same process-local maps the
//! settings card uses.

use parvati_mtproto::messaging::TgMessage;

use crate::buttons::{data_btn, rows};
use crate::db;
use crate::mod_tg::Bot;
use crate::modules::common::{f, f_args};
use crate::modules::{cfg, loc, settings_cb, Registry};
use crate::utils::{stylish, to_lower, trim};

// ── shared helpers ─────────────────────────────────────────────────────────────

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

/// The invoked command token with any @botname suffix stripped.
fn get_command(m: &TgMessage) -> String {
    let t = trim(&m.text);
    match t.find(' ') {
        Some(sp) => t[..sp].to_string(),
        None => t.to_string(),
    }
}

fn code_id(id: i64) -> String {
    format!("<code>{id}</code>")
}

/// strict_bool: the accepted on/off vocabulary, None when unrecognized.
fn strict_bool(raw: &str) -> Option<bool> {
    let v = to_lower(trim(raw));
    match v.as_str() {
        "on" | "enable" | "enabled" | "true" | "1" | "yes" | "y" => Some(true),
        "off" | "disable" | "disabled" | "false" | "0" | "no" | "n" => Some(false),
        _ => None,
    }
}

fn enabled_word(chat_id: i64, v: bool) -> String {
    f(loc(), chat_id, if v { "enabled" } else { "disabled" })
}

fn require_owner(bot: &mut dyn Bot, m: &TgMessage) -> bool {
    let c = cfg();
    if c.owner_id == 0 || m.sender_id != c.owner_id {
        let _ = bot.reply_text(m, &f(loc(), m.chat_id, "only_owner"), None);
        return false;
    }
    true
}

fn arg_present(m: &TgMessage) -> bool {
    !fields(&args_of(m)).is_empty() || m.reply_to != 0
}

/// first_arg parity: the first whitespace token after the command.
fn first_arg(m: &TgMessage) -> String {
    let toks = fields(&args_of(m));
    if toks.is_empty() {
        return String::new();
    }
    toks[0].clone()
}

/// parse_int64_arg parity: trims, strips a leading '@', parses as i64.
fn parse_int64_arg(raw: &str) -> Option<i64> {
    let v = trim(raw).trim_start_matches('@');
    if v.is_empty() {
        return None;
    }
    v.parse::<i64>().ok()
}

/// extract_target_user parity: replied messages resolve to their sender;
/// otherwise a numeric user id; usernames are unsupported.
fn extract_target_user(bot: &mut dyn Bot, m: &TgMessage) -> Result<i64, String> {
    if m.reply_to != 0 {
        let uid = bot.get_replied_sender(m.chat_id, m.reply_to);
        if uid <= 0 {
            return Err("failed to fetch replied message".into());
        }
        return Ok(uid);
    }
    let toks = fields(&args_of(m));
    if toks.is_empty() {
        return Err("no user identifier found".into());
    }
    parse_int64_arg(&toks[0])
        .ok_or_else(|| "username lookup is not supported, use a numeric user id".into())
}

// ── blacklist ──────────────────────────────────────────────────────────────────

fn block_user_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_owner(bot, m) {
        return;
    }
    if !arg_present(m) {
        let text = f_args(loc(), m.chat_id, "auth_no_user", &[("cmd", &get_command(m))]);
        let _ = bot.reply_text(m, &text, None);
        return;
    }
    let target = match extract_target_user(bot, m) {
        Ok(t) => t,
        Err(e) => {
            let text = f_args(loc(), m.chat_id, "user_extract_fail", &[("error", &e)]);
            let _ = bot.reply_text(m, &text, None);
            return;
        }
    };
    if !db::add_blacklisted_user(target) {
        let _ = bot.reply_text(m, &stylish("Failed to block user: storage error"), None);
        return;
    }
    let text = f_args(loc(), m.chat_id, "blacklist_block_user_success", &[("id", &target.to_string())]);
    let _ = bot.reply_text(m, &text, None);
}

fn unblock_user_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_owner(bot, m) {
        return;
    }
    if !arg_present(m) {
        let text = f_args(loc(), m.chat_id, "auth_no_user", &[("cmd", &get_command(m))]);
        let _ = bot.reply_text(m, &text, None);
        return;
    }
    let target = match extract_target_user(bot, m) {
        Ok(t) => t,
        Err(e) => {
            let text = f_args(loc(), m.chat_id, "user_extract_fail", &[("error", &e)]);
            let _ = bot.reply_text(m, &text, None);
            return;
        }
    };
    if !db::del_blacklisted_user(target) {
        let text = f_args(loc(), m.chat_id, "blacklist_unblock_user_fail", &[("error", "storage error")]);
        let _ = bot.reply_text(m, &text, None);
        return;
    }
    let text = f_args(loc(), m.chat_id, "blacklist_unblock_user_success", &[("id", &target.to_string())]);
    let _ = bot.reply_text(m, &text, None);
}

fn block_chat_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_owner(bot, m) {
        return;
    }
    let arg = first_arg(m);
    if arg.is_empty() {
        let text = f(loc(), m.chat_id, "blacklist_usage_blockchat");
        let _ = bot.reply_text(m, &text, None);
        return;
    }
    let Some(target) = parse_int64_arg(&arg) else {
        let text = f_args(
            loc(),
            m.chat_id,
            "blacklist_invalid_chat_identifier",
            &[("error", "username lookup is not supported, use a numeric chat id")],
        );
        let _ = bot.reply_text(m, &text, None);
        return;
    };
    if !db::add_blacklisted_chat(target) {
        let _ = bot.reply_text(m, &stylish("Failed to block chat: storage error"), None);
        return;
    }
    let text = f_args(loc(), m.chat_id, "blacklist_block_chat_success", &[("id", &target.to_string())]);
    let _ = bot.reply_text(m, &text, None);
}

fn unblock_chat_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_owner(bot, m) {
        return;
    }
    let arg = first_arg(m);
    if arg.is_empty() {
        let text = f(loc(), m.chat_id, "blacklist_usage_unblockchat");
        let _ = bot.reply_text(m, &text, None);
        return;
    }
    let Some(target) = parse_int64_arg(&arg) else {
        let text = f_args(
            loc(),
            m.chat_id,
            "blacklist_invalid_chat_identifier",
            &[("error", "username lookup is not supported, use a numeric chat id")],
        );
        let _ = bot.reply_text(m, &text, None);
        return;
    };
    if !db::del_blacklisted_chat(target) {
        let text = f_args(loc(), m.chat_id, "blacklist_unblock_chat_fail", &[("error", "storage error")]);
        let _ = bot.reply_text(m, &text, None);
        return;
    }
    let text = f_args(loc(), m.chat_id, "blacklist_unblock_chat_success", &[("id", &target.to_string())]);
    let _ = bot.reply_text(m, &text, None);
}

fn blacklisted_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_owner(bot, m) {
        return;
    }
    let chat_id = m.chat_id;
    let chats = db::blacklisted_chats();
    let users = db::blacklisted_users();
    let mut sb = f(loc(), chat_id, "blacklist_list_title");
    sb.push_str("\n\n");
    sb.push_str(&f(loc(), chat_id, "blacklist_list_chats"));
    sb.push('\n');
    if chats.is_empty() {
        sb.push_str("\u{2022} None\n");
    } else {
        for (i, c) in chats.iter().enumerate() {
            sb.push_str(&format!("{}. {}\n", i + 1, code_id(*c)));
        }
    }
    sb.push('\n');
    sb.push_str(&f(loc(), chat_id, "blacklist_list_users"));
    sb.push('\n');
    if users.is_empty() {
        sb.push_str("\u{2022} None");
    } else {
        for (i, u) in users.iter().enumerate() {
            sb.push_str(&format!("{}. {}\n", i + 1, code_id(*u)));
        }
    }
    let _ = bot.reply_text(m, &sb, None);
}

// ── modes ──────────────────────────────────────────────────────────────────────

/// group_gate parity used by mode commands: supergroup-only, registers served.
fn group_gate(bot: &mut dyn Bot, m: &TgMessage) -> bool {
    if m.is_private {
        let _ = bot.reply_text(m, &f(loc(), m.chat_id, "only_supergroup"), None);
        db::add_served_user(m.chat_id);
        return false;
    }
    if m.is_channel {
        return false;
    }
    db::add_served_chat(m.chat_id);
    true
}

fn playmode_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !group_gate(bot, m) {
        return;
    }
    let chat_id = m.chat_id;
    let toks = fields(&args_of(m));
    let current = db::play_mode_admins_only(chat_id);
    if toks.is_empty() {
        let key = if current {
            "playmode_status_admins"
        } else {
            "playmode_status_everyone"
        };
        let text = f_args(loc(), chat_id, "playmode_help", &[("status", &f(loc(), chat_id, key))]);
        let _ = bot.reply_text(m, &text, None);
        return;
    }
    let Some(admins_only) = strict_bool(&toks[0]) else {
        let _ = bot.reply_text(m, &f(loc(), chat_id, "invalid_bool"), None);
        return;
    };
    db::set_play_mode_admins_only(chat_id, admins_only);
    let key = if admins_only {
        "playmode_success_admins"
    } else {
        "playmode_success_everyone"
    };
    let _ = bot.reply_text(m, &f(loc(), chat_id, key), None);
}

fn nothumb_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !group_gate(bot, m) {
        return;
    }
    let chat_id = m.chat_id;
    let toks = fields(&args_of(m));
    let current = db::thumbnails_disabled(chat_id);
    if toks.is_empty() {
        let text = f_args(
            loc(),
            chat_id,
            "nothumb_status",
            &[
                ("cmd", &get_command(m)),
                ("action", if current { "disabled" } else { "enabled" }),
            ],
        );
        let _ = bot.reply_text(m, &text, None);
        return;
    }
    let Some(value) = strict_bool(&toks[0]) else {
        let _ = bot.reply_text(m, &f(loc(), chat_id, "invalid_bool"), None);
        return;
    };
    let action = if value { "disabled" } else { "enabled" };
    if value == current {
        let text = f_args(loc(), chat_id, "nothumb_already", &[("action", action)]);
        let _ = bot.reply_text(m, &text, None);
        return;
    }
    db::set_thumbnails_disabled(chat_id, value);
    let text = f_args(loc(), chat_id, "nothumb_updated", &[("action", action)]);
    let _ = bot.reply_text(m, &text, None);
}

fn clean_mode_text(chat_id: i64, enabled: bool) -> String {
    let status = f_args(
        loc(),
        chat_id,
        "cleanmode_status",
        &[("action", &enabled_word(chat_id, enabled)), ("duration", "15")],
    );
    format!("{status}\n\n{}", f(loc(), chat_id, "cleanmode_hint"))
}

fn clean_mode_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !group_gate(bot, m) {
        return;
    }
    let chat_id = m.chat_id;
    let toks = fields(&args_of(m));
    let current = settings_cb::cleanmode(chat_id);
    if toks.is_empty() {
        let _ = bot.reply_text(m, &clean_mode_text(chat_id, current), None);
        return;
    }
    let Some(enabled) = strict_bool(&toks[0]) else {
        let _ = bot.reply_text(m, &f(loc(), chat_id, "invalid_bool"), None);
        return;
    };
    settings_cb::set_cleanmode(chat_id, enabled);
    let _ = bot.reply_text(m, &clean_mode_text(chat_id, enabled), None);
}

fn admin_mode_status_key(mode: &str) -> &'static str {
    match mode {
        "admins" => "adminmode_status_admin",
        "everyone" => "adminmode_status_everyone",
        _ => "adminmode_status_adminauth",
    }
}

fn parse_admin_mode(input: &str) -> Option<&'static str> {
    match to_lower(trim(input)).as_str() {
        "admin" | "admins" | "adminonly" | "adminsonly" | "admins_only" => Some("admins"),
        "adminauth" | "auth" | "admin+auth" | "dj" | "admin_auth" => Some("adminauth"),
        "everyone" | "all" => Some("everyone"),
        _ => None,
    }
}

fn admin_mode_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !group_gate(bot, m) {
        return;
    }
    let chat_id = m.chat_id;
    let toks = fields(&args_of(m));
    let current = settings_cb::adminmode(chat_id);
    if toks.is_empty() {
        let status = f(loc(), chat_id, admin_mode_status_key(&current));
        let text = f_args(loc(), chat_id, "adminmode_help", &[("status", &status)]);
        let _ = bot.reply_text(m, &text, None);
        return;
    }
    let Some(mode) = parse_admin_mode(&toks[0]) else {
        let _ = bot.reply_text(m, &f(loc(), chat_id, "adminmode_invalid"), None);
        return;
    };
    settings_cb::set_adminmode(chat_id, mode);
    let status = f(loc(), chat_id, admin_mode_status_key(mode));
    let text = f_args(loc(), chat_id, "adminmode_updated", &[("status", &status)]);
    let _ = bot.reply_text(m, &text, None);
}

fn set_cplay_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !group_gate(bot, m) {
        return;
    }
    let chat_id = m.chat_id;
    let toks = fields(&args_of(m));
    if toks.is_empty() {
        let _ = bot.reply_text(m, &f(loc(), chat_id, "cplay_usage"), None);
        return;
    }
    let arg = to_lower(trim(&toks[0]));
    let flag = strict_bool(&arg);
    let clear = arg == "reset" || arg == "none" || arg == "del" || flag == Some(false);
    if clear {
        db::set_linked_channel(chat_id, 0);
        let _ = bot.reply_text(m, &f(loc(), chat_id, "cplay_disabled"), None);
        return;
    }
    if arg == "linked" {
        let _ = bot.reply_text(m, &f(loc(), chat_id, "cplay_resolve_peer_fail"), None);
        return;
    }
    match parse_int64_arg(&arg) {
        Some(target) if target < 0 => {
            db::set_linked_channel(chat_id, target);
            let text = f_args(loc(), chat_id, "cplay_enabled", &[("channel_id", &target.to_string())]);
            let _ = bot.reply_text(m, &text, None);
        }
        _ => {
            let _ = bot.reply_text(m, &f(loc(), chat_id, "cplay_invalid_target"), None);
        }
    }
}

/// autoplay_menu: toggle the room/db flag, with a close row.
fn autoplay_menu(chat_id: i64, enabled: bool) -> parvati_mtproto::messaging::Markup {
    let state = enabled_word(chat_id, enabled);
    let text = f_args(loc(), chat_id, "autoplay_btn", &[("state", &state)]);
    rows(vec![
        vec![data_btn(&text, "autoplay_toggle")],
        vec![data_btn(&f(loc(), chat_id, "CLOSE_BTN"), "close")],
    ])
}

fn autoplay_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !group_gate(bot, m) {
        return;
    }
    let chat_id = m.chat_id;
    let enabled = match crate::modules::rooms().and_then(|rs| rs.room_for(chat_id)) {
        Some(room) => {
            let g = room.lock().unwrap_or_else(|e| e.into_inner());
            if g.active() {
                g.autoplay
            } else {
                db::autoplay(chat_id)
            }
        }
        None => db::autoplay(chat_id),
    };
    let text = f_args(loc(), chat_id, "autoplay_menu", &[("state", &enabled_word(chat_id, enabled))]);
    let _ = bot.reply_text(m, &text, Some(&autoplay_menu(chat_id, enabled)));
}

/// #hashtag/filter extras are wired in moderation.rs; this module only owns the
/// command surface.
pub fn register(r: &mut Registry) {
    r.command("blockuser", block_user_handler);
    r.command("blacklistuser", block_user_handler);
    r.command("blackuser", block_user_handler);
    r.command("bluser", block_user_handler);
    r.command("unblockuser", unblock_user_handler);
    r.command("unblacklistuser", unblock_user_handler);
    r.command("unbluser", unblock_user_handler);
    r.command("whitelistuser", unblock_user_handler);
    r.command("blockchat", block_chat_handler);
    r.command("blacklistchat", block_chat_handler);
    r.command("blackchat", block_chat_handler);
    r.command("blchat", block_chat_handler);
    r.command("unblockchat", unblock_chat_handler);
    r.command("unblackchat", unblock_chat_handler);
    r.command("unblacklistchat", unblock_chat_handler);
    r.command("unblchat", unblock_chat_handler);
    r.command("whitechat", unblock_chat_handler);
    r.command("blocked", blacklisted_handler);
    r.command("blacklisted", blacklisted_handler);

    r.command("adminmode", admin_mode_handler);
    r.command("cleanmode", clean_mode_handler);
    r.command("playmode", playmode_handler);
    r.command("nothumb", nothumb_handler);
    r.command("nothumbs", nothumb_handler);
    r.command("setcplay", set_cplay_handler);
    r.command("channelplay", set_cplay_handler);
    r.command("autoplay", autoplay_handler);
}
