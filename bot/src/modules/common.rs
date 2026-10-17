//! Shared module helpers.
//! (LiveMsg, cards, now-playing text, thumbnail-gated photo publishing).
use crate::buttons::RoomView;
use crate::locales::{LocaleArg, Locales};
use crate::models::Track;
use parvati_mtproto::messaging::{Markup, TgMessage};

use crate::mod_tg::Bot;
use crate::room::RoomState;
use crate::utils::{escape_html, format_duration, short_title};

/// command_of: the invoked command token with any @botname suffix stripped.
pub fn command_of(m: &TgMessage) -> String {
    match m.text.find(' ') {
        Some(sp) => m.text[..sp].to_string(),
        None => m.text.clone(),
    }
}

/// mention_of: HTML link for the sender's display name.
pub fn mention_of(m: &TgMessage) -> String {
    let full = format!("{} {}", m.first_name, m.last_name);
    let name = crate::utils::trim(&full);
    crate::utils::html_mention(if name.is_empty() { "user" } else { name }, m.sender_id)
}

/// Cancel flags for in-flight downloads, keyed by chat id (the cancel
/// keyboard callback flips the flag; the sink clears its entry when done).
static DL_CANCELS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<i64, std::sync::Arc<std::sync::atomic::AtomicBool>>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// Registers the cancel flag for a chat's current download.
pub fn dl_register_cancel(chat_id: i64, flag: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    DL_CANCELS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(chat_id, flag);
}

/// Removes the chat's cancel flag if it is still this download's flag.
pub fn dl_unregister_cancel(chat_id: i64, flag: &std::sync::Arc<std::sync::atomic::AtomicBool>) {
    let mut g = DL_CANCELS.lock().unwrap_or_else(|e| e.into_inner());
    if g.get(&chat_id).is_some_and(|f| std::sync::Arc::ptr_eq(f, flag)) {
        g.remove(&chat_id);
    }
}

/// flip-and-forget for the chat's active download; true when one existed.
pub fn cancel_download(chat_id: i64) -> bool {
    let mut g = DL_CANCELS.lock().unwrap_or_else(|e| e.into_inner());
    g.remove(&chat_id)
        .map(|f| {
            f.store(true, std::sync::atomic::Ordering::SeqCst);
            true
        })
        .unwrap_or(false)
}

/// Locale lookup with the chat's language (F(chat_id, key)).
pub fn f(loc: &Locales, chat_id: i64, key: &str) -> String {
    loc.get(&crate::db::language(chat_id), key, &LocaleArg::new())
}

/// F(chat_id, key, {{"k", v}}) with placeholder args.
pub fn f_args(loc: &Locales, chat_id: i64, key: &str, args: &[(&str, &str)]) -> String {
    let mut a = LocaleArg::new();
    for (k, v) in args {
        a.insert((*k).to_string(), (*v).to_string());
    }
    loc.get(&crate::db::language(chat_id), key, &a)
}

/// One live message that is created once and then edited in place (Go's
/// utils.EOR): if an edit fails the message is deleted and a fresh reply is
/// sent, matching edit-or-reply semantics.
pub struct LiveMsg {
    pub chat_id: i64,
    pub id: i64,
    pub reply_to: i64,
}

impl LiveMsg {
    pub fn new() -> LiveMsg {
        LiveMsg { chat_id: 0, id: 0, reply_to: 0 }
    }

    pub fn reply(
        &mut self,
        bot: &mut dyn Bot,
        chat_id: i64,
        text: &str,
        markup: Option<&Markup>,
        reply_to: i64,
    ) {
        self.chat_id = chat_id;
        self.reply_to = reply_to;
        self.id = bot
            .send_text_sync(chat_id, text, markup, reply_to)
            .unwrap_or(0);
        if self.id == 0 {
            let _ = bot.send_text(chat_id, text, markup);
        }
    }

    pub fn send(&mut self, bot: &mut dyn Bot, text: &str, markup: Option<&Markup>) {
        self.id = bot.send_text_sync(self.chat_id, text, markup, self.reply_to).unwrap_or(0);
        if self.id == 0 {
            let _ = bot.send_text(self.chat_id, text, markup);
        }
    }

    pub fn eor(&mut self, bot: &mut dyn Bot, text: &str, markup: Option<&Markup>) {
        if self.id != 0 && bot.edit_text(self.chat_id, self.id, text, markup).unwrap_or(false) {
            return;
        }
        self.send(bot, text, markup);
    }

    pub fn remove(&mut self, bot: &mut dyn Bot) {
        if self.id != 0 {
            let _ = bot.delete_message(self.chat_id, self.id);
        }
        self.id = 0;
    }
}

impl Default for LiveMsg {
    fn default() -> Self {
        Self::new()
    }
}

/// now_playing_text: locale stream_now_playing filled from the track.
pub fn now_playing_text(loc: &Locales, chat_id: i64, t: &Track) -> String {
    f_args(
        loc,
        chat_id,
        "stream_now_playing",
        &[
            ("url", t.url.as_str()),
            ("title", escape_html(&short_title(&t.title, 25)).as_str()),
            ("duration", format_duration(t.duration).as_str()),
            ("by", t.requester.as_str()),
        ],
    )
}

/// Edits a player-card message without knowing whether it is a photo or a
/// plain text card.
pub fn card_edit(
    bot: &mut dyn Bot,
    chat_id: i64,
    message_id: i64,
    text: &str,
    markup: Option<&Markup>,
    expect_photo: bool,
) -> bool {
    if expect_photo && bot.edit_caption(chat_id, message_id, text, markup, true).unwrap_or(false) {
        return true;
    }
    bot.edit_text(chat_id, message_id, text, markup).unwrap_or(false)
}

/// Publish the track card: local artwork photo unless thumbnails are
/// disabled, plain text otherwise. Returns (message_id, is_photo).
pub fn publish_card(
    bot: &mut dyn Bot,
    chat_id: i64,
    t: &Track,
    text: &str,
    mk: Option<&Markup>,
) -> (i64, bool) {
    if !t.artwork.is_empty() && !crate::db::thumbnails_disabled(chat_id) {
        let local = crate::platforms::registry::artwork_local_path(&t.artwork);
        if !local.is_empty() {
            let id = bot.send_photo_sync(chat_id, &local, true, text, mk, 0).unwrap_or(0);
            if id != 0 {
                return (id, true);
            }
        }
    }
    let id = bot.send_text_sync(chat_id, text, mk, 0).unwrap_or(0);
    (id, false)
}

/// send_now_playing: publish the card, take over the room's status slot
/// and delete the previous card.
pub fn send_now_playing(bot: &mut dyn Bot, chat_id: i64, r: &mut RoomState, t: &Track, loc: &Locales) {
    let old = r.status_msg;
    let text = now_playing_text(loc, chat_id, t);
    let mk = play_markup(loc, chat_id, &r.view(), false);
    let (id, photo) = publish_card(bot, chat_id, t, &text, Some(&mk));
    r.status_msg = id;
    r.status_is_photo = photo;
    if old != 0 && old != id {
        let _ = bot.delete_message(chat_id, old);
    }
}

fn play_markup(loc: &Locales, chat_id: i64, view: &RoomView, queued: bool) -> Markup {
    let lang = crate::db::language(chat_id);
    crate::buttons::play_markup(loc, &lang, view, queued)
}

/// Monitor set_status: edit the live slot in place, deleting + resending when
/// the edit fails.
pub fn set_status(bot: &mut dyn Bot, r: &mut RoomState, text: &str) {    let mid = r.status_msg;
    if mid != 0 {
        if r.status_is_photo && bot.edit_caption(r.chat_id, mid, text, None, true).unwrap_or(false) {
            return;
        }
        if !r.status_is_photo && bot.edit_text(r.chat_id, mid, text, None).unwrap_or(false) {
            return;
        }
        let _ = bot.delete_message(r.chat_id, mid);
    }
    let id = bot.send_text_sync(r.chat_id, text, None, 0).unwrap_or(0);
    if id != 0 {
        r.status_msg = id;
        r.status_is_photo = false;
    }
}

/// show_track: publish the now-playing card, retire the
/// live /play message and delete the previous card. Falls back to an in-place
/// edit of the live message when the card could not be published.
pub fn show_track(
    lm: &mut LiveMsg,
    bot: &mut dyn Bot,
    chat_id: i64,
    r: &mut RoomState,
    t: &Track,
    text: &str,
    queued: bool,
) {
    let old = r.status_msg;
    let mk = play_markup(super::loc(), chat_id, &r.view(), queued);
    let (id, photo) = publish_card(bot, chat_id, t, text, Some(&mk));
    if id == 0 {
        lm.eor(bot, text, Some(&mk));
        r.status_msg = lm.id;
        r.status_is_photo = false;
        return;
    }
    lm.remove(bot);
    r.status_msg = id;
    r.status_is_photo = photo;
    if old != 0 && old != id {
        let _ = bot.delete_message(chat_id, old);
    }
}

/// show_track_queued: a queued reply edits the live
/// /play message in place and leaves the room's now-playing card alone.
pub fn show_track_queued(lm: &mut LiveMsg, bot: &mut dyn Bot, chat_id: i64, r: &RoomState, text: &str) {
    let mk = play_markup(super::loc(), chat_id, &r.view(), true);
    lm.eor(bot, text, Some(&mk));
}
