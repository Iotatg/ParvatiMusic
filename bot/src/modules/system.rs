//! System and console command surface: the eval/shell dev tools, the json dump,
//! the log/logger pair, the vc logger status commands, maintenance mode, the
//! locale reload, restart, privacy, bug reports and the repo card.
//!
//! Port of the reference admin_system module. V1 deviations (same as the
//! reference C++ build): eval reports it is unavailable, replied-message
//! forwarding (json -f, bug media) is not wired, and restart does not actually
//! respawn the process.

use std::sync::atomic::{AtomicBool, Ordering};

use parvati_mtproto::messaging::TgMessage;

use crate::buttons::{data_btn, rows, url_btn};
use crate::db;
use crate::mod_tg::Bot;
use crate::modules::common::{f, f_args};
use crate::modules::{cfg, loc, Registry};
use crate::utils::{escape_html, stylish, to_lower, trim};

/// Process-local vclogger state (enabled + running), keyed by chat id.
static VC_LOGGER: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<i64, (bool, bool)>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));
/// Maintenance reason (empty when unset).
static MAINT_REASON: std::sync::LazyLock<std::sync::Mutex<String>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(String::new()));
static MAINT_CANCEL: AtomicBool = AtomicBool::new(false);

// ── shared helpers ─────────────────────────────────────────────────────────────

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

fn get_command(m: &TgMessage) -> String {
    let t = trim(&m.text);
    match t.find(' ') {
        Some(sp) => t[..sp].to_string(),
        None => t.to_string(),
    }
}

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
        crate::parv_warn!(
            "owner",
            "require_owner denied: sender_id={} owner_id={} chat_id={}",
            m.sender_id,
            c.owner_id,
            m.chat_id
        );
        let _ = bot.reply_text(m, &f(loc(), m.chat_id, "only_owner"), None);
        return false;
    }
    true
}

fn require_owner_or_sudo(bot: &mut dyn Bot, m: &TgMessage) -> bool {
    let c = cfg();
    if (c.owner_id != 0 && m.sender_id == c.owner_id) || db::is_sudo(m.sender_id) {
        return true;
    }
    let _ = bot.reply_text(m, &f(loc(), m.chat_id, "only_sudo"), None);
    false
}

/// privileged_gate: owner/sudo/authorized pass (no live chat-admin lookup).
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

// ── dev / eval ─────────────────────────────────────────────────────────────────

fn eval_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_owner(bot, m) {
        return;
    }
    let _ = bot.reply_text(
        m,
        &stylish("Eval is not available in this build (Go snippet evaluation was not ported)."),
        None,
    );
}

fn shell_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_owner_or_sudo(bot, m) {
        return;
    }
    let command = args_of(m);
    if command.is_empty() {
        let _ = bot.reply_text(m, &stylish("No command provided"), None);
        return;
    }
    let merged = format!("{command} 2>&1");
    let output = std::process::Command::new("sh").arg("-c").arg(&merged).output();
    let (text, code) = match output {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).to_string();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            let code = o.status.code().unwrap_or(-1);
            (s, code)
        }
        Err(_) => {
            let _ = bot.reply_text(m, &stylish("<code>Error:</code> <b>failed to spawn command</b>"), None);
            return;
        }
    };
    let text = trim(&text).to_string();
    const CAP: usize = 4000;
    if text.is_empty() {
        if code != 0 {
            let _ = bot.reply_text(
                m,
                &stylish(&format!("<code>Error:</code> <b>command exited with code {code}</b>")),
                None,
            );
        } else {
            let _ = bot.reply_text(m, &stylish("<code>No Output</code>"), None);
        }
        return;
    }
    let truncated = text.len() > CAP;
    let body: String = text.chars().take(CAP).collect();
    let mut out = format!("<pre lang=\"bash\">{}</pre>", escape_html(&body));
    if truncated {
        out = format!("<pre lang=\"bash\">{}\n... (output truncated)</pre>", escape_html(&body));
    }
    let _ = bot.reply_text(m, &out, None);
}

fn json_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if m.reply_to != 0 {
        let _ = bot.reply_text(
            m,
            &stylish("<code>Error:</code> <b>could not fetch replied message</b>"),
            None,
        );
        return;
    }
    let dump = format!(
        "{{\n  \"chat_id\": {},\n  \"message_id\": {},\n  \"sender_id\": {},\n  \"text\": \"{}\",\n  \"first_name\": \"{}\",\n  \"is_private\": {},\n  \"is_channel\": {}\n}}",
        m.chat_id,
        m.id,
        m.sender_id,
        escape_html(&m.text).replace('"', "\\\""),
        escape_html(&m.first_name),
        m.is_private,
        m.is_channel
    );
    let body: String = dump.chars().take(3800).collect();
    let _ = bot.reply_text(m, &format!("<pre lang='json'>{}</pre>", escape_html(&body)), None);
}

fn logs_handler(bot: &mut dyn Bot, m: &TgMessage) {
    let exists = std::fs::metadata("logs.txt").map(|md| md.len() > 0).unwrap_or(false);
    if !exists {
        let _ = bot.reply_text(m, &f(loc(), m.chat_id, "logs_empty"), None);
        return;
    }
    let _ = bot.reply_text(
        m,
        &stylish("<code>Error:</code> <b>document sending is not available in this build</b>"),
        None,
    );
}

// ── logger ─────────────────────────────────────────────────────────────────────

fn logger_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_owner_or_sudo(bot, m) {
        return;
    }
    let chat_id = m.chat_id;
    let toks = fields(&args_of(m));
    let current = db::logger();
    let action = enabled_word(chat_id, current);
    if toks.is_empty() {
        let status = f_args(loc(), chat_id, "logger_status", &[("action", &action)]);
        let text = f_args(
            loc(),
            chat_id,
            "logger_usage",
            &[("cmd", &get_command(m)), ("status", &status)],
        );
        let _ = bot.reply_text(m, &text, None);
        return;
    }
    let Some(enable) = strict_bool(&toks[0]) else {
        let _ = bot.reply_text(m, &f(loc(), chat_id, "invalid_bool"), None);
        return;
    };
    let action = enabled_word(chat_id, enable);
    if current == enable {
        let text = f_args(loc(), chat_id, "logger_already", &[("action", &action)]);
        let _ = bot.reply_text(m, &text, None);
        return;
    }
    db::set_logger(enable);
    let text = f_args(loc(), chat_id, "logger_updated", &[("action", &action)]);
    let _ = bot.reply_text(m, &text, None);
}

// ── maintenance ────────────────────────────────────────────────────────────────

fn maint_get_reason() -> String {
    MAINT_REASON.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn maint_set_reason(reason: &str) {
    *MAINT_REASON.lock().unwrap_or_else(|e| e.into_inner()) = reason.to_string();
}

fn maintenance_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_owner(bot, m) {
        return;
    }
    let chat_id = m.chat_id;
    let toks = fields(&args_of(m));
    let current = db::maintenance();

    if toks.is_empty() {
        let reason = maint_get_reason();
        let status = if current {
            if reason.is_empty() {
                f(loc(), chat_id, "enabled")
            } else {
                f_args(loc(), chat_id, "enabled_with_reason", &[("reason", &escape_html(&reason))])
            }
        } else {
            f(loc(), chat_id, "disabled")
        };
        let text = f_args(
            loc(),
            chat_id,
            "maint_usage",
            &[("cmd", &get_command(m)), ("status", &status)],
        );
        let _ = bot.reply_text(m, &text, None);
        return;
    }

    let Some(enable) = strict_bool(&toks[0]) else {
        let _ = bot.reply_text(m, &f(loc(), chat_id, "invalid_bool"), None);
        return;
    };
    let reason = toks[1..].join(" ");

    if current == enable {
        let old = maint_get_reason();
        if !enable {
            let _ = bot.reply_text(m, &f(loc(), chat_id, "maint_already_disabled"), None);
        } else if reason == old {
            let _ = bot.reply_text(m, &f(loc(), chat_id, "maint_already_reason_same"), None);
        } else if reason.is_empty() && !old.is_empty() {
            maint_set_reason("");
            let _ = bot.reply_text(m, &f(loc(), chat_id, "maint_reason_removed"), None);
        } else if !reason.is_empty() {
            maint_set_reason(&reason);
            let text = f_args(loc(), chat_id, "maint_reason_updated", &[("reason", &escape_html(&reason))]);
            let _ = bot.reply_text(m, &text, None);
        } else {
            let _ = bot.reply_text(m, &f(loc(), chat_id, "maint_already_enabled"), None);
        }
        return;
    }

    db::set_maintenance(enable);
    maint_set_reason(if enable { &reason } else { "" });
    MAINT_CANCEL.store(!enable, Ordering::SeqCst);
    if enable {
        if let Some(rooms) = crate::modules::rooms() {
            for (target, _) in rooms.all_rooms() {
                if MAINT_CANCEL.load(Ordering::SeqCst) {
                    break;
                }
                rooms.drop_room(target);
                let mut msg = f(loc(), target, "maint_entering");
                if !reason.is_empty() {
                    msg.push('\n');
                    msg.push_str(&f_args(
                        loc(),
                        target,
                        "maint_reason",
                        &[("reason", &escape_html(&reason))],
                    ));
                }
                let _ = bot.send_text(target, &msg, None);
            }
        }
        if reason.is_empty() {
            let _ = bot.reply_text(m, &f(loc(), chat_id, "maint_enabled"), None);
        } else {
            let text = f_args(loc(), chat_id, "maint_enabled_reason", &[("reason", &escape_html(&reason))]);
            let _ = bot.reply_text(m, &text, None);
        }
    } else {
        let _ = bot.reply_text(m, &f(loc(), chat_id, "maint_disabled"), None);
    }
}

// ── reload ─────────────────────────────────────────────────────────────────────

fn reload_impl(bot: &mut dyn Bot, m: &TgMessage, cplay: bool) {
    if !group_gate(bot, m) {
        return;
    }
    if cplay && !privileged_gate(m.chat_id, m.sender_id) {
        let _ = bot.reply_text(m, &f(loc(), m.chat_id, "only_admin_or_auth"), None);
        return;
    }
    let chat_id = m.chat_id;
    let start = f(loc(), chat_id, "reload_start");
    let id = bot.send_text_sync(chat_id, &start, None, m.id).unwrap_or(0);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/locales");
    let summary = match crate::locales::Locales::load(dir) {
        Ok(_) => "locales reloaded".to_string(),
        Err(e) => e,
    };
    let done = f_args(loc(), chat_id, "reload_done", &[("summary", &escape_html(&summary))]);
    if id != 0 && bot.edit_text(chat_id, id, &done, None).unwrap_or(false) {
        return;
    }
    let _ = bot.send_text(chat_id, &done, None);
}

fn reload_handler(bot: &mut dyn Bot, m: &TgMessage) {
    reload_impl(bot, m, false);
}

fn creload_handler(bot: &mut dyn Bot, m: &TgMessage) {
    reload_impl(bot, m, true);
}

// ── restart ────────────────────────────────────────────────────────────────────

fn restart_confirm_markup(chat_id: i64) -> parvati_mtproto::messaging::Markup {
    rows(vec![vec![
        data_btn(&f(loc(), chat_id, "restart_btn_bot"), "restart:bot"),
        data_btn(&f(loc(), chat_id, "restart_btn_replay"), "restart:replay"),
    ]])
}

fn active_room(chat_id: i64) -> bool {
    crate::modules::rooms()
        .and_then(|rs| rs.room_for(chat_id))
        .map(|r| r.lock().unwrap_or_else(|e| e.into_inner()).active())
        .unwrap_or(false)
}

fn restart_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !require_owner(bot, m) {
        return;
    }
    let chat_id = m.chat_id;
    if active_room(chat_id) {
        let text = f(loc(), chat_id, "restart_confirm_running");
        let _ = bot.reply_text(m, &text, Some(&restart_confirm_markup(chat_id)));
        return;
    }
    let _ = bot.reply_text(m, &f(loc(), chat_id, "restart"), None);
}

fn restart_cb(bot: &mut dyn Bot, cb: &parvati_mtproto::messaging::TgCallback) {
    let c = cfg();
    if c.owner_id == 0 || cb.user_id != c.owner_id {
        let _ = bot.answer_callback(cb.query_id, "", false);
        return;
    }
    let chat_id = cb.chat_id;
    if !cb.data.starts_with("restart:") {
        let _ = bot.answer_callback(cb.query_id, "", false);
        return;
    }
    let action = &cb.data["restart:".len()..];
    if action == "bot" {
        let _ = bot.answer_callback(cb.query_id, "", false);
        let _ = bot.edit_text(chat_id, cb.message_id, &f(loc(), chat_id, "restart"), None);
        return;
    }
    if action == "replay" {
        let replay_ok = crate::modules::rooms()
            .and_then(|rs| rs.room_for(chat_id))
            .map(|r| r.lock().unwrap_or_else(|e| e.into_inner()).replay())
            .unwrap_or(false);
        if !replay_ok {
            let _ = bot.answer_callback(cb.query_id, "", false);
            return;
        }
        let _ = bot.edit_text(
            chat_id,
            cb.message_id,
            &f(loc(), chat_id, "restart_confirm_replay_done"),
            None,
        );
        let _ = bot.answer_callback(cb.query_id, "", false);
    }
}

// ── privacy ────────────────────────────────────────────────────────────────────

fn privacy_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if m.is_channel {
        return;
    }
    let support = escape_html(&cfg().support_chat);
    let text = format!(
        "<b>\u{1f6e1} Privacy Policy &amp; Data Handling</b>\n\n\
Your privacy is important to us. This bot is designed with privacy in mind.\n\n\
<b>\u{1f4ca} Data We Collect</b>\n\
<blockquote>We only store essential data required for the bot to function:\n\
\u{2022} <b>User & Chat IDs:</b> To identify groups and manage settings.\n\
\u{2022} <b>Preferences:</b> Language settings and bot configurations.\n\
\u{2022} <b>Access Control:</b> Authorized user lists for your group.</blockquote>\n\n\
<b>\u{1f4e9} Message Privacy</b>\n\
<blockquote>\u{2022} The bot <b>only</b> reads messages that start with a command (e.g., <code>/play</code>) or interactions with its own buttons.\n\
\u{2022} It <b>does not</b> read, store, or monitor your private conversations or general group messages.</blockquote>\n\n\
<b>\u{1f310} Third-Party Services</b>\n\
<blockquote>\u{2022} We use external services like <b>YouTube</b> and <b>Spotify</b> solely to search for and stream the music you request.\n\
\u{2022} No personal data is shared with these services beyond the search query itself.</blockquote>\n\n\
<b>\u{1f91d} Data Sharing</b>\n\
<blockquote>\u{2022} We <b>never</b> sell, share, or trade your data with third parties.\n\
\u{2022} All data is used strictly to provide and improve the bot's music streaming features.</blockquote>\n\n\
<b>\u{2728} Our Commitment</b>\n\
This bot is an <a href=\"https://github.com/Iotatg/ParvatiMusic\">open-source project</a> dedicated to providing a high-quality streaming experience while respecting user privacy.\n\n\
<i>If you have any questions, feel free to join our <a href=\"{support}\">Support Chat</a>.</i>"
    );
    let _ = bot.reply_text(m, &text, None);
}

// ── bug / repo ─────────────────────────────────────────────────────────────────

fn bug_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if m.is_channel {
        return;
    }
    let chat_id = m.chat_id;
    let reason = args_of(m);
    if reason.is_empty() && m.reply_to == 0 {
        let text = f_args(loc(), chat_id, "bug_usage", &[("cmd", &get_command(m))]);
        let _ = bot.reply_text(m, &text, None);
        return;
    }
    let chat_name = f(loc(), chat_id, "bug_private_chat");
    let mut report = escape_html(&reason);
    if report.is_empty() {
        report = f(loc(), chat_id, "bug_reply_only");
    }
    let msg = f_args(
        loc(),
        chat_id,
        "bug_report_format",
        &[
            ("user", &crate::modules::common::mention_of(m)),
            ("user_id", &m.sender_id.to_string()),
            ("chat", &chat_name),
            ("chat_id", &chat_id.to_string()),
            ("report", &report),
        ],
    );
    let c = cfg();
    let mut targets: Vec<i64> = Vec::new();
    if c.logger_id != 0 {
        targets.push(c.logger_id);
    }
    if c.owner_id != 0 && c.owner_id != c.logger_id {
        targets.push(c.owner_id);
    }
    for target in targets {
        let _ = bot.send_text(target, &msg, None);
    }
    let _ = bot.reply_text(m, &f(loc(), chat_id, "bug_thanks"), None);
}

fn repo_markup(chat_id: i64) -> parvati_mtproto::messaging::Markup {
    let c = cfg();
    let dev = if !c.dev_url.is_empty() {
        c.dev_url.clone()
    } else if c.owner_id != 0 {
        format!("tg://openmessage?user_id={}", c.owner_id)
    } else {
        "https://t.me/+Q493A_VBAUpkMmRl".to_string()
    };
    rows(vec![vec![
        url_btn(&f(loc(), chat_id, "REPO_DEV_BTN"), &dev),
        url_btn(&f(loc(), chat_id, "REPO_BTN"), "https://github.com/Iotatg/ParvatiMusic"),
    ]])
}

fn repo_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if m.is_channel {
        return;
    }
    if m.is_private {
        db::add_served_user(m.chat_id);
    } else {
        db::add_served_chat(m.chat_id);
    }
    let mut name = bot.me_first_name();
    if name.is_empty() {
        name = "Parvati Music".to_string();
    }
    let text = f_args(loc(), m.chat_id, "repo_text", &[("bot", &escape_html(&name))]);
    let _ = bot.reply_text(m, &text, Some(&repo_markup(m.chat_id)));
}

// ── vclogger / vcstats ─────────────────────────────────────────────────────────

const VC_ANON: &str = "\u{26a0}\u{fe0f} \u{1d00}\u{1d1c}\u{1d0f}\u{1d1c}\u{1d0d}\u{1d0f}\u{1d1c}\u{1d1c}";
const VC_ALREADY: &str = "\u{2139}\u{fe0f} <b>VC logger is already active in this group.</b>";
const VC_ENABLED_MSG: &str = "\u{2705} <b>VC logger has been enabled!</b>";
const VC_DISABLED_MSG: &str = "\u{1f6ab} <b>VC logger has been disabled!</b>";
const VC_DENIED: &str = "\u{274c} <b>Permission denied!</b>";
const VC_INVALID: &str = "\u{274c} <b>Invalid argument</b>\n\n\u{27a1} <b>Usage:</b> <code>/vclogger [on / off]</code>";
const VC_STATE_ON: &str = "Enabled \u{2705}";
const VC_STATE_OFF: &str = "Disabled \u{274c}";
const VC_STATUS_A: &str = "\u{1f399} <b>VC logger status</b>\n\n\u{27a1} <b>Current state:</b> ";
const VC_STATUS_B: &str = "\n\n\u{27a1} <b>Usage:</b> <code>/vclogger [on / off]</code>";
const VC_ADMIN_ONLY: &str = "\u{274c} <b>Admin only!</b>";
const VC_ON: &str = "\u{2705} Enabled";
const VC_OFF: &str = "\u{274c} Disabled";
const VC_RUNNING_ON: &str = "\u{1f7e2} Running";
const VC_RUNNING_OFF: &str = "\u{1f534} Stopped";
const VC_STATS_A: &str = "\u{1f4ca} <b>VC logger stats</b>\n\n\u{27a1} <b>Logger :</b> ";
const VC_STATS_B: &str = "\n\u{27a1} <b>Monitor :</b> ";
const VC_STATS_C: &str = "\n\u{27a1} <b>User in VC :</b> <code>";

fn vc_enabled(chat_id: i64) -> bool {
    VC_LOGGER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&chat_id)
        .map(|(e, _)| *e)
        .unwrap_or(false)
}

fn vc_running(chat_id: i64) -> bool {
    VC_LOGGER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&chat_id)
        .map(|(_, r)| *r)
        .unwrap_or(false)
}

fn vc_set(chat_id: i64, enabled: bool, running: bool) {
    VC_LOGGER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(chat_id, (enabled, running));
}

fn vclogger_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if m.is_private {
        let _ = bot.reply_text(m, &f(loc(), m.chat_id, "only_supergroup"), None);
        db::add_served_user(m.chat_id);
        return;
    }
    if m.is_channel {
        return;
    }
    db::add_served_chat(m.chat_id);
    let chat_id = m.chat_id;
    let toks = fields(&args_of(m));

    if m.sender_id == 0 {
        if toks.is_empty() {
            let _ = bot.reply_text(m, &stylish(VC_ANON), None);
            return;
        }
        let arg = to_lower(&toks[0]);
        if arg == "on" || arg == "enable" || arg == "yes" {
            if vc_running(chat_id) {
                let _ = bot.reply_text(m, &stylish(VC_ALREADY), None);
                return;
            }
            vc_set(chat_id, true, true);
            let _ = bot.reply_text(m, &stylish(VC_ENABLED_MSG), None);
            return;
        }
        if arg == "off" || arg == "disable" || arg == "no" {
            vc_set(chat_id, false, false);
            let _ = bot.reply_text(m, &stylish(VC_DISABLED_MSG), None);
        }
        return;
    }

    if !privileged_gate(chat_id, m.sender_id) {
        let _ = bot.reply_text(m, &stylish(VC_DENIED), None);
        return;
    }

    if toks.is_empty() {
        let state = if vc_enabled(chat_id) { VC_STATE_ON } else { VC_STATE_OFF };
        let _ = bot.reply_text(m, &stylish(&format!("{VC_STATUS_A}{state}{VC_STATUS_B}")), None);
        return;
    }

    let arg = to_lower(&toks[0]);
    if arg == "on" || arg == "enable" || arg == "yes" {
        if vc_running(chat_id) {
            let _ = bot.reply_text(m, &stylish(VC_ALREADY), None);
            return;
        }
        vc_set(chat_id, true, true);
        let _ = bot.reply_text(m, &stylish(VC_ENABLED_MSG), None);
    } else if arg == "off" || arg == "disable" || arg == "no" {
        vc_set(chat_id, false, false);
        let _ = bot.reply_text(m, &stylish(VC_DISABLED_MSG), None);
    } else {
        let _ = bot.reply_text(m, &stylish(VC_INVALID), None);
    }
}

fn vcstats_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if m.is_private {
        let _ = bot.reply_text(m, &f(loc(), m.chat_id, "only_supergroup"), None);
        db::add_served_user(m.chat_id);
        return;
    }
    if m.is_channel {
        return;
    }
    let chat_id = m.chat_id;
    if m.sender_id != 0 && !privileged_gate(chat_id, m.sender_id) {
        let _ = bot.reply_text(m, &stylish(VC_ADMIN_ONLY), None);
        return;
    }
    let enabled_text = if vc_enabled(chat_id) { VC_ON } else { VC_OFF };
    let running_text = if vc_running(chat_id) { VC_RUNNING_ON } else { VC_RUNNING_OFF };
    let count = if active_room(chat_id) { 1 } else { 0 };
    let text = format!("{VC_STATS_A}{enabled_text}{VC_STATS_B}{running_text}{VC_STATS_C}{count}</code>");
    let _ = bot.reply_text(m, &stylish(&text), None);
}

// ── group gate ─────────────────────────────────────────────────────────────────

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

pub fn register(r: &mut Registry) {
    r.command("eval", eval_handler);
    r.command("ev", eval_handler);
    r.command("sh", shell_handler);
    r.command("bash", shell_handler);
    r.command("shell", shell_handler);
    r.command("json", json_handler);
    r.command("log", logs_handler);
    r.command("logs", logs_handler);
    r.command("logger", logger_handler);
    r.command("vclogger", vclogger_handler);
    r.command("vclog", vclogger_handler);
    r.command("vcstatus", vcstats_handler);
    r.command("vcstats", vcstats_handler);
    r.command("maintenance", maintenance_handler);
    r.command("maint", maintenance_handler);
    r.command("reload", reload_handler);
    r.command("creload", creload_handler);
    r.command("restart", restart_handler);
    r.command("privacy", privacy_handler);
    r.command("bug", bug_handler);
    r.command("repo", repo_handler);
    r.command("source", repo_handler);
    r.callback("restart:", restart_cb);
}
