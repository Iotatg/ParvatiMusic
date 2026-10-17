//! /autoleave toggle and the voicechat lifecycle handlers.
//!
//! Deviation (v1): the started/ended service-message cards cannot trigger —
//! the Rust message facade carries no raw payload (messageService events are
//! not parsed in layer-227), so only the flag command is ported.

use std::sync::atomic::{AtomicBool, Ordering};

use parvati_mtproto::messaging::TgMessage;

use crate::db;
use crate::mod_tg::Bot;
use crate::modules::common::f_args;
use crate::modules::{loc, Registry};

/// Process-local autoleave flag.
static AUTO_LEAVE: AtomicBool = AtomicBool::new(false);

pub fn autoleave_enabled() -> bool {
    AUTO_LEAVE.load(Ordering::SeqCst)
}

/// First whitespace field of the command, with any "@bot" suffix stripped.
fn get_command(m: &TgMessage) -> String {
    match m.text.find(' ') {
        Some(sp) => m.text[..sp].to_string(),
        None => m.text.clone(),
    }
    .split('@')
    .next()
    .unwrap_or_default()
    .to_string()
}

fn owner_or_sudo(user_id: i64) -> bool {
    let owner = crate::modules::cfg().owner_id;
    (owner != 0 && user_id == owner) || db::is_sudo(user_id)
}

/// strict_bool: the accepted on/off vocabulary, None when unrecognized.
fn strict_bool(raw: &str) -> Option<bool> {
    let v = crate::utils::to_lower(crate::utils::trim(raw));
    match v.as_str() {
        "on" | "enable" | "enabled" | "true" | "1" | "yes" | "y" => Some(true),
        "off" | "disable" | "disabled" | "false" | "0" | "no" | "n" => Some(false),
        _ => None,
    }
}

fn enabled_word(chat_id: i64, v: bool) -> String {
    f_args(loc(), chat_id, if v { "enabled" } else { "disabled" }, &[])
}

/// /autoleave — owner/sudo-only flag; bare command reports the state.
pub fn handle_autoleave(bot: &mut dyn Bot, m: &TgMessage) {
    let loc = loc();
    let chat_id = m.chat_id;
    if !owner_or_sudo(m.sender_id) {
        let _ = bot.reply_text(m, &f_args(loc, chat_id, "only_sudo", &[]), None);
        return;
    }
    let fields: Vec<&str> = m.text.split_whitespace().collect();
    let current = autoleave_enabled();
    if fields.len() < 2 {
        let text = f_args(
            loc,
            chat_id,
            "autoleave_status",
            &[
                ("cmd", &get_command(m)),
                ("action", &enabled_word(chat_id, current)),
            ],
        );
        let _ = bot.reply_text(m, &text, None);
        return;
    }
    let Some(enabled) = strict_bool(fields[1]) else {
        let _ = bot.reply_text(m, &f_args(loc, chat_id, "invalid_bool", &[]), None);
        return;
    };
    let action = enabled_word(chat_id, enabled);
    if enabled == current {
        let _ = bot.reply_text(
            m,
            &f_args(loc, chat_id, "autoleave_already", &[("action", &action)]),
            None,
        );
        return;
    }
    AUTO_LEAVE.store(enabled, Ordering::SeqCst);
    let _ = bot.reply_text(
        m,
        &f_args(loc, chat_id, "autoleave_updated", &[("action", &action)]),
        None,
    );
}

pub fn register(r: &mut Registry) {
    r.command("autoleave", handle_autoleave);
}
