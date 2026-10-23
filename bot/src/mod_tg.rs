// mod_tg — the bot-mode facade.
//
// Defines the `Bot` trait every command module calls, plus the in-memory
// `MockBot` test double. The production backend is `FerogramBot` (see
// `ferogram_bot.rs`), which drives the vendored ferogram MTProto framework.
// There is no Bot API HTTP layer: everything rides MTProto, like the
// reference bots.

use parvati_mtproto::messaging::{Markup, TgMessage};


/// Identity of a sticker document sufficient to re-send it over MTProto.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StickerRef {
    pub doc_id: i64,
    pub doc_access_hash: i64,
    pub doc_file_ref: Vec<u8>,
    pub emoji: String,
}

/// The surface subset the command modules actually call.
pub trait Bot: Send {
    /// One idle-pump cycle on the backing transport (called from the main
    /// loop); default is inert for fakes.
    fn pump_step(&mut self, idle: std::time::Duration) -> Result<(), String> {
        let _ = idle;
        Ok(())
    }

    /// New handshake + login after a transport death; default no-op fakes.
    fn reconnect(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn send_text(&mut self, chat_id: i64, text: &str, markup: Option<&Markup>) -> Result<bool, String>;
    fn send_text_sync(
        &mut self,
        chat_id: i64,
        text: &str,
        markup: Option<&Markup>,
        reply_to: i64,
    ) -> Result<i64, String>;
    fn reply_text(&mut self, msg: &TgMessage, text: &str, markup: Option<&Markup>) -> Result<bool, String>;
    fn edit_text(
        &mut self,
        chat_id: i64,
        message_id: i64,
        text: &str,
        markup: Option<&Markup>,
    ) -> Result<bool, String>;
    fn send_photo(
        &mut self,
        chat_id: i64,
        photo: &str,
        caption: &str,
        markup: Option<&Markup>,
    ) -> Result<bool, String>;
    fn send_photo_sync(
        &mut self,
        chat_id: i64,
        photo: &str,
        local_path: bool,
        caption: &str,
        markup: Option<&Markup>,
        reply_to: i64,
    ) -> Result<i64, String>;
    fn edit_caption(
        &mut self,
        chat_id: i64,
        message_id: i64,
        caption: &str,
        markup: Option<&Markup>,
        quiet: bool,
    ) -> Result<bool, String>;
    fn edit_markup(
        &mut self,
        chat_id: i64,
        message_id: i64,
        markup: Option<&Markup>,
        quiet: bool,
    ) -> Result<bool, String>;
    fn answer_callback(&mut self, query_id: i64, text: &str, alert: bool) -> Result<bool, String>;
    fn delete_message(&mut self, chat_id: i64, message_id: i64) -> Result<bool, String>;

    // --- moderation ops ---------------------------------------------------
    /// Ban with an absolute until-timestamp (0 = permanent); kick is this
    /// followed by an immediate unban.
    fn ban_member(&mut self, chat_id: i64, user_id: i64, until: i64) -> Result<bool, String>;
    fn unban_member(&mut self, chat_id: i64, user_id: i64) -> Result<bool, String>;
    /// Restrict to read-only (warn-limit mute penalty).
    fn mute_member(&mut self, chat_id: i64, user_id: i64) -> Result<bool, String>;
    /// Promote with a rights bitset (Bot API promoteChatMember flags).
    fn promote_member(&mut self, chat_id: i64, user_id: i64, rights: &PromoteRights) -> Result<bool, String>;
    /// Demote back to a plain member (Bot API: all flags false).
    fn demote_member(&mut self, chat_id: i64, user_id: i64) -> Result<bool, String>;
    fn pin_message(&mut self, chat_id: i64, message_id: i64, silent: bool) -> Result<bool, String>;
    fn unpin_message(&mut self, chat_id: i64, message_id: i64) -> Result<bool, String>;
    fn unpin_all_messages(&mut self, chat_id: i64) -> Result<bool, String>;

 // --- chat_state queries -------------------------
 /// Membership snapshot of `user_id` in `chat_id`.
    fn get_chat_member_state(&mut self, chat_id: i64, user_id: i64) -> MemberState;
    /// Public @username of a supergroup, empty when private/unknown
    fn chat_public_username(&mut self, chat_id: i64) -> String;
 /// Mint an invite link the assistant can import.
    fn create_chat_invite_link(&mut self, chat_id: i64) -> Result<String, String>;
    /// Approve the assistant's pending join request.
    fn approve_join_request(&mut self, chat_id: i64, user_id: i64) -> Result<bool, String>;

    /// Sender user id of the message being replied to (0 when unknown). Lets
    /// admin commands target the replied user without an @username/id arg.
    fn get_replied_sender(&mut self, chat_id: i64, message_id: i64) -> i64;

    /// Raise the "choosing a sticker" chat indicator in `chat_id`.
    fn send_chat_action_choosing_sticker(&mut self, chat_id: i64) -> Result<bool, String>;

    /// Show "typing…" in the chat. Transports that cannot do it return Ok(false).
    fn send_typing(&mut self, _chat_id: i64) -> Result<bool, String> {
        Ok(false)
    }

    /// Send a sticker by document identity. `reply_to` = 0 for none.
    fn send_sticker(
        &mut self,
        chat_id: i64,
        doc_id: i64,
        doc_access_hash: i64,
        doc_file_ref: &[u8],
        reply_to: i64,
    ) -> Result<i64, String>;

    /// Fetch the sticker attached to the replied message (None when the
    /// message is missing or carries no sticker).
    fn get_replied_sticker(&mut self, chat_id: i64, message_id: i64) -> Option<StickerRef>;


    fn me_id(&self) -> i64;
    fn me_username(&self) -> String;
    fn me_first_name(&self) -> String;
    fn ready(&self) -> bool;
}

/// Membership snapshot fields. `admin` is true for creator/administrator;
/// the can_* flags mirror the Bot API moderation rights.
#[derive(Clone, Debug, Default)]
pub struct MemberState {
    pub ok: bool,
    pub present: bool,
    pub banned: bool,
    pub error: String,
    pub admin: bool,
    pub can_restrict: bool,
    pub can_promote: bool,
    pub can_pin: bool,
    pub can_delete: bool,
}

/// Promote rights bitset mapped to Bot API promoteChatMember flags
/// (order: manage, change_info, del, invite,
/// restrict, pin, promote, video).
#[derive(Clone, Copy, Debug, Default)]
pub struct PromoteRights {
    pub manage: bool,
    pub change_info: bool,
    pub del: bool,
    pub invite: bool,
    pub restrict: bool,
    pub pin: bool,
    pub promote: bool,
    pub video: bool,
}


/// Test double recording `"{op} {chat_id}"` per call (plan Step 1 format).
/// Moderation ops record richer payloads in `calls` (ban/promote/pin args),
/// deleted message ids land in `dels()`, and `users` scripts per-user member
/// snapshots that override the chat-wide default (admin targets, callers).
pub struct MockBot {
    calls: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    texts: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    member: std::sync::Arc<std::sync::Mutex<MemberState>>,
    users: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<i64, MemberState>>>,
    dels: std::sync::Arc<std::sync::Mutex<Vec<(i64, i64)>>>,
    username: std::sync::Arc<std::sync::Mutex<String>>,
    invite: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    approve: std::sync::Arc<std::sync::Mutex<bool>>,
    replied_sender: std::sync::Arc<std::sync::Mutex<i64>>,
    replied_sticker: std::sync::Arc<std::sync::Mutex<Option<StickerRef>>>,
}

impl Default for MockBot {
    fn default() -> Self {
        Self::new()
    }
}

impl MockBot {
    pub fn new() -> MockBot {
        MockBot {
            calls: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            texts: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            member: std::sync::Arc::new(std::sync::Mutex::new(MemberState {
                ok: true,
                present: true,
                ..Default::default()
            })),
            users: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            dels: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            username: std::sync::Arc::new(std::sync::Mutex::new(String::new())),
            invite: std::sync::Arc::new(std::sync::Mutex::new(None)),
            approve: std::sync::Arc::new(std::sync::Mutex::new(true)),
            replied_sender: std::sync::Arc::new(std::sync::Mutex::new(0)),
            replied_sticker: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Scripts the assistant membership snapshot (default: member present).
    pub fn set_member_state(&self, s: MemberState) {
        *self.member.lock().unwrap_or_else(|e| e.into_inner()) = s;
    }

    /// Scripts one user's snapshot, overriding the chat-wide default.
    pub fn set_user_state(&self, user_id: i64, s: MemberState) {
        self.users
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(user_id, s);
    }

    /// Deleted (chat, message) pairs recorded by delete_message.
    pub fn dels(&self) -> Vec<(i64, i64)> {
        self.dels.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Scripts the chat's public @username (empty = private chat).
    pub fn set_public_username(&self, u: &str) {
        *self.username.lock().unwrap_or_else(|e| e.into_inner()) = u.to_string();
    }

    /// Scripts the invite link createChatInviteLink returns.
    pub fn set_invite_link(&self, link: &str) {
        *self.invite.lock().unwrap_or_else(|e| e.into_inner()) = Some(link.to_string());
    }

    /// Scripts whether approveChatJoinRequest succeeds.
    pub fn set_approve_join(&self, ok: bool) {
        *self.approve.lock().unwrap_or_else(|e| e.into_inner()) = ok;
    }

    /// Scripts the sender id get_replied_sender returns.
    pub fn set_replied_sender(&self, user_id: i64) {
        *self.replied_sender.lock().unwrap_or_else(|e| e.into_inner()) = user_id;
    }

    /// Scripts the sticker get_replied_sticker returns.
    pub fn set_replied_sticker(&self, s: Option<StickerRef>) {
        *self.replied_sticker.lock().unwrap_or_else(|e| e.into_inner()) = s;
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Text/caption payloads, one entry per recorded call (cards, status
    /// messages) — lets tests assert locale content, not just op names.
    pub fn texts(&self) -> Vec<String> {
        self.texts.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn record(&self, op: &str, chat_id: i64) {
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(format!("{op} {chat_id}"));
        self.texts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(String::new());
    }

    /// {"op} {chat} {args}" per call — richer payloads than `record` with the
    /// same text-payload behavior (used by the moderation op records).
    fn record_args(&self, op: &str, chat_id: i64, args: &str) {
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(format!("{op} {chat_id} {args}"));
        self.texts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(args.to_string());
    }

    fn record_text(&self, op: &str, chat_id: i64, text: &str) {
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(format!("{op} {chat_id}"));
        self.texts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(text.to_string());
    }
}

impl Bot for MockBot {
    fn send_text(&mut self, chat_id: i64, text: &str, _markup: Option<&Markup>) -> Result<bool, String> {
        self.record_text("send_text", chat_id, text);
        Ok(true)
    }

    fn send_text_sync(
        &mut self,
        chat_id: i64,
        text: &str,
        _markup: Option<&Markup>,
        _reply_to: i64,
    ) -> Result<i64, String> {
        self.record_text("send_text_sync", chat_id, text);
        Ok(4242)
    }

    fn reply_text(&mut self, msg: &TgMessage, text: &str, _markup: Option<&Markup>) -> Result<bool, String> {
        self.record_text("reply_text", msg.chat_id, text);
        Ok(true)
    }

    fn edit_text(
        &mut self,
        chat_id: i64,
        _message_id: i64,
        text: &str,
        _markup: Option<&Markup>,
    ) -> Result<bool, String> {
        self.record_text("edit_text", chat_id, text);
        Ok(true)
    }

    fn send_photo(
        &mut self,
        chat_id: i64,
        _photo: &str,
        caption: &str,
        _markup: Option<&Markup>,
    ) -> Result<bool, String> {
        self.record_text("send_photo", chat_id, caption);
        Ok(true)
    }

    fn send_photo_sync(
        &mut self,
        chat_id: i64,
        _photo: &str,
        _local_path: bool,
        caption: &str,
        _markup: Option<&Markup>,
        _reply_to: i64,
    ) -> Result<i64, String> {
        self.record_text("send_photo_sync", chat_id, caption);
        Ok(4242)
    }

    fn edit_caption(
        &mut self,
        chat_id: i64,
        _message_id: i64,
        caption: &str,
        _markup: Option<&Markup>,
        _quiet: bool,
    ) -> Result<bool, String> {
        self.record_text("edit_caption", chat_id, caption);
        Ok(true)
    }

    fn edit_markup(
        &mut self,
        chat_id: i64,
        _message_id: i64,
        _markup: Option<&Markup>,
        _quiet: bool,
    ) -> Result<bool, String> {
        self.record("edit_markup", chat_id);
        Ok(true)
    }

    fn answer_callback(&mut self, _query_id: i64, text: &str, _alert: bool) -> Result<bool, String> {
        // Query ids are not chats: record the op with the query id for parity.
        self.record("answer_callback", _query_id);
        let _ = text;
        Ok(true)
    }

    fn delete_message(&mut self, chat_id: i64, message_id: i64) -> Result<bool, String> {
        self.record("delete_message", chat_id);
        self.dels
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((chat_id, message_id));
        Ok(message_id != 0)
    }

    fn ban_member(&mut self, chat_id: i64, user_id: i64, until: i64) -> Result<bool, String> {
        self.record_args("ban", chat_id, &format!("{user_id} {until}"));
        Ok(true)
    }

    fn unban_member(&mut self, chat_id: i64, user_id: i64) -> Result<bool, String> {
        self.record_args("unban", chat_id, &format!("{user_id} 0"));
        Ok(true)
    }

    fn mute_member(&mut self, chat_id: i64, user_id: i64) -> Result<bool, String> {
        self.record_args("mute", chat_id, &format!("{user_id} 0"));
        Ok(true)
    }

    fn promote_member(&mut self, chat_id: i64, user_id: i64, rights: &PromoteRights) -> Result<bool, String> {
        let flag = |v: bool| if v { "1" } else { "0" };
        let detail = format!(
            "manage={},change_info={},del={},invite={},restrict={},pin={},promote={},video={}",
            flag(rights.manage),
            flag(rights.change_info),
            flag(rights.del),
            flag(rights.invite),
            flag(rights.restrict),
            flag(rights.pin),
            flag(rights.promote),
            flag(rights.video),
        );
        self.record_args("promote", chat_id, &format!("{user_id} {detail}"));
        Ok(true)
    }

    fn demote_member(&mut self, chat_id: i64, user_id: i64) -> Result<bool, String> {
        self.record_args("demote", chat_id, &format!("{user_id}"));
        Ok(true)
    }

    fn pin_message(&mut self, chat_id: i64, message_id: i64, silent: bool) -> Result<bool, String> {
        self.record_args("pin", chat_id, &format!("{message_id} silent={}", if silent { 1 } else { 0 }));
        Ok(true)
    }

    fn unpin_message(&mut self, chat_id: i64, message_id: i64) -> Result<bool, String> {
        self.record_args("unpin", chat_id, &format!("{message_id}"));
        Ok(true)
    }

    fn unpin_all_messages(&mut self, chat_id: i64) -> Result<bool, String> {
        self.record("unpinall", chat_id);
        Ok(true)
    }

    fn get_chat_member_state(&mut self, chat_id: i64, user_id: i64) -> MemberState {
        self.record("get_chat_member_state", chat_id);
        if let Some(s) = self
            .users
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&user_id)
        {
            return s.clone();
        }
        self.member.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn chat_public_username(&mut self, chat_id: i64) -> String {
        self.record("chat_public_username", chat_id);
        self.username.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn create_chat_invite_link(&mut self, chat_id: i64) -> Result<String, String> {
        self.record("create_chat_invite_link", chat_id);
        self.invite
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| "bot: createChatInviteLink returned an empty link".into())
    }

    fn approve_join_request(&mut self, chat_id: i64, _user_id: i64) -> Result<bool, String> {
        self.record("approve_join_request", chat_id);
        if *self.approve.lock().unwrap_or_else(|e| e.into_inner()) {
            Ok(true)
        } else {
            Err("telegram_error:400:USER_NOT_PARTICIPANT".into())
        }
    }

    fn get_replied_sender(&mut self, chat_id: i64, message_id: i64) -> i64 {
        self.record("get_replied_sender", chat_id);
        let _ = message_id;
        *self.replied_sender.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn send_chat_action_choosing_sticker(&mut self, chat_id: i64) -> Result<bool, String> {
        self.record("send_chat_action_choosing_sticker", chat_id);
        Ok(true)
    }

    fn send_sticker(
        &mut self,
        chat_id: i64,
        doc_id: i64,
        _doc_access_hash: i64,
        _doc_file_ref: &[u8],
        reply_to: i64,
    ) -> Result<i64, String> {
        self.record(&format!("send_sticker {doc_id} {reply_to}"), chat_id);
        Ok(1)
    }

    fn get_replied_sticker(&mut self, chat_id: i64, message_id: i64) -> Option<StickerRef> {
        self.record("get_replied_sticker", chat_id);
        let _ = message_id;
        self.replied_sticker.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn me_id(&self) -> i64 {
        42
    }

    fn me_username(&self) -> String {
        "parvati_bot".into()
    }

    fn me_first_name(&self) -> String {
        "Parvati".into()
    }

    fn ready(&self) -> bool {
        true
    }
}
