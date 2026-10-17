//! Admin moderation subset: group admin ops (promote/demote levels, the
//! ban/kick/warn family, pins, purge), the sudo + auth registries and the
//! broadcast fan-out.
//!
//! V1 deviations (tracked by the parity matrix):
//!   * target resolution accepts numeric ids only (no search_public_chat /
//!     get_replied_message ops yet; non-numeric tokens report "invalid user id")
//!   * mentions render as <code>id</code> (no get_user op for display names)
//!   * purge enumerates ids in range start..=reply (no paged history-read op
//!     exists in this build to skip service-message id gaps)
//!   * broadcast fans out synchronously with no inter-message delay and the
//!     initializing reply omits the inline cancel keyboard; the bcast_cancel
//!     callback and the -cancel flag still work
//!   * the sudoers listing skips the 30s flood guard
//!
//! The blacklist/blockchat-user family lives in `lists.rs`; the eval/sh/log/
//! restart/vclogger console family lives in `system.rs`.
//!
//! Gate order: group-only, bot right, caller right, with the owner/sudo
//! bypass in between. Message strings run through utils::stylish.

use std::sync::atomic::{AtomicBool, Ordering};

use parvati_mtproto::messaging::{TgCallback, TgMessage};

use crate::db;
use crate::mod_tg::{Bot, MemberState, PromoteRights};
use crate::modules::common::{command_of, f, f_args};
use crate::modules::{cfg, loc, Registry};
use crate::utils::{escape_html, stylish, to_lower, trim};

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

/// parse_duration_token parity: digits plus one unit of m/h/d only.
fn parse_duration_token(tok: &str) -> Option<(i64, String)> {
    let b = tok.as_bytes();
    if b.len() < 2 {
        return None;
    }
    let mut v: i64 = 0;
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        v = v * 10 + (b[i] - b'0') as i64;
        i += 1;
    }
    if i == 0 || i + 1 != b.len() || v <= 0 {
        return None;
    }
    let secs = match b[i] {
        b'm' | b'M' => v * 60,
        b'h' | b'H' => v * 3600,
        b'd' | b'D' => v * 86400,
        _ => return None,
    };
    if secs > i32::MAX as i64 {
        return None;
    }
    Some((secs, format!("{}{}", v, b[i] as char)))
}

/// v1 mention: the get_user op is not wired, so targets render as code ids.
fn mention(user_id: i64) -> String {
    format!("<code>{user_id}</code>")
}

fn code_id(user_id: i64) -> String {
    format!("<code>{user_id}</code>")
}

fn member_of(bot: &mut dyn Bot, chat_id: i64, user_id: i64) -> MemberState {
    bot.get_chat_member_state(chat_id, user_id)
}

pub(crate) fn caller_is_owner_or_sudo(m: &TgMessage) -> bool {
    c_owner_is(m.sender_id) || db::is_sudo(m.sender_id)
}

fn c_owner_is(sender: i64) -> bool {
    let c = cfg();
    c.owner_id != 0 && c.owner_id == sender
}

/// require_chat_flag parity: group-only, bot right, owner/sudo bypass, then
/// the caller right.
fn require_chat_flag(
    bot: &mut dyn Bot,
    m: &TgMessage,
    need: &str,
    select: fn(&MemberState) -> bool,
) -> bool {
    if m.is_private {
        let _ = bot.reply_text(m, &stylish("This command can only be used in a group."), None);
        return false;
    }
    let me = bot.me_id();
    if !select(&member_of(bot, m.chat_id, me)) {
        let _ = bot.reply_text(
            m,
            &stylish(&format!(
                "I need {need} permission to do that. Promote me as an admin first."
            )),
            None,
        );
        return false;
    }
    if caller_is_owner_or_sudo(m) {
        return true;
    }
    if !select(&member_of(bot, m.chat_id, m.sender_id)) {
        let _ = bot.reply_text(
            m,
            &stylish(&format!("You need {need} permission to use this command.")),
            None,
        );
        return false;
    }
    true
}

pub(crate) fn require_restrict_right(bot: &mut dyn Bot, m: &TgMessage) -> bool {
    require_chat_flag(bot, m, "restrict members", |r| r.can_restrict)
}

pub(crate) fn require_promote_right(bot: &mut dyn Bot, m: &TgMessage) -> bool {
    require_chat_flag(bot, m, "promote members", |r| r.can_promote)
}

pub(crate) fn require_pin_right(bot: &mut dyn Bot, m: &TgMessage) -> bool {
    require_chat_flag(bot, m, "pin messages", |r| r.can_pin)
}

pub(crate) fn require_delete_right(bot: &mut dyn Bot, m: &TgMessage) -> bool {
    require_chat_flag(bot, m, "delete messages", |r| r.can_delete)
}

/// require_owner parity: replies with the locale line and returns false when
/// the sender is not the configured owner.
pub(crate) fn require_owner(bot: &mut dyn Bot, m: &TgMessage) -> bool {
    let c = cfg();
    if c.owner_id == 0 || m.sender_id != c.owner_id {
        crate::parv_warn!(
            "owner",
            "admin::require_owner denied: sender_id={} owner_id={} chat_id={}",
            m.sender_id,
            c.owner_id,
            m.chat_id
        );
        let _ = bot.reply_text(m, &f(loc(), m.chat_id, "only_owner"), None);
        return false;
    }
    true
}

fn is_telegram_admin(bot: &mut dyn Bot, chat_id: i64, user_id: i64) -> bool {
    member_of(bot, chat_id, user_id).admin
}

/// protected_target parity: the bot itself, the owner and sudoers are safe.
fn protected_target(bot: &mut dyn Bot, user_id: i64) -> Option<String> {
    if user_id == bot.me_id() {
        return Some("I can't perform that action on myself.".into());
    }
    if c_owner_is(user_id) {
        return Some("That user is protected and can't be targeted.".into());
    }
    if db::is_sudo(user_id) {
        return Some("That user is protected and can't be targeted.".into());
    }
    None
}

/// v1 resolve_from_arg parity (numeric ids only; @username lookups need a
/// search op this build does not have yet).
fn resolve_target(user_args: &[String]) -> Result<i64, String> {
    let Some(first) = user_args.first() else {
        return Err("no target".into());
    };
    let arg = trim(first).trim_start_matches('@');
    if arg.is_empty() {
        return Err("username cannot be empty".into());
    }
    if arg.bytes().all(|b| b.is_ascii_digit()) {
        let id: i64 = arg.parse().unwrap_or(0);
        return if id <= 0 { Err("invalid user id".into()) } else { Ok(id) };
    }
    Err("invalid user id".into())
}

fn resolve_admin_target(bot: &mut dyn Bot, m: &TgMessage, user_args: &[String]) -> Result<i64, String> {
    if m.reply_to != 0 {
        let uid = bot.get_replied_sender(m.chat_id, m.reply_to);
        if uid <= 0 {
            return Err("failed to fetch replied message".into());
        }
        return Ok(uid);
    }
    resolve_target(user_args)
}

fn target_usage(usage: &str) -> String {
    stylish(&format!("Reply to a user or pass @username / id.\nUsage: {usage}"))
}

// ── promote / demote ──────────────────────────────────────────────────────────

/// promote_level parity: 1 junior, 2 senior, 3 full.
fn promote_level(level: i64) -> Option<(&'static str, PromoteRights)> {
    let r = |manage: bool,
             change_info: bool,
             del: bool,
             invite: bool,
             restrict: bool,
             pin: bool,
             promote: bool,
             video: bool| PromoteRights {
        manage,
        change_info,
        del,
        invite,
        restrict,
        pin,
        promote,
        video,
    };
    match level {
        1 => Some(("Junior Admin", r(false, false, false, true, false, true, false, false))),
        2 => Some(("Senior Admin", r(false, true, true, true, true, true, false, false))),
        3 => Some(("Full Admin", r(true, true, true, true, true, true, false, true))),
        _ => None,
    }
}

fn promote_rights() -> PromoteRights {
    PromoteRights {
        manage: true,
        change_info: true,
        del: true,
        invite: true,
        restrict: true,
        pin: true,
        promote: false,
        video: true,
    }
}

fn promote_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_promote_right(bot, m) {
        return;
    }
    let mut level: i64 = 0;
    let mut user_args: Vec<String> = Vec::new();
    for t in fields(&args_of(m)) {
        if level == 0 && (t == "1" || t == "2" || t == "3") {
            level = t.as_bytes()[0] as i64 - '0' as i64;
            continue;
        }
        user_args.push(t);
    }
    let uid = match resolve_admin_target(bot, m, &user_args) {
        Ok(u) => u,
        Err(_) => {
            let _ = bot.reply_text(m, &target_usage("/promote [user] [1|2|3]"), None);
            return;
        }
    };
    if let Some(reason) = protected_target(bot, uid) {
        let _ = bot.reply_text(m, &stylish(&reason), None);
        return;
    }
    let (title, rights) = match level {
        0 => (String::new(), promote_rights()),
        l if l > 3 => {
            let _ = bot.reply_text(m, &stylish("Promote level must be 1, 2, or 3."), None);
            return;
        }
        l => {
            let (t, r) = promote_level(l)
                .ok_or(())
                .expect("level is 1..=3 here");
            (t.to_string(), r)
        }
    };
    if bot.promote_member(m.chat_id, uid, &rights).is_err() {
        let _ = bot.reply_text(m, &stylish("Failed to promote: telegram_error"), None);
        return;
    }
    if title.is_empty() {
        let _ = bot.reply_text(m, &stylish(&format!("Promoted {} to admin.", mention(uid))), None);
    } else {
        let _ = bot.reply_text(
            m,
            &stylish(&format!("Promoted {} to {title}.", mention(uid))),
            None,
        );
    }
}

fn full_promote_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_promote_right(bot, m) {
        return;
    }
    let uid = match resolve_admin_target(bot, m, &fields(&args_of(m))) {
        Ok(u) => u,
        Err(_) => {
            let _ = bot.reply_text(m, &target_usage("/fullpromote [user]"), None);
            return;
        }
    };
    if let Some(reason) = protected_target(bot, uid) {
        let _ = bot.reply_text(m, &stylish(&reason), None);
        return;
    }
    let rights = PromoteRights {
        manage: true,
        change_info: true,
        del: true,
        invite: true,
        restrict: true,
        pin: true,
        promote: true,
        video: true,
    };
    if bot.promote_member(m.chat_id, uid, &rights).is_err() {
        let _ = bot.reply_text(m, &stylish("Failed to promote: telegram_error"), None);
        return;
    }
    let _ = bot.reply_text(m, &stylish(&format!("Fully promoted {}.", mention(uid))), None);
}

fn demote_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_promote_right(bot, m) {
        return;
    }
    let uid = match resolve_admin_target(bot, m, &fields(&args_of(m))) {
        Ok(u) => u,
        Err(_) => {
            let _ = bot.reply_text(m, &target_usage("/demote [user]"), None);
            return;
        }
    };
    if let Some(reason) = protected_target(bot, uid) {
        let _ = bot.reply_text(m, &stylish(&reason), None);
        return;
    }
    if bot.demote_member(m.chat_id, uid).is_err() {
        let _ = bot.reply_text(m, &stylish("Failed to demote: telegram_error"), None);
        return;
    }
    let _ = bot.reply_text(m, &stylish(&format!("Demoted {}.", mention(uid))), None);
}

// ── ban / kick / unban ────────────────────────────────────────────────────────

fn ban_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_restrict_right(bot, m) {
        return;
    }
    let mut dur = (0i64, String::new());
    let mut user_args: Vec<String> = Vec::new();
    for t in fields(&args_of(m)) {
        match parse_duration_token(&t) {
            Some(d) => dur = d,
            None => user_args.push(t),
        }
    }
    let uid = match resolve_admin_target(bot, m, &user_args) {
        Ok(u) => u,
        Err(_) => {
            let _ = bot.reply_text(m, &target_usage("/ban [user] [10m|2h|1d]"), None);
            return;
        }
    };
    if let Some(reason) = protected_target(bot, uid) {
        let _ = bot.reply_text(m, &stylish(&reason), None);
        return;
    }
    if is_telegram_admin(bot, m.chat_id, uid) {
        let _ = bot.reply_text(m, &stylish("You can't restrict an admin."), None);
        return;
    }
    if bot.ban_member(m.chat_id, uid, dur.0).is_err() {
        let _ = bot.reply_text(m, &stylish("Failed to ban: telegram_error"), None);
        return;
    }
    let suffix = if dur.1.is_empty() { String::new() } else { format!(" ({})", dur.1) };
    let _ = bot.reply_text(m, &stylish(&format!("Banned {}{suffix}.", mention(uid))), None);
}

fn unban_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_restrict_right(bot, m) {
        return;
    }
    let uid = match resolve_admin_target(bot, m, &fields(&args_of(m))) {
        Ok(u) => u,
        Err(_) => {
            let _ = bot.reply_text(m, &target_usage("/unban [user]"), None);
            return;
        }
    };
    if bot.unban_member(m.chat_id, uid).is_err() {
        let _ = bot.reply_text(m, &stylish("Failed to unban: telegram_error"), None);
        return;
    }
    let _ = bot.reply_text(m, &stylish(&format!("Unbanned {}.", mention(uid))), None);
}

fn kick_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_restrict_right(bot, m) {
        return;
    }
    let uid = match resolve_admin_target(bot, m, &fields(&args_of(m))) {
        Ok(u) => u,
        Err(_) => {
            let _ = bot.reply_text(m, &target_usage("/kick [user]"), None);
            return;
        }
    };
    if let Some(reason) = protected_target(bot, uid) {
        let _ = bot.reply_text(m, &stylish(&reason), None);
        return;
    }
    if is_telegram_admin(bot, m.chat_id, uid) {
        let _ = bot.reply_text(m, &stylish("You can't restrict an admin."), None);
        return;
    }
    if bot.ban_member(m.chat_id, uid, 0).is_err() {
        let _ = bot.reply_text(m, &stylish("Failed to kick: telegram_error"), None);
        return;
    }
    let _ = bot.unban_member(m.chat_id, uid);
    let _ = bot.reply_text(m, &stylish(&format!("Kicked {}.", mention(uid))), None);
}

// ── warns ─────────────────────────────────────────────────────────────────────

/// warnReasonFromArgs parity: without reply support the first token is the
/// user and the rest is the reason.
fn warn_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_restrict_right(bot, m) {
        return;
    }
    let toks = fields(&args_of(m));
    let mut user_args: Vec<String> = Vec::new();
    let mut reason = String::new();
    if let Some(first) = toks.first() {
        user_args.push(first.clone());
        reason = toks[1..].join(" ");
    }
    let uid = match resolve_admin_target(bot, m, &user_args) {
        Ok(u) => u,
        Err(_) => {
            let _ = bot.reply_text(m, &target_usage("/warn [user] [reason]"), None);
            return;
        }
    };
    if let Some(pr) = protected_target(bot, uid) {
        let _ = bot.reply_text(m, &stylish(&pr), None);
        return;
    }
    if is_telegram_admin(bot, m.chat_id, uid) {
        let _ = bot.reply_text(m, &stylish("You can't warn an admin."), None);
        return;
    }
    let count = db::add_warning(m.chat_id, uid, m.sender_id, &reason);
    let limit = db::warn_limit(m.chat_id);
    let mode = db::warn_mode(m.chat_id);
    let mut text = format!("Warned {} ({count}/{limit}).", mention(uid));
    if !reason.is_empty() {
        text += &format!("\nReason: {}", escape_html(&reason));
    }
    if count >= limit {
        let penalty = match mode.as_str() {
            "mute" => bot.mute_member(m.chat_id, uid),
            "kick" => bot
                .ban_member(m.chat_id, uid, 0)
                .and_then(|_| bot.unban_member(m.chat_id, uid)),
            _ => bot.ban_member(m.chat_id, uid, 0),
        };
        match penalty {
            Ok(_) => {
                db::clear_warnings(m.chat_id, uid);
                text += &format!("\nLimit reached — user was {mode}.");
            }
            Err(e) => text += &format!("\nLimit reached, but penalty failed: {}", escape_html(&e)),
        }
    }
    let _ = bot.reply_text(m, &stylish(&text), None);
}

fn unwarn_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_restrict_right(bot, m) {
        return;
    }
    let uid = match resolve_admin_target(bot, m, &fields(&args_of(m))) {
        Ok(u) => u,
        Err(_) => {
            let _ = bot.reply_text(m, &target_usage("/unwarn [user]"), None);
            return;
        }
    };
    if !db::remove_last_warning(m.chat_id, uid) {
        let _ = bot.reply_text(
            m,
            &stylish(&format!("{} has no warnings to remove.", mention(uid))),
            None,
        );
        return;
    }
    let count = db::count_warnings(m.chat_id, uid);
    let limit = db::warn_limit(m.chat_id);
    let _ = bot.reply_text(
        m,
        &stylish(&format!(
            "Removed last warning from {} ({count}/{limit}).",
            mention(uid)
        )),
        None,
    );
}

fn warns_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if m.is_private {
        let _ = bot.reply_text(m, &stylish("This command can only be used in a group."), None);
        return;
    }
    let mut uid = m.sender_id;
    let toks = fields(&args_of(m));
    if m.reply_to != 0 || !toks.is_empty() {
        uid = match resolve_admin_target(bot, m, &toks) {
            Ok(u) => u,
            Err(_) => {
                let _ = bot.reply_text(m, &target_usage("/warns [user]"), None);
                return;
            }
        };
    }
    let count = db::count_warnings(m.chat_id, uid);
    let limit = db::warn_limit(m.chat_id);
    let mode = db::warn_mode(m.chat_id);
    let mut text = format!(
        "{}: <b>{count}/{limit}</b> warnings (mode: {})",
        mention(uid),
        escape_html(&mode)
    );
    let rows = db::warnings_list(m.chat_id, uid);
    for row in rows.iter().take(8) {
        let reason = if row.reason.is_empty() {
            "no reason".to_string()
        } else {
            escape_html(&row.reason)
        };
        text += &format!("\n\u{2022} {reason}");
    }
    if rows.len() > 8 {
        text += &format!("\n...and {} more", rows.len() - 8);
    }
    let _ = bot.reply_text(m, &stylish(&text), None);
}

fn clear_warn_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_restrict_right(bot, m) {
        return;
    }
    let uid = match resolve_admin_target(bot, m, &fields(&args_of(m))) {
        Ok(u) => u,
        Err(_) => {
            let _ = bot.reply_text(m, &target_usage("/clearwarn [user]"), None);
            return;
        }
    };
    let n = db::clear_warnings(m.chat_id, uid);
    let _ = bot.reply_text(
        m,
        &stylish(&format!("Cleared {n} warning(s) from {}.", mention(uid))),
        None,
    );
}

fn reset_warns_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if m.is_private {
        let _ = bot.reply_text(m, &stylish("This command can only be used in a group."), None);
        return;
    }
    if !require_restrict_right(bot, m) {
        return;
    }
    let n = db::clear_all_warnings(m.chat_id);
    let _ = bot.reply_text(m, &stylish(&format!("Cleared {n} warning(s) in this chat.")), None);
}

fn warn_limit_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if m.is_private {
        let _ = bot.reply_text(m, &stylish("This command can only be used in a group."), None);
        return;
    }
    if !require_restrict_right(bot, m) {
        return;
    }
    let toks = fields(&args_of(m));
    if toks.is_empty() {
        let _ = bot.reply_text(
            m,
            &stylish(&format!(
                "Warn limit: <b>{}</b>\nMode: <b>{}</b>\nUsage: /warnlimit [2-20]",
                db::warn_limit(m.chat_id),
                escape_html(&db::warn_mode(m.chat_id))
            )),
            None,
        );
        return;
    }
    let n: i64 = toks[0].trim().parse().unwrap_or(-1);
    if !(2..=20).contains(&n) {
        let _ = bot.reply_text(m, &stylish("Warn limit must be a number from 2 to 20."), None);
        return;
    }
    db::set_warn_limit(m.chat_id, n);
    let _ = bot.reply_text(m, &stylish(&format!("Warn limit set to <b>{n}</b>.")), None);
}

fn set_warn_mode_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if m.is_private {
        let _ = bot.reply_text(m, &stylish("This command can only be used in a group."), None);
        return;
    }
    if !require_restrict_right(bot, m) {
        return;
    }
    let toks = fields(&args_of(m));
    if toks.is_empty() {
        let _ = bot.reply_text(m, &stylish("Usage: /setwarnmode ban|mute|kick"), None);
        return;
    }
    let mode = to_lower(&toks[0]);
    if mode != "ban" && mode != "mute" && mode != "kick" {
        let _ = bot.reply_text(m, &stylish("Warn mode must be ban, mute, or kick."), None);
        return;
    }
    db::set_warn_mode(m.chat_id, &mode);
    let _ = bot.reply_text(m, &stylish(&format!("Warn mode set to <b>{mode}</b>.")), None);
}

// ── pins / purge ──────────────────────────────────────────────────────────────

fn pin_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_pin_right(bot, m) {
        return;
    }
    if m.reply_to == 0 {
        let _ = bot.reply_text(
            m,
            &stylish("Reply to a message to pin it.\nUsage: /pin [silent|quiet|s]"),
            None,
        );
        return;
    }
    let mut silent = false;
    for t in fields(&args_of(m)) {
        let l = to_lower(&t);
        if l == "silent" || l == "quiet" || l == "s" {
            silent = true;
        }
    }
    if bot.pin_message(m.chat_id, m.reply_to, silent).is_err() {
        let _ = bot.reply_text(m, &stylish("Failed to pin."), None);
        return;
    }
    let _ = bot.reply_text(m, &stylish("Message pinned."), None);
}

fn unpin_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_pin_right(bot, m) {
        return;
    }
    let id = if m.reply_to != 0 { m.reply_to } else { 0 };
    if bot.unpin_message(m.chat_id, id).is_err() {
        let _ = bot.reply_text(m, &stylish("Failed to unpin."), None);
        return;
    }
    let _ = bot.reply_text(m, &stylish("Message unpinned."), None);
}

fn unpin_all_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if m.is_private {
        let _ = bot.reply_text(m, &stylish("This command can only be used in a group."), None);
        return;
    }
    if !require_pin_right(bot, m) {
        return;
    }
    if bot.unpin_all_messages(m.chat_id).is_err() {
        let _ = bot.reply_text(m, &stylish("Failed to unpin."), None);
        return;
    }
    let _ = bot.reply_text(m, &stylish("All pinned messages were unpinned."), None);
}

/// purge v1: message ids are enumerated across the reply range (no paged
/// history-read op exists in this build to skip service-message id gaps).
/// Deletes run in chunks of 100.
fn purge_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_delete_right(bot, m) {
        return;
    }
    let start_id = m.reply_to;
    if start_id == 0 {
        let _ = bot.reply_text(m, &stylish("Reply to the message you want to purge from."), None);
        return;
    }
    let end_id = m.id;
    if end_id <= start_id {
        let _ = bot.reply_text(m, &stylish("Nothing to purge."), None);
        return;
    }
    let ids: Vec<i64> = (start_id..=end_id).take(100).collect();
    let mut deleted = 0i64;
    for id in ids {
        if bot.delete_message(m.chat_id, id).unwrap_or(false) {
            deleted += 1;
        }
    }
    let _ = bot.send_text_sync(
        m.chat_id,
        &stylish(&format!("Purged {deleted} message(s).")),
        None,
        0,
    );
}

// ── sudo family ─────────────────────────────────────────────

fn add_sudo_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_owner(bot, m) {
        return;
    }
    let chat_id = m.chat_id;
    let toks = fields(&args_of(m));
    if toks.is_empty() {
        let cmd = command_of(m);
        let _ = bot.reply_text(m, &f_args(loc(), chat_id, "auth_no_user", &[("cmd", &cmd)]), None);
        return;
    }
    let target = match resolve_target(&toks) {
        Ok(t) => t,
        Err(err) => {
            let _ = bot.reply_text(
                m,
                &f_args(loc(), chat_id, "user_extract_fail", &[("error", &escape_html(&err))]),
                None,
            );
            return;
        }
    };
    let c = cfg();
    if target == c.owner_id {
        let _ = bot.reply_text(m, &f(loc(), chat_id, "sudo_owner_self"), None);
        return;
    }
    if target == bot.me_id() {
        let _ = bot.reply_text(m, &f(loc(), chat_id, "sudo_bot_self"), None);
        return;
    }
    let user = code_id(target);
    let id_str = target.to_string();
    if db::is_sudo(target) {
        let _ = bot.reply_text(
            m,
            &f_args(loc(), chat_id, "sudo_already", &[("user", &user), ("id", &id_str)]),
            None,
        );
        return;
    }
    if !db::add_sudo(target) {
        let _ = bot.reply_text(
            m,
            &f_args(loc(), chat_id, "sudo_add_fail", &[("error", "storage error")]),
            None,
        );
        return;
    }
    let _ = bot.reply_text(
        m,
        &f_args(loc(), chat_id, "sudo_add_success", &[("user", &user), ("id", &id_str)]),
        None,
    );
}

fn del_sudo_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_owner(bot, m) {
        return;
    }
    let chat_id = m.chat_id;
    let toks = fields(&args_of(m));
    if toks.is_empty() {
        let cmd = command_of(m);
        let _ = bot.reply_text(m, &f_args(loc(), chat_id, "auth_no_user", &[("cmd", &cmd)]), None);
        return;
    }
    let target = match resolve_target(&toks) {
        Ok(t) => t,
        Err(err) => {
            let _ = bot.reply_text(
                m,
                &f_args(loc(), chat_id, "user_extract_fail", &[("error", &escape_html(&err))]),
                None,
            );
            return;
        }
    };
    let c = cfg();
    if target == c.owner_id {
        let _ = bot.reply_text(m, &f(loc(), chat_id, "sudo_owner_remove_block"), None);
        return;
    }
    let user = code_id(target);
    let id_str = target.to_string();
    if !db::is_sudo(target) {
        let _ = bot.reply_text(
            m,
            &f_args(loc(), chat_id, "sudo_not_exists", &[("user", &user), ("id", &id_str)]),
            None,
        );
        return;
    }
    if !db::del_sudo(target) {
        let _ = bot.reply_text(
            m,
            &f_args(loc(), chat_id, "sudo_remove_fail", &[("error", "storage error")]),
            None,
        );
        return;
    }
    let _ = bot.reply_text(
        m,
        &f_args(loc(), chat_id, "sudo_remove_success", &[("user", &user), ("id", &id_str)]),
        None,
    );
}

fn get_sudoers_handler(bot: &mut dyn Bot, m: &TgMessage) {
    let chat_id = m.chat_id;
    let mut sb = f(loc(), chat_id, "sudo_list_header");
    sb += "\n\n";
    let owner_id = cfg().owner_id;
    sb += &f_args(
        loc(),
        chat_id,
        "sudo_list_owner",
        &[("index", "1"), ("user", &code_id(owner_id))],
    );
    sb += "\n";
    let mut idx = 2;
    for id in db::sudo_list() {
        if id == owner_id {
            continue;
        }
        sb += &f_args(
            loc(),
            chat_id,
            "sudo_list_entry",
            &[("index", &idx.to_string()), ("user", &code_id(id))],
        );
        sb += "\n";
        idx += 1;
    }
    if idx == 2 {
        sb += &f(loc(), chat_id, "sudo_list_no_extra");
        sb += "\n";
    }
    let _ = bot.reply_text(m, &sb, None);
}

// ── auth family ─────────────────────────────────────────────

fn add_auth_handler(bot: &mut dyn Bot, m: &TgMessage) {
    let chat_id = m.chat_id;
    let toks = fields(&args_of(m));
    if toks.is_empty() {
        let cmd = command_of(m);
        let _ = bot.reply_text(m, &f_args(loc(), chat_id, "auth_no_user", &[("cmd", &cmd)]), None);
        return;
    }
    let c = cfg();
    let limit = if c.max_auth_users > 0 { c.max_auth_users } else { 10 };
    if db::auth_users(chat_id).len() as i64 >= limit {
        let _ = bot.reply_text(
            m,
            &f_args(loc(), chat_id, "auth_limit_reached", &[("limit", &limit.to_string())]),
            None,
        );
        return;
    }
    let target = match resolve_target(&toks) {
        Ok(t) => t,
        Err(err) => {
            let _ = bot.reply_text(
                m,
                &f_args(loc(), chat_id, "user_extract_fail", &[("error", &escape_html(&err))]),
                None,
            );
            return;
        }
    };
    if target == c.owner_id || target == bot.me_id() || target == m.sender_id {
        let _ = bot.reply_text(m, &f(loc(), chat_id, "cannot_authorize_user"), None);
        return;
    }
    if db::is_authorized(chat_id, target) {
        let _ = bot.reply_text(m, &f(loc(), chat_id, "already_authed"), None);
        return;
    }
    if !db::add_auth_user(chat_id, target) {
        let _ = bot.reply_text(
            m,
            &f_args(loc(), chat_id, "addauth_add_fail", &[("error", "storage error")]),
            None,
        );
        return;
    }
    let au = db::auth_users(chat_id);
    let user = code_id(target);
    if !au.is_empty() {
        let _ = bot.reply_text(
            m,
            &f_args(
                loc(),
                chat_id,
                "addauth_success_with_count",
                &[
                    ("user", &user),
                    ("count", &au.len().to_string()),
                    ("limit", &limit.to_string()),
                ],
            ),
            None,
        );
    } else {
        let _ = bot.reply_text(m, &f_args(loc(), chat_id, "addauth_success", &[("user", &user)]), None);
    }
}

fn del_auth_handler(bot: &mut dyn Bot, m: &TgMessage) {
    let chat_id = m.chat_id;
    let toks = fields(&args_of(m));
    if toks.is_empty() {
        let cmd = command_of(m);
        let _ = bot.reply_text(m, &f_args(loc(), chat_id, "auth_no_user", &[("cmd", &cmd)]), None);
        return;
    }
    let target = match resolve_target(&toks) {
        Ok(t) => t,
        Err(err) => {
            let _ = bot.reply_text(
                m,
                &f_args(loc(), chat_id, "user_extract_fail", &[("error", &escape_html(&err))]),
                None,
            );
            return;
        }
    };
    if !db::is_authorized(chat_id, target) {
        let _ = bot.reply_text(m, &f(loc(), chat_id, "del_auth_not_authorized"), None);
        return;
    }
    if !db::del_auth_user(chat_id, target) {
        let _ = bot.reply_text(
            m,
            &f_args(loc(), chat_id, "del_auth_remove_fail", &[("error", "storage error")]),
            None,
        );
        return;
    }
    let _ = bot.reply_text(
        m,
        &f_args(
            loc(),
            chat_id,
            "del_auth_success",
            &[("user", &format!("User ({})", code_id(target)))],
        ),
        None,
    );
}

fn auth_list_handler(bot: &mut dyn Bot, m: &TgMessage) {
    let chat_id = m.chat_id;
    let au = db::auth_users(chat_id);
    if au.is_empty() {
        let _ = bot.reply_text(m, &f(loc(), chat_id, "authlist_empty"), None);
        return;
    }
    let mut sb = f(loc(), chat_id, "authlist_header");
    sb += "\n";
    for (i, id) in au.iter().enumerate() {
        sb += &f_args(
            loc(),
            chat_id,
            "authlist_user_fail",
            &[("index", &(i + 1).to_string()), ("user_id", &id.to_string())],
        );
        sb += "\n";
    }
    sb += &format!(
        "\n{}",
        f_args(loc(), chat_id, "authlist_total", &[("count", &au.len().to_string())])
    );
    let _ = bot.reply_text(m, &sb, None);
}

// ── broadcast ──────────────────────────────────────────────

/// v1 sends inline (deterministic tests, no detached thread):
/// single-user sync bot) and the cancel keyboard is omitted from the reply.
static BCAST_RUNNING: AtomicBool = AtomicBool::new(false);
static BCAST_CANCEL: AtomicBool = AtomicBool::new(false);

fn fmt1(v: f64) -> String {
    format!("{v:.1}")
}

// bcast_progress args (done/total per audience, failed list,
// delay, elapsed, cancelled) — keep the flat signature.
#[allow(clippy::too_many_arguments)]
fn bcast_progress(
    chat_id: i64,
    done_chats: i64,
    total_chats: i64,
    done_users: i64,
    total_users: i64,
    failed_chats: &[i64],
    delay_sec: f64,
    elapsed_sec: i64,
    cancelled: bool,
) -> String {
    let line = |key: &str, done: i64, total: i64| {
        let pct = if total > 0 { done as f64 / total as f64 * 100.0 } else { 0.0 };
        f_args(
            loc(),
            chat_id,
            key,
            &[
                ("done", &done.to_string()),
                ("total", &total.to_string()),
                ("progress", &fmt1(pct)),
            ],
        )
    };
    let mut out = String::new();
    if cancelled {
        out += &f(loc(), chat_id, "broadcast_cancel_done");
        out += "\n\n";
    }
    out += &line("broadcast_total_chats", done_chats, total_chats);
    out += "\n";
    out += &line("broadcast_total_users", done_users, total_users);
    out += "\n\n";
    if !failed_chats.is_empty() {
        out += &f_args(
            loc(),
            chat_id,
            "broadcast_failed_chats",
            &[("count", &failed_chats.len().to_string())],
        );
        out += "\n\n";
    }
    out += &f_args(loc(), chat_id, "broadcast_delay", &[("delay", &fmt1(delay_sec))]);
    out += "\n";
    out += &f_args(
        loc(),
        chat_id,
        "broadcast_elapsed",
        &[("elapsed", &crate::utils::format_duration(elapsed_sec))],
    );
    let total_targets = total_chats + total_users;
    let sent = done_chats + done_users;
    let failed = failed_chats.len() as i64;
    let rate = if total_targets > 0 {
        (sent - failed) as f64 / total_targets as f64 * 100.0
    } else {
        0.0
    };
    out += &format!(
        "\n\n{}",
        f_args(
            loc(),
            chat_id,
            "broadcast_success_rate",
            &[
                ("rate", &fmt1(rate)),
                ("sent", &(sent - failed).to_string()),
                ("total", &total_targets.to_string()),
            ],
        )
    );
    out
}

struct BcastOpts {
    no_chat: bool,
    no_user: bool,
    limit: i64,
    #[allow(dead_code)]
    delay: f64,
    content: String,
}

/// parse_bcast_command parity: flags on line 0 (-nochat/-nouser/-limit N/
/// -delay N; -copy/-pin accepted but inert), content is the first line's
/// remaining words plus all following lines.
fn parse_bcast_command(m: &TgMessage, err: &mut String) -> Option<BcastOpts> {
    let text = trim(&args_of(m)).to_string();
    let nl = text.find('\n').unwrap_or(text.len());
    let (first_line, rest) = text.split_at(nl);
    let rest: Vec<&str> = if rest.is_empty() { Vec::new() } else { vec![&rest[1..]] };
    let mut no_chat = false;
    let mut no_user = false;
    let mut limit: i64 = 0;
    let mut delay: f64 = 0.7;
    let mut content_words: Vec<String> = Vec::new();
    let words: Vec<String> = fields(first_line);
    let mut skip_next = false;
    for (i, w) in words.iter().enumerate() {
        if skip_next {
            skip_next = false;
            continue;
        }
        let lw = to_lower(w);
        let nv = words.get(i + 1).cloned().unwrap_or_default();
        if lw == "-nochat" || lw == "--nochat" {
            no_chat = true;
        } else if lw == "-nouser" || lw == "--nouser" {
            no_user = true;
        } else if lw == "-copy"
            || lw == "--copy"
            || lw == "-pin"
            || lw == "--pin"
            || lw == "-pinloud"
            || lw == "--pinloud"
        {
            // forwarding/pinning is not available in this build (see NOTES)
        } else if lw == "-limit" || lw == "--limit" {
            match nv.parse::<i64>() {
                Ok(v) if v >= 0 => limit = v,
                _ => {
                    *err = format!("invalid limit value: {nv}");
                    return None;
                }
            }
            skip_next = true;
        } else if lw == "-delay" || lw == "--delay" {
            match nv.parse::<f64>() {
                Ok(v) if v >= 0.0 => delay = v,
                _ => {
                    *err = format!("invalid delay value: {nv}");
                    return None;
                }
            }
            skip_next = true;
        } else {
            content_words.push(w.clone());
        }
    }
    let mut lines = vec![content_words.join(" ")];
    lines.extend(rest.iter().map(|s| s.to_string()));
    Some(BcastOpts {
        no_chat,
        no_user,
        limit,
        delay,
        content: trim(&lines.join("\n")).to_string(),
    })
}

fn broadcast_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_owner(bot, m) {
        return;
    }
    let chat_id = m.chat_id;
    if to_lower(&m.text).contains("-cancel") {
        if !BCAST_RUNNING.load(Ordering::SeqCst) {
            let _ = bot.reply_text(m, &f(loc(), chat_id, "broadcast_not_running"), None);
            return;
        }
        BCAST_CANCEL.store(true, Ordering::SeqCst);
        let _ = bot.reply_text(m, &f(loc(), chat_id, "broadcast_cancel_success"), None);
        return;
    }
    if BCAST_RUNNING.load(Ordering::SeqCst) {
        let _ = bot.reply_text(m, &f(loc(), chat_id, "broadcast_already_running"), None);
        return;
    }
    let mut err = String::new();
    let Some(opts) = parse_bcast_command(m, &mut err) else {
        let _ = bot.reply_text(
            m,
            &f_args(loc(), chat_id, "broadcast_parse_failed", &[("error", &escape_html(&err))]),
            None,
        );
        return;
    };
    if opts.content.is_empty() {
        let cmd = command_of(m);
        let _ = bot.reply_text(m, &f_args(loc(), chat_id, "broadcast_no_content", &[("cmd", &cmd)]), None);
        return;
    }
    let mut chats = if opts.no_chat { Vec::new() } else { db::served_chats() };
    let mut users = if opts.no_user { Vec::new() } else { db::served_users() };
    if opts.limit > 0 && (chats.len() + users.len()) as i64 > opts.limit {
        if chats.len() as i64 >= opts.limit {
            chats.truncate(opts.limit as usize);
            users.clear();
        } else {
            users.truncate((opts.limit - chats.len() as i64) as usize);
        }
    }
    if chats.is_empty() && users.is_empty() {
        let _ = bot.reply_text(m, &f(loc(), chat_id, "broadcast_no_targets"), None);
        return;
    }
    if BCAST_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        let _ = bot.reply_text(m, &f(loc(), chat_id, "broadcast_already_running"), None);
        return;
    }
    BCAST_CANCEL.store(false, Ordering::SeqCst);
    let _ = bot.reply_text(m, &f(loc(), chat_id, "broadcast_initializing"), None);
    let start = std::time::Instant::now();
    let mut done_chats = 0i64;
    let mut done_users = 0i64;
    let mut failed: Vec<i64> = Vec::new();
    let total_chats = chats.len() as i64;
    let total_users = users.len() as i64;
    for id in &chats {
        if BCAST_CANCEL.load(Ordering::SeqCst) {
            break;
        }
        if bot.send_text(*id, &opts.content, None).unwrap_or(false) {
            done_chats += 1;
        } else {
            failed.push(*id);
        }
    }
    if !BCAST_CANCEL.load(Ordering::SeqCst) {
        for id in &users {
            if BCAST_CANCEL.load(Ordering::SeqCst) {
                break;
            }
            if bot.send_text(*id, &opts.content, None).unwrap_or(false) {
                done_users += 1;
            } else {
                failed.push(*id);
            }
        }
    }
    let elapsed = start.elapsed().as_secs();
    let text = bcast_progress(
        chat_id,
        done_chats,
        total_chats,
        done_users,
        total_users,
        &failed,
        0.0,
        elapsed as i64,
        BCAST_CANCEL.load(Ordering::SeqCst),
    );
    let _ = bot.send_text(chat_id, &text, None);
    BCAST_RUNNING.store(false, Ordering::SeqCst);
}

fn bcast_cancel_cb(bot: &mut dyn Bot, cb: &TgCallback) {
    let c = cfg();
    if c.owner_id == 0 || cb.user_id != c.owner_id {
        let _ = bot.answer_callback(cb.query_id, "", false);
        return;
    }
    if !BCAST_RUNNING.load(Ordering::SeqCst) {
        let _ = bot.answer_callback(cb.query_id, "", false);
        return;
    }
    BCAST_CANCEL.store(true, Ordering::SeqCst);
    let _ = bot.answer_callback(cb.query_id, "", false);
    let _ = bot.send_text(cb.chat_id, &f(loc(), cb.chat_id, "broadcast_cancel_done"), None);
}

// ── registration ──────────────────────────────────────────────────────────────

pub fn register(reg: &mut Registry) {
    // group admin ops
    reg.command("promote", promote_handler);
    reg.command("fullpromote", full_promote_handler);
    reg.command("demote", demote_handler);
    reg.command("unpromote", demote_handler);
    reg.command("ban", ban_handler);
    reg.command("unban", unban_handler);
    reg.command("kick", kick_handler);
    reg.command("warn", warn_handler);
    reg.command("unwarn", unwarn_handler);
    reg.command("warns", warns_handler);
    reg.command("warnings", warns_handler);
    reg.command("clearwarn", clear_warn_handler);
    reg.command("resetwarns", reset_warns_handler);
    reg.command("warnlimit", warn_limit_handler);
    reg.command("setwarnlimit", warn_limit_handler);
    reg.command("setwarnmode", set_warn_mode_handler);
    reg.command("pin", pin_handler);
    reg.command("unpin", unpin_handler);
    reg.command("unpinall", unpin_all_handler);
    reg.command("purge", purge_handler);

    // sudo + auth registries
    reg.command("addsudo", add_sudo_handler);
    reg.command("addsudoer", add_sudo_handler);
    reg.command("sudoadd", add_sudo_handler);
    reg.command("delsudo", del_sudo_handler);
    reg.command("delsudoer", del_sudo_handler);
    reg.command("sudodel", del_sudo_handler);
    reg.command("remsudo", del_sudo_handler);
    reg.command("rmsudo", del_sudo_handler);
    reg.command("sudorem", del_sudo_handler);
    reg.command("dropsudo", del_sudo_handler);
    reg.command("unsudo", del_sudo_handler);
    reg.command("sudoers", get_sudoers_handler);
    reg.command("listsudo", get_sudoers_handler);
    reg.command("sudolist", get_sudoers_handler);
    reg.command("auth", add_auth_handler);
    reg.command("addauth", add_auth_handler);
    reg.command("authorize", add_auth_handler);
    reg.command("delauth", del_auth_handler);
    reg.command("unauth", del_auth_handler);
    reg.command("delauthuser", del_auth_handler);
    reg.command("authlist", auth_list_handler);
    reg.command("authusers", auth_list_handler);

    // broadcast
    reg.command("broadcast", broadcast_handler);
    reg.command("gcast", broadcast_handler);
    reg.command("bcast", broadcast_handler);
    reg.callback("bcast_cancel", bcast_cancel_cb);
}
