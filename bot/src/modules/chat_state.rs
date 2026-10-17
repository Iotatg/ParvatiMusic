//! chat_state — assistant membership guard.
//! Runs before anything is queued: the assistant must be a chat member or be
//! able to join, otherwise playback can never start.
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use crate::modules::common::{f, f_args};
use crate::modules::loc;
use crate::room::RoomState;
use crate::utils::escape_html;

/// isAdminError: these names mean the *bot* lacks admin rights
/// rather than the assistant failing to join.
fn is_admin_error(err: &str) -> bool {
    ["CHAT_ID_INVALID", "CHAT_ADMIN_REQUIRED", "CHANNEL_PRIVATE", "CHANNEL_INVALID"]
        .iter()
        .any(|m| err.contains(m))
}

/// "https://t.me/+AbCdEf" / "https://t.me/joinchat/AbCdEf" -> "AbCdEf"
fn invite_hash_from_link(link: &str) -> String {
    let tail = link.rsplit('/').next().unwrap_or(link);
    tail.strip_prefix('+').unwrap_or(tail).to_string()
}

/// "telegram_error:420:FLOOD_WAIT_31" -> 31.
fn flood_wait_seconds(err: &str) -> i64 {
    let Some(p) = err.find("FLOOD_WAIT_") else { return 0 };
    let digits: String = err[p + 11..].chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().unwrap_or(0)
}

fn flood_text(chat_id: i64, err: &str) -> String {
    let n = flood_wait_seconds(err);
    if n <= 0 {
        return String::new();
    }
    let loc = loc();
    if n >= 60 {
        f_args(loc, chat_id, "flood_minutes", &[("duration", &((n + 59) / 60).to_string())])
    } else {
        f_args(loc, chat_id, "flood_seconds", &[("duration", &n.to_string())])
    }
}

/// ensure_voice_chat_ready. Empty string = ready;
/// any returned text is a locale message the caller shows to the user.
pub fn ensure_voice_chat_ready(
    bot: &mut dyn crate::mod_tg::Bot,
    r: &Arc<Mutex<RoomState>>,
    assistants: &crate::assistant::AssistantManager,
    notify_chat_id: i64,
) -> String {
    let loc = loc();
    let room_id = r.lock().unwrap_or_else(|e| e.into_inner()).id;
    let Some(ass) = assistants.for_chat(room_id) else {
        return f(loc, notify_chat_id, "err_assistant_get_failed");
    };
    // Session strings from some generators carry no embedded user id; resolve
 // it via users.getUsers(inputUserSelf) once.
    let mut user_id = ass.user_id;
    if user_id == 0 {
        let mut ctx = ass.ntg();
        let Some(mt) = ctx.mt_mut() else {
            return f(loc, notify_chat_id, "err_assistant_get_failed");
        };
        match mt.self_user() {
            Ok((id, _)) => user_id = id,
            Err(e) => {
                crate::parv_error!("chat_state", "assistant self lookup failed: {e}");
                return f(loc, notify_chat_id, "err_assistant_get_failed");
            }
        }
    }

    // Membership snapshot from the bot (needs admin in the chat, like
 // FullChat + GetChatMember)
    let info = bot.get_chat_member_state(room_id, user_id);
    if !info.ok {
        if is_admin_error(&info.error) {
            return f(loc, notify_chat_id, "err_admin_permission_required");
        }
        return f_args(
            loc,
            notify_chat_id,
            "err_fetch_failed",
            &[("error", &escape_html(&info.error))],
        );
    }
    if info.banned {
 // Rust assistants carry no cached first name;
        // the numeric id stands in for {user}.
        return f_args(
            loc,
            notify_chat_id,
            "err_assistant_banned",
            &[("user", &user_id.to_string()), ("id", &user_id.to_string())],
        );
    }
    if info.present {
        return String::new();
    }

    // Not a member -> join. Prefer the public username, else a fresh invite.
    let username_slot = bot.chat_public_username(room_id);
    let mut ctx = ass.ntg();
    let mut join_once = |username: &str| -> Result<(), String> {
        if !username.is_empty() {
            let Some(mt) = ctx.mt_mut() else {
                return Err(crate::context::NO_SESSION_ERR.to_string());
            };
            return mt.join_channel_by_username(username);
        }
        match bot.create_chat_invite_link(room_id) {
            Ok(link) => {
                let hash = invite_hash_from_link(&link);
                if hash.is_empty() {
                    return Err(format!("bot returned an invite link without a hash: {link}"));
                }
                let Some(mt) = ctx.mt_mut() else {
                    return Err(crate::context::NO_SESSION_ERR.to_string());
                };
                mt.join_channel_by_invite(&hash)
            }
            Err(e) => Err(e),
        }
    };

    let first = join_once(&username_slot);
    let join_err = match first {
        Ok(()) => return String::new(),
        Err(e) => e,
    };
    if join_err.contains("INVITE_REQUEST_SENT") {
        // Assistant sent a join request (join-by-request chat); approve it as
 // the bot, like approveJoinRequest
        if bot.approve_join_request(room_id, user_id).is_err() {
            return f(loc, notify_chat_id, "err_assistant_join_request_sent");
        }
        thread::sleep(Duration::from_secs(1));
        return String::new();
    }
    let fw = flood_wait_seconds(&join_err);
    if fw > 0 {
        // Short floods: sleep and retry once; long ones surface to the user.
        if fw <= 15 {
            thread::sleep(Duration::from_secs((fw + 1) as u64));
            if join_once(&username_slot).is_ok() {
                return String::new();
            }
        }
        let msg = flood_text(notify_chat_id, &join_err);
        return if msg.is_empty() {
            f_args(
                loc,
                notify_chat_id,
                "err_assistant_invite_failed",
                &[("error", &escape_html(&join_err))],
            )
        } else {
            msg
        };
    }
    if is_admin_error(&join_err) {
        return f(loc, notify_chat_id, "err_admin_permission_required");
    }
    f_args(
        loc,
        notify_chat_id,
        "err_assistant_invite_failed",
        &[("error", &escape_html(&join_err))],
    )
}
