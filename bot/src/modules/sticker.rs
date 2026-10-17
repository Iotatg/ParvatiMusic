//! Sticker curation commands and the emoji mood classifier.
//!
//! Port of the reference sticker module's owner-facing surface (addsticker,
//! stickerpacks, previewsticker, clearstickers, stickerdiag) plus the mood
//! tables the reply engine relies on. V1 deviations: the replied-message fetch
//! and sticker-send ops are not wired, so addsticker/previewsticker/stickerdiag
//! report the unavailable path instead of performing the network work; the mood
//! classifier and pack storage are unchanged.

use parvati_mtproto::messaging::{MediaKind, TgMessage};

use crate::db;
use crate::mod_tg::{Bot, StickerRef};
use crate::modules::{cfg, Registry};
use crate::utils::{escape_html, stylish, to_lower, trim};

// ── mood tables ────────────────────────────────────────────────────────────────

fn mood_emoji(emoji: &str) -> Option<&'static str> {
    Some(match emoji {
        "\u{1f602}" | "\u{1f923}" | "\u{1f639}" | "\u{1f480}" => "laugh",
        "\u{1f60d}" | "\u{1f970}" | "\u{1f495}" | "\u{1f49e}" | "\u{2764}" | "\u{1faf6}"
        | "\u{1f49d}" | "\u{1f498}" => "love",
        "\u{1f62d}" | "\u{1f622}" | "\u{1f494}" | "\u{1f614}" | "\u{1f61e}" => "sad",
        "\u{1f621}" | "\u{1f92c}" | "\u{1f624}" | "\u{1f4a2}" => "angry",
        "\u{1f973}" | "\u{1f60a}" | "\u{1f601}" | "\u{1f389}" | "\u{1f604}" | "\u{1f92e}"
        | "\u{1f38a}" => "happy",
        "\u{1f62e}" | "\u{1f631}" | "\u{1f632}" | "\u{1f440}" => "surprise",
        "\u{1f483}" | "\u{1f57a}" | "\u{1f3b5}" | "\u{1f3b6}" => "dance",
        "\u{1f60e}" | "\u{1f919}" | "\u{270c}" => "cool",
        "\u{1f97a}" | "\u{1f338}" | "\u{2728}" | "\u{1f337}" | "\u{1f351}" | "\u{1f33c}" => "cute",
        _ => return None,
    })
}

fn mood_from_emoji(s: &str) -> String {
    let mut result = "default";
    for ch in s.chars() {
        let mut buf = [0u8; 4];
        let cp = ch.encode_utf8(&mut buf);
        if result == "default" {
            if let Some(m) = mood_emoji(cp) {
                result = m;
            }
        }
    }
    result.to_string()
}

fn mood_related(mood: &str) -> Vec<&'static str> {
    match mood {
        "happy" => vec!["laugh", "dance", "cute", "cool"],
        "laugh" => vec!["happy", "cute", "dance"],
        "love" => vec!["cute", "sad", "happy"],
        "cute" => vec!["love", "happy", "laugh"],
        "sad" => vec!["love", "cute"],
        "angry" => vec!["cool", "surprise"],
        "surprise" => vec!["cool", "angry"],
        "dance" => vec!["happy", "cool", "laugh"],
        "cool" => vec!["happy", "dance", "surprise"],
        _ => vec![],
    }
}

fn mood_from_text(text: &str) -> String {
    let mood = mood_from_emoji(text);
    if mood != "default" {
        return mood;
    }
    let low = to_lower(text);
    let table: [(&[&str], &str); 5] = [
        (&["dance", "vibe", "groove"], "dance"),
        (&["love", "kiss", "heart", "pyaar"], "love"),
        (&["funny", "lol", "haha", "laugh"], "laugh"),
        (&["angry", "mad", "rage", "gussa"], "angry"),
        (&["cute", "kawaii", "aww", "soft"], "cute"),
    ];
    for (words, mood) in table {
        if words.iter().any(|w| low.contains(w)) {
            return mood.to_string();
        }
    }
    "default".to_string()
}

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

fn owner_only(m: &TgMessage) -> bool {
    let c = cfg();
    c.owner_id != 0 && c.owner_id == m.sender_id
}

fn owner_only_msg() -> String {
    stylish("Only the bot owner can use this command.")
}

// ── sticker identity encoding ────────────────────────────────────────────────
// Packs store a compact string per sticker: "doc_id:access_hash:hex(file_ref)".
// This carries everything messages.sendMedia needs to re-send the sticker, so
// no per-session file_id lookup is required.

fn hex_encode(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

fn encode_sticker(r: &StickerRef) -> String {
    format!(
        "{}:{}:{}",
        r.doc_id,
        r.doc_access_hash,
        hex_encode(&r.doc_file_ref)
    )
}

fn decode_sticker(s: &str) -> Option<StickerRef> {
    let mut parts = s.splitn(3, ':');
    let doc_id = parts.next()?.parse::<i64>().ok()?;
    let doc_access_hash = parts.next()?.parse::<i64>().ok()?;
    let file_ref = hex_decode(parts.next()?).unwrap_or_default();
    Some(StickerRef { doc_id, doc_access_hash, doc_file_ref: file_ref, emoji: String::new() })
}

fn send_sticker_ref(bot: &mut dyn Bot, m: &TgMessage, s: &StickerRef) -> bool {
    bot.send_sticker(m.chat_id, s.doc_id, s.doc_access_hash, &s.doc_file_ref, m.id)
        .is_ok()
}

// ── reply engine ─────────────────────────────────────────────────────────────

/// True when a sticker message should get a sticker reply: DMs always; groups
/// only on a reply to the bot or an @mention of the bot.
fn should_respond(bot: &mut dyn Bot, m: &TgMessage) -> bool {
    if m.is_private {
        return true;
    }
    if m.reply_to != 0 && bot.get_replied_sender(m.chat_id, m.reply_to) == bot.me_id() {
        return true;
    }
    let uname = to_lower(&bot.me_username());
    if uname.is_empty() {
        return false;
    }
    to_lower(&m.text).contains(&format!("@{uname}"))
}

/// stickerReplyHandler parity: exact mood, then related moods, then any pack.
fn sticker_engine(bot: &mut dyn Bot, m: &TgMessage) {
    if m.media_kind != MediaKind::Sticker || m.doc_id == 0 {
        return;
    }
    if !should_respond(bot, m) {
        return;
    }
    let _ = bot.send_chat_action_choosing_sticker(m.chat_id);

    let mood = mood_from_text(&m.sticker_emoji);
    let incoming = encode_sticker(&StickerRef {
        doc_id: m.doc_id,
        doc_access_hash: m.doc_access_hash,
        doc_file_ref: m.doc_file_ref.clone(),
        emoji: m.sticker_emoji.clone(),
    });
    db::add_sticker_to_pack(&mood, &incoming);

    let mut chain = vec![mood.clone()];
    chain.extend(mood_related(&mood).iter().map(|s| s.to_string()));
    for cand in chain {
        let ids = db::sticker_pack_ids(&cand);
        if let Some(s) = pick_other(&ids, &incoming).and_then(|id| decode_sticker(&id)) {
            if send_sticker_ref(bot, m, &s) {
                return;
            }
        }
    }
    for (_pack_mood, ids) in db::sticker_packs() {
        if let Some(s) = pick_other(&ids, &incoming).and_then(|id| decode_sticker(&id)) {
            if send_sticker_ref(bot, m, &s) {
                return;
            }
        }
    }
}

/// Chooses a random id from `ids` that differs from `current`.
fn pick_other(ids: &[String], current: &str) -> Option<String> {
    let candidates: Vec<&String> = ids.iter().filter(|i| !i.is_empty() && *i != current).collect();
    if candidates.is_empty() {
        return None;
    }
    let idx = (now_millis() as usize) % candidates.len();
    Some(candidates[idx].clone())
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

// ── handlers ───────────────────────────────────────────────────────────────────

fn add_sticker_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !owner_only(m) {
        let _ = bot.reply_text(m, &owner_only_msg(), None);
        return;
    }
    if m.reply_to == 0 {
        let _ = bot.reply_text(m, &stylish("\u{1f4ce} reply to a sticker to save it."), None);
        return;
    }
    let Some(st) = bot.get_replied_sticker(m.chat_id, m.reply_to) else {
        let _ = bot.reply_text(m, &stylish("\u{1f4ce} reply to a sticker to save it."), None);
        return;
    };
    let toks = fields(&args_of(m));
    let mut mood = if toks.is_empty() { String::new() } else { to_lower(&toks[0]) };
    if mood.is_empty() {
        mood = mood_from_emoji(&st.emoji);
    }
    db::add_sticker_to_pack(&mood, &encode_sticker(&st));
    let _ = bot.reply_text(
        m,
        &stylish(&format!(
            "\u{2705} added to <b>{}</b>. {} in pack.",
            escape_html(&mood),
            db::sticker_pack_ids(&mood).len()
        )),
        None,
    );
}

fn sticker_packs_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !owner_only(m) {
        let _ = bot.reply_text(m, &owner_only_msg(), None);
        return;
    }
    let packs = db::sticker_packs();
    if packs.is_empty() {
        let _ = bot.reply_text(m, &stylish("\u{1f4ad} no sticker packs. /addsticker"), None);
        return;
    }
    let mut text = format!("\u{1f3ad} <b>packs ({})</b>", packs.len());
    for (mood, ids) in &packs {
        text.push_str(&format!(
            "\n\u{2022} <b>{}</b> \u{2014} {}",
            escape_html(mood),
            ids.len()
        ));
    }
    let _ = bot.reply_text(m, &text, None);
}

fn preview_sticker_handler(bot: &mut dyn Bot, m: &TgMessage) {
    let toks = fields(&args_of(m));
    if toks.is_empty() {
        let _ = bot.reply_text(m, &stylish("\u{1f4ce} usage: /previewsticker <mood>"), None);
        return;
    }
    let mood = to_lower(&toks[0]);
    let ids = db::sticker_pack_ids(&mood);
    if ids.is_empty() {
        let _ = bot.reply_text(
            m,
            &stylish(&format!("\u{274c} no stickers in <b>{}</b>", escape_html(&mood))),
            None,
        );
        return;
    }
    match decode_sticker(&ids[0]) {
        Some(s) if send_sticker_ref(bot, m, &s) => {}
        _ => {
            let _ = bot.reply_text(
                m,
                &stylish(&format!(
                    "\u{274c} failed to send a sticker from <b>{}</b>.",
                    escape_html(&mood)
                )),
                None,
            );
        }
    }
}

fn clear_stickers_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !owner_only(m) {
        let _ = bot.reply_text(m, &owner_only_msg(), None);
        return;
    }
    let toks = fields(&args_of(m));
    if toks.is_empty() {
        let _ = bot.reply_text(m, &stylish("\u{1f4ce} usage: /clearstickers <mood|all>"), None);
        return;
    }
    let target = to_lower(&toks[0]);
    let removed = db::clear_sticker_pack(&target);
    if removed == 0 {
        let _ = bot.reply_text(
            m,
            &stylish(&format!("\u{274c} <b>{}</b> not found.", escape_html(&target))),
            None,
        );
        return;
    }
    if target == "all" {
        let _ = bot.reply_text(m, &stylish(&format!("\u{1f5d1}\u{fe0f} removed {removed}")), None);
        return;
    }
    let _ = bot.reply_text(
        m,
        &stylish(&format!("\u{1f5d1}\u{fe0f} removed pack <b>{}</b>", escape_html(&target))),
        None,
    );
}

fn stickerdiag_handler(bot: &mut dyn Bot, m: &TgMessage) {
    if !owner_only(m) {
        let _ = bot.reply_text(m, &owner_only_msg(), None);
        return;
    }
    if m.reply_to == 0 {
        let _ = bot.reply_text(m, &stylish("\u{1f4ce} reply to a sticker to diagnose it."), None);
        return;
    }
    match bot.get_replied_sticker(m.chat_id, m.reply_to) {
        Some(st) => {
            let _ = bot.reply_text(
                m,
                &stylish(&format!(
                    "diag doc={} hash={} ref={} bytes emoji={}",
                    st.doc_id,
                    st.doc_access_hash,
                    st.doc_file_ref.len(),
                    escape_html(&st.emoji)
                )),
                None,
            );
        }
        None => {
            let _ = bot.reply_text(
                m,
                &stylish("\u{274c} replied message is not a sticker (or fetch failed)."),
                None,
            );
        }
    }
}

/// The reply engine hook: classifies inbound sticker moods and answers with a
/// same/related-mood sticker (DM always; group on reply-to-bot or @mention).
fn sticker_message_hook(bot: &mut dyn Bot, m: &TgMessage) {
    sticker_engine(bot, m);
}

pub fn register(r: &mut Registry) {
    r.command("addsticker", add_sticker_handler);
    r.command("stickerdiag", stickerdiag_handler);
    r.command("stickerpacks", sticker_packs_handler);
    r.command("previewsticker", preview_sticker_handler);
    r.command("clearstickers", clear_stickers_handler);
    r.on_message(sticker_message_hook);
}
