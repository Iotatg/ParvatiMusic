//! Bot-mode messaging over the MTProto core
//!
//! Bot login via
//! auth.importBotAuthorization, on_command / on_callback / on_message hooks
//! fed by the update stream, send/edit/delete text with inline keyboards,
//! callback answers and a sender-name cache. Photo/file media goes through
//! the Bot API HTTP channel exactly like the bot_api helper (curl).
//!
//! All constructor ids are layer 227 (docs/tl/api-227.tl); the layouts
//! used here are byte-identical across layers 227..230.
//!
//! Deviation note: clients in this ecosystem commonly negotiate layer 229;
//! this core speaks 227. Both are accepted by the server and every
//! constructor used here is byte-identical in 227..230.

use std::collections::HashMap;
use std::sync::Arc;

use crate::client::{Client, UpdateEvent};
use crate::client::random_bytes;
use crate::tl::{TlReader, TlWriter};

pub mod crc227 {
    pub const BOOL_TRUE: u32 = 0x997275b5;
    pub const BOOL_FALSE: u32 = 0xbc799737;
    pub const PEER_USER: u32 = 0x59511722;
    pub const PEER_CHAT: u32 = 0x36c6019a;
    pub const PEER_CHANNEL: u32 = 0xa2a5371e;
    pub const UPDATES_CONTAINER: u32 = 0x74ae4240;
    pub const UPDATES_COMBINED: u32 = 0x725b04c3;
    pub const UPDATE_SHORT: u32 = 0x78d4dec1;
    pub const UPDATE_SHORT_MESSAGE: u32 = 0x313bc7f8;
    pub const UPDATE_SHORT_CHAT_MESSAGE: u32 = 0x4d6deea5;
    pub const UPDATE_NEW_MESSAGE: u32 = 0x1f2b0afd;
    pub const UPDATE_NEW_CHANNEL_MESSAGE: u32 = 0x62ba04d9;
    pub const UPDATES_TOO_LONG: u32 = 0xe317af7e;
    pub const UPDATE_MESSAGE_ID: u32 = 0x4e90bfd6;
    pub const UPDATE_BOT_CALLBACK: u32 = 0xb9cfc48d;
    pub const MESSAGE: u32 = 0x7600b9d3;
    pub const MESSAGE_EMPTY: u32 = 0x90a6ca84;
    pub const USER: u32 = 0x31774388;
    pub const USER_EMPTY: u32 = 0xd3bc4b7a;
    pub const CHAT: u32 = 0x41cbf256;
    pub const CHAT_EMPTY: u32 = 0x29562865;
    pub const CHANNEL: u32 = 0x1c32b11c;
    pub const CHANNEL_FORBIDDEN: u32 = 0x17d493d5;
    pub const AUTH_IMPORT_BOT: u32 = 0x67a3ff2c;
    pub const AUTH_AUTHORIZATION: u32 = 0x2ea2c0d4;
    pub const INPUT_PEER_SELF: u32 = 0x7da07ec9;
    pub const INPUT_PEER_CHAT: u32 = 0x35a95cb9;
    pub const INPUT_PEER_USER: u32 = 0xdde8a54c;
    pub const INPUT_PEER_CHANNEL: u32 = 0x27bcbbfc;
    pub const INPUT_CHANNEL: u32 = 0xf35aec28;
    pub const INPUT_CHANNEL_EMPTY: u32 = 0xee8c1e86;
    pub const INPUT_REPLY_TO_MESSAGE: u32 = 0x3bd4b7c2;
    pub const MESSAGES_SEND_MESSAGE: u32 = 0xfef48f62;
    pub const MESSAGES_EDIT_MESSAGE: u32 = 0xb106e66c;
    pub const MESSAGES_DELETE_MESSAGES: u32 = 0xe58e95d2;
    pub const MESSAGES_SET_CALLBACK_ANSWER: u32 = 0xd58f130a;
    pub const MESSAGE_ENTITY_BOLD: u32 = 0xbd610bc9;
    pub const MESSAGE_ENTITY_ITALIC: u32 = 0x826f8b60;
    pub const MESSAGE_ENTITY_UNDERLINE: u32 = 0x9c4e7e8b;
    pub const MESSAGE_ENTITY_STRIKETHROUGH: u32 = 0xbf069ec4;
    pub const MESSAGE_ENTITY_CODE: u32 = 0x28a20571;
    pub const MESSAGE_ENTITY_PRE: u32 = 0x73924be0;
    pub const MESSAGE_ENTITY_TEXT_URL: u32 = 0x76a6d327;
    pub const MESSAGE_ENTITY_MENTION_NAME: u32 = 0xdc7b1140;
    pub const MESSAGE_MEDIA_EMPTY: u32 = 0x3ded6320;
    pub const MESSAGE_MEDIA_PHOTO: u32 = 0x695150d7;
    pub const MESSAGE_MEDIA_DOCUMENT: u32 = 0x52d8ccd9;
    pub const DOCUMENT: u32 = 0x8fd4c4d8;
    pub const DOCUMENT_EMPTY: u32 = 0x36f8c871;
    pub const DOCUMENT_ATTRIBUTE_STICKER: u32 = 0x6319d612;
    pub const DOCUMENT_ATTRIBUTE_VIDEO: u32 = 0x43c57c48;
    pub const DOCUMENT_ATTRIBUTE_FILENAME: u32 = 0x15590068;
    pub const MESSAGES_SET_TYPING: u32 = 0x58943ee2;
    pub const SEND_MESSAGE_CHOOSE_STICKER_ACTION: u32 = 0xb05ac6b1;
    pub const MESSAGES_SEND_MEDIA: u32 = 0x330e77f;
    pub const INPUT_MEDIA_DOCUMENT: u32 = 0xa8763ab5;
    pub const INPUT_DOCUMENT: u32 = 0x1abfb575;
    pub const INPUT_STICKER_SET_EMPTY: u32 = 0xffb62b95;
    pub const INPUT_STICKER_SET_ID: u32 = 0x9de7a269;
    pub const INPUT_STICKER_SET_SHORT_NAME: u32 = 0x861cc8a0;
    pub const INPUT_STICKER_SET_ANIMATED_EMOJI: u32 = 0x28703c8;
    pub const PHOTO_SIZE_EMPTY: u32 = 0xe17e23c;
    pub const PHOTO_SIZE: u32 = 0x75c78e60;
    pub const PHOTO_CACHED_SIZE: u32 = 0x21e1ad6;
    pub const PHOTO_STRIPPED_SIZE: u32 = 0xe0b0bc2e;
    pub const PHOTO_SIZE_PROGRESSIVE: u32 = 0xfa3efb95;
    pub const PHOTO_PATH_SIZE: u32 = 0xd8214d41;
    pub const REPLY_INLINE_MARKUP: u32 = 0x48a30254;
    pub const KEYBOARD_BUTTON_ROW: u32 = 0x77608b83;
    pub const KEYBOARD_BUTTON_URL: u32 = 0xd80c25ec;
    pub const KEYBOARD_BUTTON_CALLBACK: u32 = 0xe62bc960;
    pub const USER_PROFILE_PHOTO_EMPTY: u32 = 0x4f11bae1;
    pub const USER_PROFILE_PHOTO: u32 = 0x82d1f706;
    pub const CHAT_PHOTO_EMPTY: u32 = 0x37c1011c;
    pub const CHAT_PHOTO: u32 = 0x1c6e1c11;
    pub const USER_STATUS_EMPTY: u32 = 0x9d05049;
    pub const USER_STATUS_ONLINE: u32 = 0xedb93949;
    pub const USER_STATUS_OFFLINE: u32 = 0x8c703f;
    pub const USER_STATUS_RECENTLY: u32 = 0x7b197dc8;
    pub const USER_STATUS_LAST_WEEK: u32 = 0x541a1d1a;
    pub const USER_STATUS_LAST_MONTH: u32 = 0x65899777;
    pub const EMOJI_STATUS_EMPTY: u32 = 0x2de11aae;
    pub const EMOJI_STATUS: u32 = 0xe7ff068a;
    pub const PEER_COLOR: u32 = 0xb54b5acf;
    pub const RECENT_STORY: u32 = 0x711d692d;
    pub const USERNAME: u32 = 0xb4073647;
}


pub type CommandHandler = Arc<dyn Fn(&mut Client, &TgMessage) + Send + Sync>;
pub type CallbackHandler = Arc<dyn Fn(&mut Client, &TgCallback) + Send + Sync>;
pub type RawMessageHandler = Arc<dyn Fn(&Client, &TgMessage) + Send + Sync>;

/// A Telegram message as seen by command handlers.
#[derive(Clone, Debug, Default)]
pub struct TgMessage {
    pub chat_id: i64,
    pub id: i64,
    pub sender_id: i64,
    pub text: String,
    pub first_name: String,
    pub last_name: String,
    pub is_private: bool,
    pub is_channel: bool,
    pub reply_to: i64,
    /// Kind of media attached (default = no media).
    pub media_kind: MediaKind,
    /// document id / access hash / file reference for media documents
    /// (stickers, video notes, files) — needed to re-send or download.
    pub doc_id: i64,
    pub doc_access_hash: i64,
    pub doc_file_ref: Vec<u8>,
    /// Sticker emoji (e.g. "😀") from documentAttributeSticker, empty otherwise.
    pub sticker_emoji: String,
}

/// Coarse classification of a message's attached media.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MediaKind {
    #[default]
    None,
    Sticker,
    Video,
    Document,
    Photo,
}

/// A callback query as seen by command handlers.
#[derive(Clone, Debug, Default)]
pub struct TgCallback {
    pub chat_id: i64,
    pub message_id: i64,
    pub user_id: i64,
    pub query_id: i64,
    pub data: String,
    pub first_name: String,
    pub last_name: String,
}

/// Inline button (worker code converts to wire markup on send).
#[derive(Clone, Debug)]
pub struct Button {
    pub text: String,
    pub kind: Btn,
    /// 0 = default, 1 = bg_primary, 2 = bg_danger, 3 = bg_success
    /// (keyboardButtonStyle#4fdd3430, attached via button flags.10).
    pub style: u8,
}

#[derive(Clone, Debug)]
pub enum Btn {
    Callback(Vec<u8>),
    Url(String),
}

#[derive(Clone, Debug, Default)]
pub struct Markup {
    pub rows: Vec<Vec<Button>>,
}

impl Markup {
    pub fn row(mut self, buttons: Vec<Button>) -> Markup {
        self.rows.push(buttons);
        self
    }
}

pub(crate) struct MsgState {
    commands: HashMap<String, CommandHandler>,
    callbacks: Vec<(String, CallbackHandler)>,
    message_hooks: Vec<RawMessageHandler>,
    user_names: HashMap<i64, (String, String)>,
    peers: HashMap<i64, PeerInfo>,
    me_id: i64,
    me_username: String,
    me_first_name: String,
    me_last_name: String,
    authorized: bool,
    remap: HashMap<i64, i64>,
    /// updates.getState snapshot for getDifference polling.
    pub(crate) up_pts: i32,
    pub(crate) up_qts: i32,
    pub(crate) up_date: i32,
    pub(crate) up_seq: i32,
}

#[derive(Clone, Copy)]
pub(crate) struct PeerInfo {
    pub kind: u32,
    pub access_hash: i64,
    /// channel.megagroup: supergroups use PEER_CHANNEL too, but they accept
    /// commands like a normal group; broadcast channels do not.
    pub megagroup: bool,
    /// True once we have seen a full channel object (or asked for one), so we
    /// know whether megagroup was actually resolved.
    pub classified: bool,
}

impl MsgState {
    pub(crate) fn new() -> MsgState {
        MsgState {
            commands: HashMap::new(),
            callbacks: Vec::new(),
            message_hooks: Vec::new(),
            user_names: HashMap::new(),
            peers: HashMap::new(),
            me_id: 0,
            me_username: String::new(),
            me_first_name: String::new(),
            me_last_name: String::new(),
            authorized: false,
            remap: HashMap::new(),
            up_pts: 0,
            up_qts: 0,
            up_date: 0,
            up_seq: 0,
        }
    }
}

pub(crate) const REPLY_INLINE_MARKUP: u32 = 0xb2b15770;
pub(crate) const KEYBOARD_INLINE_BUTTON_ROW: u32 = 0x19420af6;
pub(crate) const KEYBOARD_INLINE_BUTTON: u32 = 0x11c1a322;
pub(crate) const INLINE_BUTTON_TYPE_URL: u32 = 0xeca4f8d4;
pub(crate) const INLINE_BUTTON_TYPE_CALLBACK: u32 = 0x2955bc38;

pub(crate) fn write_markup(w: &mut TlWriter, markup: &Markup) {
    // New-schema inline keyboard: replyInlineMarkup -> rows of
    // keyboardInlineButton(text + InlineButtonType object). The old-style
    // keyboardButtonUrl/Callback ctors are rejected by the server now
    // (INPUT_CONSTRUCTOR_INVALID_03 / BUTTON_URL_INVALID).
    w.u32(REPLY_INLINE_MARKUP);
    w.u32(0); // flags
    w.vec_header(markup.rows.len() as u32);
    for row in &markup.rows {
        // URL buttons with an empty url are rejected by the server
        // (BUTTON_URL_INVALID); drop them so a misconfigured env degrades
        // to fewer buttons instead of a failed send.
        let live: Vec<&crate::messaging::Button> =
            row.iter().filter(|b| match &b.kind {
                Btn::Url(u) => !u.is_empty(),
                _ => true,
            }).collect();
        if live.is_empty() {
            continue;
        }
        w.u32(KEYBOARD_INLINE_BUTTON_ROW);
        w.vec_header(live.len() as u32);
        for b in live {
            w.u32(KEYBOARD_INLINE_BUTTON);
            if b.style != 0 {
                // style:flags.10?KeyboardButtonStyle (flag bit 10)
                w.u32(1 << 10);
            } else {
                w.u32(0);
            }
            if b.style != 0 {
                // keyboardButtonStyle#4fdd3430:
                // bg_primary:flags.0 bg_danger:flags.1 bg_success:flags.2
                let style_flags = match b.style {
                    1 => 1u32,
                    2 => 1 << 1,
                    3 => 1 << 2,
                    _ => 0,
                };
                w.u32(0x4fdd3430);
                w.u32(style_flags);
            }
            w.str(&b.text);
            match &b.kind {
                Btn::Callback(data) => {
                    w.u32(INLINE_BUTTON_TYPE_CALLBACK);
                    w.u32(0); // flags: requires_password=false
                    w.blob(data);
                }
                Btn::Url(url) => {
                    w.u32(INLINE_BUTTON_TYPE_URL);
                    w.str(url);
                }
            }
        }
    }
}


// ---------------------------------------------------------------------------
// HTML -> entities (parity with parseTextEntities for the tags the bot
// actually emits: b i u s code pre a)
// ---------------------------------------------------------------------------

fn utf16_len(s: &str) -> usize {
    s.chars().map(|c| c.len_utf16()).sum()
}

pub fn parse_html(text: &str) -> (String, Vec<(u32, i32, i32, String)>) {
    enum Tag {
        Bold,
        Italic,
        Under,
        Strike,
        Code,
        Pre,
        Url,
        Blockquote,
    }

    impl Tag {
        fn same(&self, other: &Tag) -> bool {
            std::mem::discriminant(self) == std::mem::discriminant(other)
        }

        fn crc(&self) -> u32 {
            match self {
                Tag::Bold => crc227::MESSAGE_ENTITY_BOLD,
                Tag::Italic => crc227::MESSAGE_ENTITY_ITALIC,
                Tag::Under => crc227::MESSAGE_ENTITY_UNDERLINE,
                Tag::Strike => crc227::MESSAGE_ENTITY_STRIKETHROUGH,
                Tag::Code => crc227::MESSAGE_ENTITY_CODE,
                Tag::Pre => crc227::MESSAGE_ENTITY_PRE,
                Tag::Url => crc227::MESSAGE_ENTITY_TEXT_URL,
                Tag::Blockquote => 0xf1ccaaac,
            }
        }
    }

    fn tag_for(name: &str) -> Option<Tag> {
        match name {
            "b" | "strong" => Some(Tag::Bold),
            "i" | "em" => Some(Tag::Italic),
            "u" | "ins" => Some(Tag::Under),
            "s" | "strike" | "del" => Some(Tag::Strike),
            "code" => Some(Tag::Code),
            "pre" => Some(Tag::Pre),
            "a" => Some(Tag::Url),
            "blockquote" | "expandable_blockquote" => Some(Tag::Blockquote),
            _ => None,
        }
    }

    let chars: Vec<char> = text.chars().collect();
    let mut plain = String::new();
    let mut entities: Vec<(u32, i32, i32, String)> = Vec::new();
    let mut stack: Vec<(Tag, i32, Option<String>)> = Vec::new();
    let mut i = 0usize;

    while i < chars.len() {
        if chars[i] == '<' {
            let (tag_txt, next) = scan_tag(&chars, i);
            let Some(tag_txt) = tag_txt else {
                i += 1;
                continue;
            };
            i = next;
            let is_close = tag_txt.starts_with('/');
            let body = if is_close { &tag_txt[1..] } else { &tag_txt[..] };
            let name: String = body.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
            let Some(tag) = tag_for(&name.to_lowercase()) else {
                continue;
            };
            let pos16 = utf16_len(&plain) as i32;
            if is_close {
                if let Some(k) = stack.iter().rposition(|(t, _, _)| t.same(&tag)) {
                    let (t, open, href) = stack.remove(k);
                    let len = pos16 - open;
                    if len > 0 {
                        let raw = href.unwrap_or_default();
                        match mention_user_id(&raw) {
                            Some(uid) => {
                                entities.push((
                                    crc227::MESSAGE_ENTITY_MENTION_NAME,
                                    open,
                                    len,
                                    uid.to_string(),
                                ));
                            }
                            None => entities.push((t.crc(), open, len, raw)),
                        }
                    }
                }
            } else {
                let href = if name.eq_ignore_ascii_case("a") { extract_href(body) } else { None };
                stack.push((tag, pos16, href));
            }
            continue;
        }
        if chars[i] == '&' {
            if let Some((rep, skip)) = decode_entity(&chars[i..]) {
                plain.push(rep);
                i += skip;
                continue;
            }
        }
        plain.push(chars[i]);
        i += 1;
    }
    (plain, entities)
}


/// Scans a tag starting at chars[start] == '<'; returns (inner, index-after-'>').
fn scan_tag(chars: &[char], start: usize) -> (Option<String>, usize) {
    let mut j = start + 1;
    let mut in_quote = false;
    while j < chars.len() {
        let c = chars[j];
        if c == '"' {
            in_quote = !in_quote;
        } else if c == '>' && !in_quote {
            let inner: String = chars[start + 1..j].iter().collect();
            return (Some(inner), j + 1);
        }
        j += 1;
    }
    (None, start + 1)
}

/// Parses a `tg://user?id=N` href into the numeric user id so it can be sent
/// as messageEntityMentionName instead of a plain textUrl. Anything else
/// (http/https links) returns None.
fn mention_user_id(href: &str) -> Option<i64> {
    let rest = href.strip_prefix("tg://user?id=")?;
    rest.split('&').next()?.parse::<i64>().ok()
}

fn extract_href(body: &str) -> Option<String> {
    let lower = body.to_lowercase();
    let k = lower.find("href=")?;
    let after = &body[k + 5..];
    let (quote, rest) = match after.chars().next()? {
        q @ ('"' | '\'') => (q, &after[1..]),
        _ => ('"', after),
    };
    let endq = rest.find(quote)?;
    Some(rest[..endq].to_string())
}

/// Returns (replacement char, chars consumed) for &lt; &gt; &amp; &quot;.
fn decode_entity(chars: &[char]) -> Option<(char, usize)> {
    let s: String = chars.iter().take(7).collect();
    for (ent, rep, skip) in [
        ("&lt;", '<', 4),
        ("&gt;", '>', 4),
        ("&amp;", '&', 5),
        ("&quot;", '"', 6),
        ("&nbsp;", '\u{00A0}', 6),
    ] {
        if s.starts_with(ent) {
            return Some((rep, skip));
        }
    }
    None
}



// ---------------------------------------------------------------------------
// Typed TL parsing (reader-consumes-fields style)
// ---------------------------------------------------------------------------

fn parse_peer_in(r: &mut TlReader) -> Option<(u32, i64)> {
    let crc = r.u32();
    match crc {
        crc227::PEER_USER => Some((crc227::PEER_USER, r.i64())),
        crc227::PEER_CHAT => Some((crc227::PEER_CHAT, r.i64())),
        crc227::PEER_CHANNEL => Some((crc227::PEER_CHANNEL, r.i64())),
        _ => None,
    }
}

#[derive(Default)]
struct MediaInfo {
    kind: MediaKind,
    doc_id: i64,
    doc_access_hash: i64,
    doc_file_ref: Vec<u8>,
    sticker_emoji: String,
}

/// Consumes a MessageMedia, extracting document identity + kind.
fn parse_media_in(r: &mut TlReader) -> Option<MediaInfo> {
    let crc = r.u32();
    match crc {
        crc227::MESSAGE_MEDIA_EMPTY => Some(MediaInfo::default()),
        crc227::MESSAGE_MEDIA_DOCUMENT => {
            let flags = r.u32();
            let mut info = MediaInfo::default();
            if flags & (1 << 0) != 0 {
                info = parse_document_in(r)?;
            }
            if flags & (1 << 1) != 0 && !skip_vector_of(r, skip_photo_entity) {
                return None;
            }
            if flags & (1 << 2) != 0 {
                let _ttl = r.i32();
            }
            Some(info)
        }
        crc227::MESSAGE_MEDIA_PHOTO => {
            let flags = r.u32();
            let info = MediaInfo { kind: MediaKind::Photo, ..Default::default() };
            if flags & (1 << 0) != 0 {
                let _ = skip_photo_entity(r);
            }
            if flags & (1 << 2) != 0 {
                let _ttl = r.i32();
            }
            Some(info)
        }
        _ => None,
    }
}

/// Consumes a Document, reading id/access_hash/file_reference and scanning its
/// attributes for sticker emoji / video / filename classification.
fn parse_document_in(r: &mut TlReader) -> Option<MediaInfo> {
    let crc = r.u32();
    if crc == crc227::DOCUMENT_EMPTY {
        return Some(MediaInfo::default());
    }
    if crc != crc227::DOCUMENT {
        return None;
    }
    let flags = r.u32();
    let id = r.i64();
    let access_hash = r.i64();
    let file_ref = r.bytes().to_vec();
    let _date = r.i32();
    let _mime = r.str();
    let _size = r.i64();
    if flags & (1 << 0) != 0 && !skip_vector_of(r, skip_photo_size) {
        return None;
    }
    let mut info = MediaInfo {
        kind: MediaKind::Document,
        doc_id: id,
        doc_access_hash: access_hash,
        doc_file_ref: file_ref,
        sticker_emoji: String::new(),
    };
    let _vec = r.u32();
    let n = r.vec_count();
    if !(0..=256).contains(&n) {
        return None;
    }
    for _ in 0..n {
        let acrc = r.u32();
        match acrc {
            crc227::DOCUMENT_ATTRIBUTE_STICKER => {
                let afl = r.u32();
                info.sticker_emoji = String::from_utf8_lossy(r.str()).into_owned();
                if !skip_input_sticker_set(r) {
                    return None;
                }
                if afl & (1 << 1) != 0 {
                    return None; // mask_coords: not typed
                }
                info.kind = MediaKind::Sticker;
            }
            crc227::DOCUMENT_ATTRIBUTE_VIDEO => {
                let afl = r.u32();
                let _duration = r.f64();
                let _w = r.i32();
                let _h = r.i32();
                if afl & (1 << 1) != 0 {
                    let _ = r.str();
                }
                info.kind = MediaKind::Video;
            }
            crc227::DOCUMENT_ATTRIBUTE_FILENAME => {
                let _ = r.str();
            }
            _ => return None,
        }
    }
    Some(info)
}

/// Consumes an InputStickerSet of any variant (we only need to skip it).
fn skip_input_sticker_set(r: &mut TlReader) -> bool {
    let crc = r.u32();
    match crc {
        crc227::INPUT_STICKER_SET_EMPTY => true,
        crc227::INPUT_STICKER_SET_ID => {
            let _ = r.i64();
            let _ = r.i64();
            r.ok()
        }
        crc227::INPUT_STICKER_SET_SHORT_NAME => {
            let _ = r.str();
            r.ok()
        }
        crc227::INPUT_STICKER_SET_ANIMATED_EMOJI => true,
        _ => false,
    }
}

/// Reads a TL vector and consumes each element with `elem`; returns false on a
/// malformed header or element.
fn skip_vector_of(r: &mut TlReader, elem: fn(&mut TlReader) -> bool) -> bool {
    let _vec = r.u32();
    let n = r.vec_count();
    if !(0..=1000).contains(&n) {
        return false;
    }
    for _ in 0..n {
        if !elem(r) {
            return false;
        }
    }
    r.ok()
}

fn parse_reply_header_in(r: &mut TlReader) -> Option<i64> {
    let crc = r.u32();
    if crc != 0x1b97dd66 {
        return None;
    }
    let flags = r.u32();
    let mut reply_to = 0i64;
    if flags & (1 << 4) != 0 {
        reply_to = r.i32() as i64;
    }
    if flags & (1 << 0) != 0 && parse_peer_in(r).is_none() {
        return None;
    }
    if flags & (1 << 5) != 0 {
        return None; // reply_from: MessageFwdHeader not typed here
    }
    if flags & (1 << 8) != 0 {
        return None; // reply_media
    }
    if flags & (1 << 1) != 0 {
        let _top = r.i32();
    }
    if flags & (1 << 6) != 0 {
        let _qt = r.str();
    }
    if flags & (1 << 7) != 0 {
        return None; // quote_entities vector
    }
    if flags & (1 << 10) != 0 {
        let _off = r.i32();
    }
    if flags & (1 << 11) != 0 {
        let _ = r.i32();
    }
    if flags & (1 << 12) != 0 {
        let _ = r.bytes();
    }
    r.ok().then_some(reply_to)
}

/// Consumes a message object from the reader (ctor already consumed OR read
/// here). Returns None for anything this bot does not need to read as text.
fn parse_message_in(r: &mut TlReader) -> Option<TgMessage> {
    let crc = r.u32();
    if crc == crc227::MESSAGE_EMPTY {
        let id = r.i32() as i64;
        let _peer = if r.peek_u32() == crc227::PEER_USER
            || r.peek_u32() == crc227::PEER_CHAT
            || r.peek_u32() == crc227::PEER_CHANNEL
        {
            parse_peer_in(r)
        } else {
            None
        };
        return Some(TgMessage { id, ..Default::default() });
    }
    if crc != crc227::MESSAGE {
        return None;
    }
    // message#7600b9d3 flags flags2 id:int from_id:flags.8?Peer
    // from_boosts:flags.29?int from_rank:flags2.12?string peer_id:Peer
    // saved_peer_id:flags.28?Peer fwd_from:flags.2? via_bot_id:flags.11?long
    // via_business_bot_id:flags2.0?long guestchat_via_from:flags2.19?Peer
    // reply_to:flags.3?MessageReplyHeader date:int message:string media:flags.9?
    // reply_markup:flags.6? entities:flags.7? views/forwards:flags.10?int
    // replies:flags.23? edit_date:flags.15? post_author:flags.16? grouped_id:flags.17?
    // reactions:flags.20? restriction:flags.22? ttl:flags.25? quick:flags.30?
    // effect:flags2.2? factcheck:flags2.3? report:flags2.5? paid_stars:flags2.6?
    // suggested:flags2.7? repeat:flags2.10? lang:flags2.11? rich:flags2.13?
    let flags = r.u32();
    let flags2 = r.u32();
    let _abort = flags & (1 << 2) != 0      // fwd_from
        || flags & (1 << 6) != 0            // reply_markup
        || flags & (1 << 23) != 0           // replies
        || flags & (1 << 20) != 0           // reactions
        || flags & (1 << 22) != 0           // restriction_reason
        || flags & (1 << 30) != 0           // quick_reply
        || flags2 & (1 << 3) != 0           // factcheck
        || flags2 & (1 << 5) != 0           // report_delivery_until_date
        || flags2 & (1 << 7) != 0           // suggested_post
        || flags2 & (1 << 11) != 0          // summary_from_language
        || flags2 & (1 << 13) != 0;         // rich_message
    let id = r.i32() as i64;
    let from = if flags & (1 << 8) != 0 {
        parse_peer_in(r)?
    } else {
        (0, 0)
    };
    if flags & (1 << 29) != 0 {
        let _ = r.i32();
    }
    if flags2 & (1 << 12) != 0 {
        let _ = r.str();
    }
    let peer = parse_peer_in(r)?;
    if flags & (1 << 28) != 0 {
        parse_peer_in(r)?;
    }
    if flags & (1 << 11) != 0 {
        let _ = r.i64();
    }
    if flags2 & (1 << 0) != 0 {
        let _ = r.i64();
    }
    if flags2 & (1 << 19) != 0 {
        parse_peer_in(r)?;
    }
    let mut reply_to = 0i64;
    if flags & (1 << 3) != 0 {
        reply_to = parse_reply_header_in(r)?;
    }
    let _date = r.i32();
    let text = String::from_utf8_lossy(r.str()).into_owned();
    let media = if flags & (1 << 9) != 0 {
        parse_media_in(r).unwrap_or_default()
    } else {
        MediaInfo::default()
    };
    if flags & (1 << 10) != 0 {
        let _ = r.i32();
    }
    if flags & (1 << 15) != 0 {
        let _ = r.i32();
    }
    if flags & (1 << 16) != 0 {
        let _ = r.str();
    }
    if flags & (1 << 17) != 0 {
        let _ = r.i64();
    }
    if flags & (1 << 25) != 0 {
        let _ = r.i32();
    }
    if flags & (1 << 30) != 0 {
        let _ = r.i32();
    }
    if flags2 & (1 << 2) != 0 {
        let _ = r.i64();
    }
    if flags2 & (1 << 6) != 0 {
        let _ = r.i64();
    }
    if flags2 & (1 << 10) != 0 {
        let _ = r.i32();
    }
    // entities:flags.7 — plain keyboards send these constantly (empty or
    // trivial markup around command text), so they must be consumed, not
    // treated as unsupported.
    if flags & (1 << 7) != 0 && !skip_entities(r) {
        return None;
    }

    if _abort {
        return None;
    }

    let (chat_id, is_private, is_channel) = match peer {
        (crc227::PEER_USER, id) => (id, true, false),
        (crc227::PEER_CHAT, id) => (id, false, false),
        (crc227::PEER_CHANNEL, id) => (id, false, true),
        _ => return None,
    };
    // Incoming DMs carry from_id only under flags.8; for user-to-user
    // messages the peer_user IS the sender, so backfill from the peer.
    // (Outgoing echoes keep flags.out set and are dropped by the self-skip.)
    let sender_id = if from.1 != 0 { from.1 } else { chat_id };
    Some(TgMessage {
        chat_id,
        id,
        sender_id,
        text,
        is_private,
        is_channel,
        reply_to,
        media_kind: media.kind,
        doc_id: media.doc_id,
        doc_access_hash: media.doc_access_hash,
        doc_file_ref: media.doc_file_ref,
        sticker_emoji: media.sticker_emoji,
        ..Default::default()
    })
}

// ---------------------------------------------------------------------------
// users / chats / channels: fill sender-name + peer caches
// ---------------------------------------------------------------------------

/// Parses the first `message` object in a messages.Messages response body
/// (messages.messages#1d73e7ea / messages.channelMessages#c776ba4e).
fn parse_first_message(body: &[u8]) -> Option<TgMessage> {
    if body.len() < 4 {
        return None;
    }
    let needle = crc227::MESSAGE.to_le_bytes();
    let at = body.windows(4).position(|w| w == needle)?;
    let mut r = TlReader::new(&body[at..]);
    parse_message_in(&mut r)
}

/// Sender user id of the first message in a messages.Messages response body
/// (messages.messages#1d73e7ea / messages.channelMessages#c776ba4e). Scans for
/// the first `message` ctor and reads its header; the sender peer precedes any
/// media payload, so this never misreads media bytes.
/// Consumes one MessageEntity. Most entities are (offset, length) pairs;
/// only a few carry extra fields, listed in the match below.
fn skip_entity(r: &mut TlReader) -> bool {
    let crc = r.u32();
    let _ = r.i32(); // offset
    let _ = r.i32(); // length
    match crc {
        0x73924be0 => { // messageEntityPre: ..., lang:string
            let _ = r.str();
        }
        0x76a6d327 => { // messageEntityTextUrl: ..., url:string
            let _ = r.str();
        }
        0xdc7b1140 => { // messageEntityMentionName: ..., user_id:int
            let _ = r.i32();
        }
        0x7c24fdb0 => { // messageEntityCustomEmoji: ..., document_id:long
            let _ = r.i64();
        }
        // (offset,length)-only ctors: unknown, mention, hashtag, botCommand,
        // url, email, bold, italic, underline, strike, code, phone, cashtag,
        // spoiler, blockquote (old) and friends.
        0xbb92ba95 | 0xfa04579d | 0x6f635b9d | 0x6cef8ac7 | 0x6ed02538
        | 0x64e475c2 | 0xbd610bc9 | 0x826f8b60 | 0xc12622c9 | 0xbf0693ed
        | 0x28a21771 | 0x3b4b1cf9 | 0x4c4e743f | 0x31b3d366 => {}
        // new blockquote#f1ccaaac: flags before offset/length — one extra
        // u32 must be consumed to stay aligned (offset/length already read).
        0xf1ccaaac => {
            let _ = r.i32();
        }
        _ => return false,
    }
    r.ok()
}

fn skip_entities(r: &mut TlReader) -> bool {
    let _vec = r.u32();
    let n = r.vec_count();
    if !(0..=1000).contains(&n) {
        return false;
    }
    for _ in 0..n {
        if !skip_entity(r) {
            return false;
        }
    }
    r.ok()
}

fn skip_photo(r: &mut TlReader) -> bool {
    let crc = r.u32();
    match crc {
        crc227::USER_PROFILE_PHOTO_EMPTY | crc227::CHAT_PHOTO_EMPTY => true,
        crc227::USER_PROFILE_PHOTO | crc227::CHAT_PHOTO => {
            let _flags = r.u32();
            let _photo_id = r.i64();
            if _flags & (1 << 1) != 0 {
                let _stripped = r.bytes();
            }
            let _dc = r.i32();
            r.ok()
        }
        _ => false,
    }
}

/// Consumes one PhotoSize element (thumbs / photo sizes vectors).
fn skip_photo_size(r: &mut TlReader) -> bool {
    let crc = r.u32();
    match crc {
        crc227::PHOTO_SIZE_EMPTY => {
            let _ = r.str();
            r.ok()
        }
        crc227::PHOTO_SIZE => {
            let _ = r.str();
            let _ = r.i32();
            let _ = r.i32();
            let _ = r.i32();
            r.ok()
        }
        crc227::PHOTO_CACHED_SIZE => {
            let _ = r.str();
            let _ = r.i32();
            let _ = r.i32();
            let _ = r.bytes();
            r.ok()
        }
        crc227::PHOTO_STRIPPED_SIZE => {
            let _ = r.str();
            let _ = r.bytes();
            r.ok()
        }
        crc227::PHOTO_SIZE_PROGRESSIVE => {
            let _ = r.str();
            let _ = r.i32();
            let _ = r.i32();
            if !skip_vector_i32(r) {
                return false;
            }
            r.ok()
        }
        crc227::PHOTO_PATH_SIZE => {
            let _ = r.str();
            let _ = r.bytes();
            r.ok()
        }
        _ => false,
    }
}

fn skip_vector_i32(r: &mut TlReader) -> bool {
    let _vec = r.u32();
    let n = r.vec_count();
    if !(0..=1000).contains(&n) {
        return false;
    }
    for _ in 0..n {
        let _ = r.i32();
    }
    r.ok()
}

/// Consumes a `Photo` (not UserProfilePhoto): photoEmpty / photo.
fn skip_photo_entity(r: &mut TlReader) -> bool {
    let crc = r.u32();
    match crc {
        0x2331b22d => {
            let _ = r.i64();
            r.ok()
        }
        0xfb197a65 => {
            let flags = r.u32();
            let _id = r.i64();
            let _access_hash = r.i64();
            let _file_ref = r.bytes();
            let _date = r.i32();
            if !skip_vector_of(r, skip_photo_size) {
                return false;
            }
            if flags & (1 << 1) != 0 {
                return false; // video_sizes: Vector<VideoSize> not typed
            }
            let _dc = r.i32();
            r.ok()
        }
        _ => false,
    }
}

fn skip_user_status(r: &mut TlReader) -> bool {
    let crc = r.u32();
    match crc {
        crc227::USER_STATUS_EMPTY | crc227::USER_STATUS_RECENTLY
        | crc227::USER_STATUS_LAST_WEEK | crc227::USER_STATUS_LAST_MONTH => true,
        crc227::USER_STATUS_ONLINE | crc227::USER_STATUS_OFFLINE => {
            let _ = r.i32();
            true
        }
        _ => false,
    }
}

fn skip_emoji_status(r: &mut TlReader) -> bool {
    match r.u32() {
        crc227::EMOJI_STATUS_EMPTY => true,
        crc227::EMOJI_STATUS => {
            let f = r.u32();
            let _ = r.i64();
            if f & 1 != 0 {
                let _ = r.i32();
            }
            r.ok()
        }
        _ => false,
    }
}

fn skip_peer_color(r: &mut TlReader) -> bool {
    match r.u32() {
        crc227::PEER_COLOR => {
            let f = r.u32();
            if f & 1 != 0 {
                let _ = r.i32();
            }
            if f & (1 << 1) != 0 {
                let _ = r.i64();
            }
            r.ok()
        }
        _ => false,
    }
}

fn skip_recent_story(r: &mut TlReader) -> bool {
    match r.u32() {
        crc227::RECENT_STORY => {
            let f = r.u32();
            if f & (1 << 1) != 0 {
                let _ = r.i32();
            }
            r.ok()
        }
        _ => false,
    }
}

fn skip_usernames(r: &mut TlReader) -> bool {
    let n = r.vec_count();
    for _ in 0..n {
        match r.u32() {
            crc227::USERNAME => {
                let _f = r.u32();
                let _u = r.str();
            }
            _ => return false,
        }
    }
    r.ok()
}

fn skip_restriction_reasons(r: &mut TlReader) -> bool {
    let n = r.vec_count();
    for _ in 0..n {
        // restrictionReason#d072acb4 platform:string reason:string text:string
        if r.u32() != 0xd072acb4 {
            return false;
        }
        let _p = r.str();
        let _r = r.str();
        let _t = r.str();
    }
    r.ok()
}

// ---------------------------------------------------------------------------

impl Client {
    /// Consumes one User object at the reader position. Returns false when
    /// the type is unknown (cache fill resync). Fills name + peer caches.
    pub(crate) fn msg_user_in(&mut self, r: &mut TlReader) -> bool {
        let crc = r.peek_u32();
        match crc {
            crc227::USER_EMPTY => {
                r.u32();
                let _ = r.i64();
                r.ok()
            }
            crc227::USER => {
                r.u32();
                // user#31774388 flags flags2 id access_hash:0 first:1 last:2
                // username:3 phone:4 photo:5 status:6 bot_info_version:14
                // restriction:18 inline_placeholder:19 lang:22 emoji:30
                // usernames:f2.0 stories_max:f2.5 color:f2.8 profile:f2.9
                // active_users:f2.12 verify_icon:f2.14 paid_stars:f2.15
                let flags = r.u32();
                let flags2 = r.u32();
                let id = r.i64();
                let mut ah = 0i64;
                if flags & 1 != 0 {
                    ah = r.i64();
                }
                let first = if flags & (1 << 1) != 0 { String::from_utf8_lossy(r.str()).into_owned() } else { String::new() };
                let last = if flags & (1 << 2) != 0 { String::from_utf8_lossy(r.str()).into_owned() } else { String::new() };
                let username = if flags & (1 << 3) != 0 { String::from_utf8_lossy(r.str()).into_owned() } else { String::new() };
                if flags & (1 << 4) != 0 {
                    let _ = r.str();
                }
                if flags & (1 << 5) != 0 && !skip_photo(r) {
                    return false;
                }
                if flags & (1 << 6) != 0 && !skip_user_status(r) {
                    return false;
                }
                if flags & (1 << 14) != 0 {
                    let _ = r.i32();
                }
                if flags & (1 << 18) != 0 && !skip_restriction_reasons(r) {
                    return false;
                }
                if flags & (1 << 19) != 0 {
                    let _ = r.str();
                }
                if flags & (1 << 22) != 0 {
                    let _ = r.str();
                }
                if flags & (1 << 30) != 0 && !skip_emoji_status(r) {
                    return false;
                }
                if flags2 & 1 != 0 && !skip_usernames(r) {
                    return false;
                }
                if flags2 & (1 << 5) != 0 && !skip_recent_story(r) {
                    return false;
                }
                if flags2 & (1 << 8) != 0 && !skip_peer_color(r) {
                    return false;
                }
                if flags2 & (1 << 9) != 0 && !skip_peer_color(r) {
                    return false;
                }
                if flags2 & (1 << 12) != 0 {
                    let _ = r.i32();
                }
                if flags2 & (1 << 14) != 0 {
                    let _ = r.i64();
                }
                if flags2 & (1 << 15) != 0 {
                    let _ = r.i64();
                }
                if !r.ok() {
                    return false;
                }
                if self.msg.me_id == 0 || self.msg.me_id == id {
                    self.msg.me_id = id;
                    self.msg.me_first_name = first.clone();
                    self.msg.me_last_name = last.clone();
                    self.msg.me_username = username.clone();
                }
                self.msg.user_names.entry(id).or_insert((first, last));
                self.msg.peers.insert(id, PeerInfo { kind: crc227::INPUT_PEER_USER, access_hash: ah, megagroup: false, classified: true });
                true
            }
            _ => false,
        }
    }

    /// Consumes one Chat/Channel object; fills the peer cache.
    pub(crate) fn msg_chat_in(&mut self, r: &mut TlReader) -> bool {
        let crc = r.peek_u32();
        match crc {
            crc227::CHAT_EMPTY => {
                r.u32();
                let _ = r.i64();
                r.ok()
            }
            crc227::CHAT => {
                r.u32();
                // chat#41cbf256 flags id title photo participants_count date
                // version migrated_to:6 admin_rights:14 default_banned:18
                let flags = r.u32();
                let id = r.i64();
                let _title = r.str();
                if !skip_photo(r) {
                    return false;
                }
                let _participants = r.i32();
                let _date = r.i32();
                let _version = r.i32();
                if flags & (1 << 6) != 0 {
                    match r.u32() {
                        0xee8c1e86 => {}
                        crc227::INPUT_CHANNEL => {
                            let _ = r.i64();
                            let _ = r.i64();
                        }
                        _ => return false,
                    }
                }
                if flags & (1 << 14) != 0 || flags & (1 << 18) != 0 {
                    // ChatAdminRights / ChatBannedRights: typed but bulky; skip
                    // is not critical for messaging, abort resync instead.
                    return false;
                }
                if !r.ok() {
                    return false;
                }
                self.msg.peers.insert(id, PeerInfo { kind: crc227::INPUT_PEER_CHAT, access_hash: 0, megagroup: false, classified: true });
                true
            }
            crc227::CHANNEL => {
                r.u32();
                // channel#1c32b11c flags flags2 id access_hash:13 title
                // username:6 photo date restriction:9 admin:14 banned:15
                // default_banned:18 participants:17 usernames:f2.0
                // stories_max:f2.4 color:f2.7 profile:f2.8 emoji:f2.9
                // level:f2.10 sub_until:f2.11 verify:f2.13 paid:f2.14 mono:f2.18
                let flags = r.u32();
                let flags2 = r.u32();
                let id = r.i64();
                let mut ah = 0i64;
                if flags & (1 << 13) != 0 {
                    ah = r.i64();
                }
                let _title = r.str();
                if flags & (1 << 6) != 0 {
                    let _ = r.str();
                }
                if !skip_photo(r) {
                    return false;
                }
                let _date = r.i32();
                if flags & (1 << 9) != 0 && !skip_restriction_reasons(r) {
                    return false;
                }
                if flags & ((1 << 14) | (1 << 15) | (1 << 18)) != 0 {
                    return false; // admin/banned rights objects: resync
                }
                if flags & (1 << 17) != 0 {
                    let _ = r.i32();
                }
                if flags2 & 1 != 0 && !skip_usernames(r) {
                    return false;
                }
                if flags2 & (1 << 4) != 0 && !skip_recent_story(r) {
                    return false;
                }
                if flags2 & (1 << 7) != 0 && !skip_peer_color(r) {
                    return false;
                }
                if flags2 & (1 << 8) != 0 && !skip_peer_color(r) {
                    return false;
                }
                if flags2 & (1 << 9) != 0 && !skip_emoji_status(r) {
                    return false;
                }
                if flags2 & (1 << 10) != 0 {
                    let _ = r.i32();
                }
                if flags2 & (1 << 11) != 0 {
                    let _ = r.i32();
                }
                if flags2 & (1 << 13) != 0 {
                    let _ = r.i64();
                }
                if flags2 & (1 << 14) != 0 {
                    let _ = r.i64();
                }
                if flags2 & (1 << 18) != 0 {
                    let _ = r.i64();
                }
                if !r.ok() {
                    return false;
                }
                self.msg.peers.insert(id, PeerInfo {
                    kind: crc227::INPUT_PEER_CHANNEL,
                    access_hash: ah,
                    megagroup: flags & (1 << 8) != 0,
                    classified: true,
                });
                true
            }
            crc227::CHANNEL_FORBIDDEN => {
                r.u32();
                let _flags = r.u32();
                let _id = r.i64();
                let _ah = r.i64();
                let _title = r.str();
                if _flags & (1 << 16) != 0 {
                    let _ = r.i32();
                }
                r.ok()
            }
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Dispatch + private/short message shapes
// ---------------------------------------------------------------------------

impl Client {
    /// Resolve whether a cached PEER_CHANNEL is a supergroup (megagroup) by
    /// querying channels.getChannels once, then cache the classification so
    /// later messages do not repeat the round-trip.
    fn classify_channel(&mut self, chat_id: i64) {
        let Some(p) = self.msg.peers.get(&chat_id) else {
            return;
        };
        let access_hash = p.access_hash;
        let mut w = crate::tl::TlWriter::new();
        w.u32(0x0a7f6bbb); // channels.getChannels
        w.vec_header(1);
        w.u32(0xf35aec28); // inputChannel
        w.i64(chat_id);
        w.i64(access_hash);
        let megagroup = match self.call_rpc(w.out(), true, false) {
            Ok(resp) => crate::rpc::parse_channel_megagroup(&resp, chat_id),
            Err(_) => None,
        };
        if let Some(mg) = megagroup {
            if let Some(e) = self.msg.peers.get_mut(&chat_id) {
                e.megagroup = mg;
                e.classified = true;
            }
        }
    }

    pub(crate) fn msg_dispatch_message(&mut self, mut m: TgMessage) {
        // Skip our own outgoing messages echoed back through the poll loop,
        // otherwise the bot would react to its own replies.
        if self.msg.me_id != 0 && m.sender_id == self.msg.me_id {
            return;
        }
        // Supergroups arrive as peerChannel; use the cached channel object's
        // megagroup flag so group commands are not swallowed by the
        // broadcast-channel guard (`if m.is_channel { return; }`).
        if m.is_channel {
            let unclassified = match self.msg.peers.get(&m.chat_id) {
                Some(p) => {
                    if p.megagroup {
                        m.is_channel = false;
                    }
                    !p.classified
                }
                None => true,
            };
            if unclassified {
                self.classify_channel(m.chat_id);
                if self.msg.peers.get(&m.chat_id).map(|p| p.megagroup).unwrap_or(false) {
                    m.is_channel = false;
                }
            }
        }
        if m.sender_id == 0 && m.is_private {
            m.sender_id = m.chat_id;
        }
        if m.sender_id != 0 {
            if let Some((f, l)) = self.msg.user_names.get(&m.sender_id).cloned() {
                m.first_name = f;
                m.last_name = l;
            }
        }
        let hooks: Vec<RawMessageHandler> = self.msg.message_hooks.clone();
        for h in hooks {
            h(self, &m);
        }
 // Command routing: "/cmd@bot args...".
        let text = m.text.trim().to_string();
        if let Some(stripped) = text.strip_prefix('/') {
            let cmd: String = stripped
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect::<String>()
                .to_lowercase();
            let cmd = cmd.split('@').next().unwrap_or("").to_string();
            if let Some(h) = self.msg.commands.get(&cmd).cloned() {
                h(self, &m);
            }
        }
    }

    pub(crate) fn msg_handle_callback(&mut self, r: &mut TlReader) {
        // updateBotCallbackQuery#b9cfc48d flags query_id:long user_id:long
        // peer:Peer msg_id:int chat_instance:long data:flags.0?bytes game:flags.1?string
        let _flags = r.u32();
        let query_id = r.i64();
        let user_id = r.i64();
        let peer = match parse_peer_in(r) {
            Some(p) => p,
            None => return,
        };
        let msg_id = r.i32() as i64;
        let _chat_instance = r.i64();
        let raw_data = if _flags & 1 != 0 {
            String::from_utf8_lossy(r.bytes()).into_owned()
        } else {
            String::new()
        };
        // Inline keyboards built by the bot encode the callback payload as
        // base64 (see `data_btn`), so the server echoes it encoded. Decode it
        // here so every registered prefix handler matches plain data, exactly
        // like the reference bots.
        let data = match base64_decode_str(&raw_data) {
            Some(decoded) => String::from_utf8_lossy(&decoded).into_owned(),
            None => raw_data,
        };
        if _flags & (1 << 1) != 0 {
            let _ = r.str();
        }
        let chat_id: i64 = peer.1;
        let (first, last) = self.msg.user_names.get(&user_id).cloned().unwrap_or_default();
        let cb = TgCallback {
            chat_id,
            message_id: msg_id,
            user_id,
            query_id,
            data,
            first_name: first,
            last_name: last,
        };
        let hooks: Vec<(String, CallbackHandler)> = self.msg.callbacks.clone();
        for (prefix, h) in hooks {
            if cb.data.starts_with(&prefix) {
                h(self, &cb);
                return;
            }
        }
    }

    pub(crate) fn emit_update(&self, ev: &UpdateEvent) {
        if let Some(handler) = &self.update_handler {
            handler(ev);
        }
    }
}

impl Client {
    /// Registers a `/name` command handler (name matched case-insensitively,
 /// `@bot` suffix stripped, like the dispatcher).
    pub fn on_command<F>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut Client, &TgMessage) + Send + Sync + 'static,
    {
        self.msg.commands.insert(name.to_lowercase(), Arc::new(f));
    }

    /// Registers a callback-data prefix handler.
    pub fn on_callback<F>(&mut self, prefix: &str, f: F)
    where
        F: Fn(&mut Client, &TgCallback) + Send + Sync + 'static,
    {
        self.msg.callbacks.push((prefix.to_string(), Arc::new(f)));
    }

    /// Registers a raw message hook fired for every parsed message.
    pub fn on_message<F>(&mut self, f: F)
    where
        F: Fn(&Client, &TgMessage) + Send + Sync + 'static,
    {
        self.msg.message_hooks.push(Arc::new(f));
    }
}

fn msg_from_short_private(r: &mut TlReader) -> Option<TgMessage> {
    // updateShortMessage#313bc7f8 flags out:1 mentioned:4 media_unread:5
    // silent:13 id:int user_id:long message:string pts pts_count date
    // fwd:2 via:11 reply_to:3 entities:7 ttl:25
    let flags = r.u32();
    let id = r.i32() as i64;
    let user_id = r.i64();
    let text = String::from_utf8_lossy(r.str()).into_owned();
    let _pts = r.i32();
    let _cnt = r.i32();
    let _date = r.i32();
    if flags & (1 << 2) != 0 {
        return None; // fwd_from
    }
    let mut reply_to = 0i64;
    if flags & (1 << 3) != 0 {
        reply_to = parse_reply_header_in(r)?;
    }
    if flags & (1 << 7) != 0 {
        return None; // entities
    }
    Some(TgMessage { chat_id: user_id, id, sender_id: user_id, text, is_private: true, reply_to, ..Default::default() })
}

fn msg_from_short_chat(r: &mut TlReader) -> Option<TgMessage> {
    // updateShortChatMessage#4d6deea5 flags id from_id long chat_id long
    // message string pts pts_count date ...
    let flags = r.u32();
    let id = r.i32() as i64;
    let from_id = r.i64();
    let chat_id = r.i64();
    let text = String::from_utf8_lossy(r.str()).into_owned();
    let _pts = r.i32();
    let _cnt = r.i32();
    let _date = r.i32();
    let mut reply_to = 0i64;
    if flags & (1 << 3) != 0 {
        reply_to = parse_reply_header_in(r)?;
    }
    if flags & (1 << 2) != 0 || flags & (1 << 7) != 0 || flags & (1 << 11) != 0 {
        return None;
    }
    Some(TgMessage { chat_id, id, sender_id: from_id, text, reply_to, ..Default::default() })
}

// ---------------------------------------------------------------------------
// Login + sends
// ---------------------------------------------------------------------------

impl Client {
    /// Bot login via auth.importBotAuthorization.
    /// Registers the bot identity in msg state so on_command etc. work.
    pub fn login_bot(&mut self, api_id: i32, api_hash: &str, token: &str) -> Result<(), String> {
        // import + getState with DC migration: a bot account may live on
        // another DC than the one the session starts on; the server answers
        // *_MIGRATE_X and the whole dance reruns on that DC.
        for attempt in 0..3u8 {
            if self.session.auth_key == [0u8; 256] {
                self.handshake().map_err(|e| format!("handshake: {e}"))?;
            }
            // call_rpc lazily runs ping + initConnection(help.getConfig)
            // wrapped with session.api_id, so bind the identity first.
            self.session.api_id = api_id;
            let mut w = TlWriter::new();
            w.u32(crc227::AUTH_IMPORT_BOT);
            w.u32(1); // flags
            w.i32(api_id);
            w.str(api_hash);
            w.str(token);
            match self.follow_bot_login(api_id, api_hash, token, w.out().to_vec()) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    // A server -404 after login kills the connection binding
                    // (salt/auth-key rotate); reset and rerun the full dance.
                    if e.contains("-404") {
                        self.disconnect();
                        self.session.auth_key = [0u8; 256];
                    }
                    let migrated = migrate_dc(&e).and_then(|dc| {
                        let ip = crate::session::dc_ip(dc);
                        if ip.is_empty() {
                            None
                        } else {
                            self.session.dc_id = dc;
                            self.session.ip = ip.to_string();
                            self.disconnect();
                            self.session.auth_key = [0u8; 256];
                            Some(())
                        }
                    });
                    if migrated.is_none() && !e.contains("-404") {
                        return Err(e);
                    }
                    if attempt == 2 {
                        return Err(e);
                    }
                }
            }
        }
        Err("login: migrate loop exhausted".into())
    }

    /// Consumes auth.importBotAuthorization, reads the bot user, then fires
    /// updates.getState so the server starts pushing updates.
    fn follow_bot_login(
        &mut self,
        _api_id: i32,
        _api_hash: &str,
        _token: &str,
        import_body: Vec<u8>,
    ) -> Result<(), String> {
        let resp = self.call_rpc(&import_body, true, false)?;
        let mut r = TlReader::new(&resp);
        if r.u32() != crc227::AUTH_AUTHORIZATION {
            return Err(format!("auth: unexpected response {}", hex_head(&resp)));
        }
        let flags = r.u32();
        if flags & (1 << 1) != 0 {
            let _ = r.i32();
        }
        if flags & 1 != 0 {
            let _ = r.i32();
        }
        if flags & (1 << 2) != 0 {
            let _ = r.bytes();
        }
        if !self.msg_user_in(&mut r) {
            return Err("auth: could not read bot user".into());
        }
        self.msg.authorized = true;
        if self.session.user_id == 0 {
            self.session.user_id = self.msg.me_id;
        }
        // Ask the server to start pushing updates on this connection
        // (live clients fire updates.getState right after login) and
        // snapshot the sync-position from the same answer.
        if let Err(e) = self.fetch_state() {
            // 303 *_MIGRATE_X on getState means the bot account lives on
            // another DC: bubble up so login_bot's migration loop reruns
            // the whole dance on that DC.
            if e.contains("MIGRATE_") {
                return Err(e);
            }
            eprintln!("state: fetch failed (non-fatal): {e}");
        }
        Ok(())
    }

    /// Asks the server to start pushing updates on this connection.
    /// Parses updates.State (ctor + pts/qts/date/seq/unread, always the tail
    /// of the answer) and snapshots it for getDifference polling.
    pub(crate) fn fetch_state(&mut self) -> Result<(), String> {
        let mut w = TlWriter::new();
        w.u32(crate::client::crc::UPDATES_GET_STATE);
        let resp = self.call_rpc(w.out(), false, false)?;

        // updates.State#a56c2a3e: state is the last object on the wire
        // (24 bytes: ctor + 5 ints with unread_count).
        if resp.len() < 24 {
            return Ok(());
        }
        let tail = &resp[resp.len() - 24..];
        let st = u32::from_le_bytes(tail[..4].try_into().unwrap());
        if st == 0xa56c2a3e {
            let ints: Vec<i32> = tail[4..]
                .chunks_exact(4)
                .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
                .collect();
            if ints.len() >= 5 {
                let mut r = TlReader::new(tail);
                r.u32();
                self.msg.up_pts = r.i32();
                self.msg.up_qts = r.i32();
                self.msg.up_date = r.i32();
                self.msg.up_seq = r.i32();
            }
        }
        Ok(())
    }

    /// one getDifference round: dispatches new messages through the normal
    /// pipeline and advances the stored state snapshot.
    pub(crate) fn poll_difference(&mut self) -> Result<(), String> {
        self.last_poll_ms = crate::transport::mono_ms();
        let (pts, qts, date) = (self.msg.up_pts, self.msg.up_qts, self.msg.up_date);
        if pts <= 0 {
            return Ok(());
        }
        let mut w = TlWriter::new();
        w.u32(crate::client::crc::UPDATES_GET_DIFFERENCE);
        w.u32(0); // flags: no limits
        w.i32(pts);
        w.i32(date);
        w.i32(qts);
        let resp = match self.call_rpc(w.out(), false, false) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("poll difference: call failed: {e}");
                return Ok(()); // transient errors: retry on next cycle
            }
        };
        if resp.len() < 4 {
            return Ok(());
        }
        let dcrc = u32::from_le_bytes(resp[..4].try_into().unwrap());
        const DIFF_EMPTY: u32 = 0x5d75a138;
        // Diff payloads embed full user/channel objects (which carry the
        // access_hash replies need); parsing them through the ordered TL
        // sequence is hard because other_updates has ragged members, so
        // scan the raw bytes for the object ctors instead. Runs before
        // message dispatch so replies can resolve their target peer.
        self.scan_cache_peers(&resp);
        const DIFF_FULL: u32 = 0x00f49ca0;
        const DIFF_SLICE: u32 = 0xa8fb1981;
        if dcrc == DIFF_EMPTY {
            let mut r = TlReader::new(&resp);
            r.u32();
            self.msg.up_date = r.i32();
            self.msg.up_seq = r.i32();
            return Ok(());
        }
        if dcrc != DIFF_FULL && dcrc != DIFF_SLICE {
            return Ok(());
        }
        // difference / differenceSlice: new_messages:Vector<Message> first.
        let mut r = TlReader::new(&resp);
        r.u32(); // difference ctor
        r.u32(); // Vector ctor (vec_count expects it consumed)
        let count = r.vec_count();
        // other_updates:Vector<Update> carries non-message updates such as
        // updateBotCallbackQuery — without handling them, button presses that
        // arrive via getDifference are silently dropped. It is frequently the
        // only payload (new_messages count=0), so scan it before the early
        // return below. Members are ragged, so locate each callback ctor
        // directly in the raw payload instead of walking the vector in order.
        self.scan_cache_callbacks(&resp);
        if count <= 0 || count > 10_000 {
            // still advance state from the tail for slice responses
            self.advance_state_from_tail(&resp);
            return Ok(());
        }
        for _ in 0..count {
            if !self.next_message(&mut r) {
                break;
            }
        }
        // chats / users are skipped; the state snapshot is always the trailing
        // updates.State of the payload.
        self.advance_state_from_tail(&resp);
        Ok(())
    }

    /// Finds updateBotCallbackQuery constructors in a getDifference payload and
    /// dispatches each one. Decoding starts right after the matched crc.
    fn scan_cache_callbacks(&mut self, resp: &[u8]) {
        const CB: u32 = crc227::UPDATE_BOT_CALLBACK;
        let pat = CB.to_le_bytes();
        let mut i = 0;
        while i + 4 <= resp.len() {
            if resp[i..i + 4] == pat {
                let mut r = TlReader::new(&resp[i + 4..]);
                self.msg_handle_callback(&mut r);
                i += 4;
            } else {
                i += 4;
            }
        }
    }

    /// Pulls the trailing updates.State (ctor + 4 ints) out of a difference
    /// payload and updates the stored snapshot; polls re-sync through it.
    fn advance_state_from_tail(&mut self, resp: &[u8]) {
        if resp.len() < 24 {
            return;
        }
        let tail = &resp[resp.len() - 24..];
        if u32::from_le_bytes(tail[..4].try_into().unwrap()) == 0xa56c2a3e {
            let mut sr = TlReader::new(tail);
            sr.u32();
            let pts = sr.i32();
            self.msg.up_qts = sr.i32();
            self.msg.up_date = sr.i32();
            self.msg.up_seq = sr.i32();
            if pts != self.msg.up_pts {
                self.msg.up_pts = pts;
            }
        }
    }

    /// Parses one full Message object at the reader position (TL has no
    /// length prefix, so the parser must consume it whole) and dispatches it.
    fn next_message(&mut self, r: &mut TlReader) -> bool {
        // Save the position: parse_message_in consumes the whole Vector
        // element only when it understands the ctor, which messages guarantee
        // (messageEmpty / message / messageService).
        match parse_message_in(r) {
            Some(m) => {
                self.msg_dispatch_message(m);
                true
            }
            None => false,
        }
    }

    /// Scans diff bytes for user/channel object ctors and caches their
    /// access hashes. Min-flags layout is fixed: ctor, flags, flags2, id,
    /// then access_hash at a known offset driven by a single flag bit.
    fn scan_cache_peers(&mut self, resp: &[u8]) {
        let n = resp.len();
        if n < 12 {
            return;
        }
        let mut i = 0;
        while i <= n - 12 {
            let crc = u32::from_le_bytes(resp[i..i + 4].try_into().unwrap());
            let is_user = crc == crc227::USER;
            let is_channel = crc == crc227::CHANNEL;
            if !is_user && !is_channel {
                i += 4;
                continue;
            }
            let flags = u32::from_le_bytes(resp[i + 4..i + 8].try_into().unwrap());
            let id = i64::from_le_bytes(resp[i + 12..i + 20].try_into().unwrap());
            let hash_bit = if is_user { 1 } else { 1 << 13 };
            if id > 0 && flags & hash_bit != 0 && i + 20 <= n {
                let ah = i64::from_le_bytes(resp[i + 20..i + 28].try_into().unwrap());
                let kind = if is_user {
                    crc227::INPUT_PEER_USER
                } else {
                    crc227::INPUT_PEER_CHANNEL
                };
                // channel#...: megagroup:flags.8?true — needed so supergroups
                // are not treated as broadcast channels by the command guards.
                let megagroup = !is_user && flags & (1 << 8) != 0;
                let classified = is_user || megagroup || flags & (1 << 5) != 0;
                let e = self.msg.peers.entry(id).or_insert(PeerInfo {
                    kind,
                    access_hash: ah,
                    megagroup,
                    classified,
                });
                // A later full channel object may carry the megagroup flag a
                // bare update did not; upgrade it in place.
                if megagroup {
                    e.megagroup = true;
                    e.classified = true;
                }
            }
            i += 4;
        }
    }

    /// Writes the wire InputPeer for chat_id from the update-cache.
    fn write_peer(&self, w: &mut TlWriter, chat_id: i64) -> Result<(), String> {
        match self.msg.peers.get(&chat_id) {
            Some(PeerInfo { kind, access_hash, .. }) => {
                w.u32(*kind);
                w.i64(chat_id);
                if *kind != crc227::INPUT_PEER_CHAT {
                    w.i64(*access_hash);
                }
                Ok(())
            }
            None => Err(format!("peer {chat_id} not in cache (waiting for updates?)")),
        }
    }

    /// send_text: fire-and-forget sendMessage; returns the new message id
    /// (0 when the server updates stream did not confirm in time).
    pub fn send_text(
        &mut self,
        chat_id: i64,
        text: &str,
        reply_to: i64,
        markup: Option<&Markup>,
    ) -> Result<i64, String> {
        let (plain, entities) = parse_html(text);
        let mut w = TlWriter::new();
        w.u32(crc227::MESSAGES_SEND_MESSAGE);
        let mut flags: u32 = 0;
        if reply_to > 0 {
            flags |= 1;
        }
        if markup.is_some() {
            flags |= 1 << 2;
        }
        if !entities.is_empty() {
            flags |= 1 << 3;
        }
        w.u32(flags);
        self.write_peer(&mut w, chat_id)?;
        if reply_to > 0 {
            w.u32(crc227::INPUT_REPLY_TO_MESSAGE);
            w.u32(0);
            w.i32(reply_to as i32);
        }
        w.str(&plain);
        let random = random_msg_id();
        w.i64(random);
        if let Some(mk) = markup {
            write_markup(&mut w, mk);
        }
        if !entities.is_empty() {
            w.vec_header(entities.len() as u32);
            for (crc, off, len, url) in &entities {
                w.u32(*crc);
                match *crc {
                    0xf1ccaaac => {
                        // messageEntityBlockquote (new schema): flags field
                        // precedes offset/length.
                        w.u32(0);
                        w.i32(*off);
                        w.i32(*len);
                    }
                    crc227::MESSAGE_ENTITY_TEXT_URL => {
                        w.i32(*off);
                        w.i32(*len);
                        w.str(url);
                    }
                    crc227::MESSAGE_ENTITY_MENTION_NAME => {
                        w.i32(*off);
                        w.i32(*len);
                        w.i32(url.parse::<i64>().unwrap_or(0) as i32);
                    }
                    crc227::MESSAGE_ENTITY_PRE => {
                        w.i32(*off);
                        w.i32(*len);
                        w.str("");
                    }
                    _ => {
                        w.i32(*off);
                        w.i32(*len);
                    }
                }
            }
        }
        let outgoing = w.out();
        let resp = match self.call_rpc(outgoing, true, false) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("send_text failed: {e}");
                return Err(e);
            }
        };
        self.drain_updates_respond(&resp);
        Ok(self.msg.remap.remove(&random).unwrap_or(0))
    }

    pub fn edit_text(
        &mut self,
        chat_id: i64,
        message_id: i64,
        text: &str,
        markup: Option<&Markup>,
    ) -> Result<bool, String> {
        let (plain, entities) = parse_html(text);
        let mut w = TlWriter::new();
        w.u32(crc227::MESSAGES_EDIT_MESSAGE);
        let mut flags: u32 = 0;
        if markup.is_some() {
            flags |= 1 << 2;
        }
        if !entities.is_empty() {
            flags |= 1 << 3;
        }
        flags |= 1 << 11; // message
        w.u32(flags);
        self.write_peer(&mut w, chat_id)?;
        w.i32(message_id as i32);
        w.str(&plain);
        if let Some(mk) = markup {
            write_markup(&mut w, mk);
        }
        if !entities.is_empty() {
            w.vec_header(entities.len() as u32);
            for (crc, off, len, url) in &entities {
                w.u32(*crc);
                w.i32(*off);
                w.i32(*len);
                if *crc == crc227::MESSAGE_ENTITY_TEXT_URL {
                    w.str(url);
                } else if *crc == crc227::MESSAGE_ENTITY_PRE {
                    w.str("");
                } else if *crc == crc227::MESSAGE_ENTITY_MENTION_NAME {
                    w.i32(url.parse::<i64>().unwrap_or(0) as i32);
                }
            }
        }
        let out = w.out();
        let _resp = self.call_rpc(out, true, false)?;
        Ok(true)
    }

    pub fn delete_message(&mut self, chat_id: i64, message_id: i64) -> Result<bool, String> {
        let mut w = TlWriter::new();
        w.u32(crc227::MESSAGES_DELETE_MESSAGES);
        w.u32(1); // revoke
        w.vec_header(1);
        w.i32(message_id as i32);
        let _resp = self.call_rpc(w.out(), true, false)?;
        let _ = chat_id;
        Ok(true)
    }

    pub fn answer_callback(&mut self, query_id: i64, text: Option<&str>, alert: bool) -> Result<bool, String> {
        let mut w = TlWriter::new();
        w.u32(crc227::MESSAGES_SET_CALLBACK_ANSWER);
        let mut flags: u32 = 0;
        if text.is_some() {
            flags |= 1;
        }
        if alert {
            flags |= 1 << 1;
        }
        w.u32(flags);
        w.i64(query_id);
        if let Some(t) = text {
            w.str(t);
        }
        w.i32(0); // cache_time
        let resp = self.call_rpc(w.out(), true, false)?;
        let ok = u32::from_le_bytes(resp[..4].try_into().unwrap_or([0, 0, 0, 0]));
        Ok(ok == crc227::BOOL_TRUE)
    }

    /// get_replied_sender: fetch a message by id and return the sender user
    /// id (0 when the message or its sender cannot be resolved). Used by admin
    /// commands that target the user being replied to. Supergroups/broadcast
    /// channels go through channels.getMessages, everything else through
    /// messages.getMessages.
    pub fn get_replied_sender(&mut self, chat_id: i64, message_id: i64) -> Result<i64, String> {
        Ok(self
            .fetch_message(chat_id, message_id)?
            .map(|m| m.sender_id)
            .unwrap_or(0))
    }

    /// Fetch one message by id and return it parsed (None when the response
    /// carries an empty Message). Used to re-send replied stickers.
    pub fn get_replied_message(
        &mut self,
        chat_id: i64,
        message_id: i64,
    ) -> Result<Option<TgMessage>, String> {
        self.fetch_message(chat_id, message_id)
    }

    fn fetch_message(
        &mut self,
        chat_id: i64,
        message_id: i64,
    ) -> Result<Option<TgMessage>, String> {
        let peer = self
            .msg
            .peers
            .get(&chat_id)
            .map(|p| (p.kind, p.access_hash));
        let Some((kind, access_hash)) = peer else {
            return Err(format!("peer {chat_id} not in cache (waiting for updates?)"));
        };
        let mut w = TlWriter::new();
        if kind == crc227::INPUT_PEER_CHANNEL {
            w.u32(0xad8c9a23); // channels.getMessages
            w.u32(0xf35aec28); // inputChannel
            w.i64(chat_id);
            w.i64(access_hash);
        } else {
            w.u32(0x63c66506); // messages.getMessages
            self.write_peer(&mut w, chat_id)?;
        }
        w.vec_header(1);
        w.u32(0xa676a322); // inputMessageID
        w.i32(message_id as i32);
        let resp = self.call_rpc(w.out(), true, false)?;
        Ok(parse_first_message(&resp))
    }

    /// messages.setTyping with sendMessageChooseStickerAction — shows the
    /// "choosing a sticker" chat indicator while the engine picks one.
    pub fn set_typing_choosing_sticker(&mut self, chat_id: i64) -> Result<bool, String> {
        let mut w = TlWriter::new();
        w.u32(crc227::MESSAGES_SET_TYPING);
        self.write_peer(&mut w, chat_id)?;
        w.u32(crc227::SEND_MESSAGE_CHOOSE_STICKER_ACTION);
        let resp = self.call_rpc(w.out(), true, false)?;
        let ok = u32::from_le_bytes(resp[..4].try_into().unwrap_or([0, 0, 0, 0]));
        Ok(ok == crc227::BOOL_TRUE)
    }

    /// messages.editMessage with only reply_markup set — preserves
    /// keyboardButtonStyle (which the Bot API drops).
    pub fn edit_reply_markup(
        &mut self,
        chat_id: i64,
        message_id: i64,
        markup: &Markup,
    ) -> Result<bool, String> {
        let mut w = TlWriter::new();
        w.u32(crc227::MESSAGES_EDIT_MESSAGE);
        w.u32(1 << 2); // reply_markup flag
        self.write_peer(&mut w, chat_id)?;
        w.i32(message_id as i32);
        write_markup(&mut w, markup);
        let _resp = self.call_rpc(w.out(), true, false)?;
        Ok(true)
    }

    /// messages.sendMedia with inputMediaDocument/inputDocument — sends a
    /// sticker by document identity. Returns the new message id (0 when the
    /// server updates stream did not confirm in time).
    pub fn send_sticker(
        &mut self,
        chat_id: i64,
        doc_id: i64,
        doc_access_hash: i64,
        doc_file_ref: &[u8],
        reply_to: i64,
    ) -> Result<i64, String> {
        let mut w = TlWriter::new();
        w.u32(crc227::MESSAGES_SEND_MEDIA);
        let mut flags: u32 = 0;
        if reply_to > 0 {
            flags |= 1;
        }
        w.u32(flags);
        self.write_peer(&mut w, chat_id)?;
        if reply_to > 0 {
            w.u32(crc227::INPUT_REPLY_TO_MESSAGE);
            w.u32(0);
            w.i32(reply_to as i32);
        }
        // media: inputMediaDocument flags id:InputDocument
        w.u32(crc227::INPUT_MEDIA_DOCUMENT);
        w.u32(0); // flags
        w.u32(crc227::INPUT_DOCUMENT);
        w.i64(doc_id);
        w.i64(doc_access_hash);
        w.blob(doc_file_ref);
        w.str(""); // message
        let random = random_msg_id();
        w.i64(random);
        let resp = self.call_rpc(w.out(), true, false)?;
        self.drain_updates_respond(&resp);
        Ok(self.msg.remap.remove(&random).unwrap_or(0))
    }

    /// Own bot id as reported by the login authorization.
    pub fn me_id(&self) -> i64 {
        self.msg.me_id
    }

 /// Own bot username.
    pub fn me_username(&self) -> String {
        self.msg.me_username.clone()
    }

 /// Own bot first name.
    pub fn me_first_name(&self) -> String {
        self.msg.me_first_name.clone()
    }

 /// True once login_bot authorized.
    pub fn authorized(&self) -> bool {
        self.msg.authorized
    }

    /// Consumes an Updates response body so updateMessageID remaps land.
    fn drain_updates_respond(&mut self, body: &[u8]) {
        if body.len() < 4 {
            return;
        }
        let crc = u32::from_le_bytes(body[..4].try_into().unwrap());
        let mut r = TlReader::new(&body[4..]);
        match crc {
            crc227::UPDATES_CONTAINER | crc227::UPDATES_COMBINED => self.msg_drain_container(&mut r),
            crc227::UPDATE_SHORT => {
                let _ = self.msg_update_typed(&mut r);
            }
            _ => {}
        }
    }
}

fn random_msg_id() -> i64 {
    i64::from_be_bytes(random_bytes(8)[..].try_into().unwrap()) & ((1 << 62) - 1)
}

fn hex_head(b: &[u8]) -> String {
    b.iter().take(16).map(|x| format!("{x:02x}")).collect()
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Decodes a base64 string (standard alphabet, ignored padding/invalid chars).
/// Returns None when the input is empty or cannot be decoded, so callers can
/// fall back to the raw value.
fn base64_decode_str(s: &str) -> Option<Vec<u8>> {
    if s.is_empty() {
        return None;
    }
    let mut rev = [255u8; 256];
    for (i, &b) in B64.iter().enumerate() {
        rev[b as usize] = i as u8;
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut val: u32 = 0;
    let mut bits: i32 = -8;
    for c in s.bytes() {
        if c == b'=' {
            break;
        }
        let v = rev[c as usize];
        if v == 255 {
            return None;
        }
        val = (val << 6) + v as u32;
        bits += 6;
        if bits >= 0 {
            out.push(((val >> bits) & 0xFF) as u8);
            bits -= 8;
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

/// Extracts the DC id from a *_MIGRATE_X telegram error
/// (e.g. "telegram_error:303:USER_MIGRATE_5" -> 5).
fn migrate_dc(err: &str) -> Option<i32> {
    let tail = err.split("MIGRATE_").nth(1)?;
    let digits: String = tail.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

// ---------------------------------------------------------------------------
// Update stream entry points
// ---------------------------------------------------------------------------

impl Client {
    /// Entry point from the receive loop (all non-RPC pushes land here).
    pub(crate) fn msg_process(&mut self, body: &[u8]) {
        if body.len() < 4 {
            return;
        }
        let crc = u32::from_le_bytes(body[..4].try_into().unwrap());
        let mut r = TlReader::new(&body[4..]);
        match crc {
            crc227::UPDATES_CONTAINER | crc227::UPDATES_COMBINED => {
                // Container order is updates -> users -> chats, but a new
                // channel message dispatches before its channel object is
                // parsed, so the megagroup flag would be missing. Pre-scan
                // the raw body so peers (and their megagroup bit) are cached
                // before the embedded messages are routed.
                self.scan_cache_peers(&body[4..]);
                self.msg_drain_container(&mut r);
            }
            crc227::UPDATE_SHORT => {
                self.msg_update_typed(&mut r);
                if r.remaining() >= 4 {
                    let _date = r.i32();
                }
            }
            crc227::UPDATE_SHORT_MESSAGE => {
                if let Some(m) = msg_from_short_private(&mut r) {
                    self.msg_dispatch_message(m);
                }
            }
            crc227::UPDATE_SHORT_CHAT_MESSAGE => {
                if let Some(m) = msg_from_short_chat(&mut r) {
                    self.msg_dispatch_message(m);
                }
            }
            crc227::UPDATES_TOO_LONG => {
                // Server says the gap is too large; re-snapshot the sync
                // position so the next getDifference re-syncs cleanly.
                let _ = self.fetch_state();
            }
            _ => self.msg_raw(body),
        }
    }

    /// Raw pass-through for updates the messaging layer does not model
    /// (group-call signaling, connection pushes, ...).
    fn msg_raw(&self, body: &[u8]) {
        let crc = u32::from_le_bytes(body[..4].try_into().unwrap());
        let ev = UpdateEvent {
            kind: format!("update:{crc:08x}"),
            group_call_id: 0,
            chat_id: 0,
            payload: body.to_vec(),
        };
        self.emit_update(&ev);
    }

    /// updates#74ae4240 / updatesCombined#725b04c3 bodies (ctor consumed).
    /// Wire order is updates, users, chats, date, seq.
    pub(crate) fn msg_drain_container(&mut self, r: &mut TlReader) {
        // updates#74ae4240 updates:Vector<Update> users:Vector<User>
        // chats:Vector<Chat> date:int seq:int — every Vector<..> is prefixed
        // with the vector#1cb5c415 ctor, so it must be consumed before the
        // element count (otherwise the ctor is read as a huge count and the
        // whole pushed container is dropped).
        let _vec = r.u32();
        let updates = r.vec_count();
        if !(0..=1_000_000).contains(&updates) {
            return;
        }
        for _ in 0..updates {
            if r.remaining() < 4 {
                return;
            }
            if !self.msg_update_typed(r) {
                // Unknown update: hand the rest of the buffer to the raw
                // handler and stop typed parsing (cannot know object sizes).
                let rest = r.rest().to_vec();
                if rest.len() >= 4 {
                    let crc = u32::from_le_bytes(rest[..4].try_into().unwrap());
                    let ev = UpdateEvent {
                        kind: format!("update:{crc:08x}"),
                        group_call_id: 0,
                        chat_id: 0,
                        payload: rest,
                    };
                    self.emit_update(&ev);
                }
                return;
            }
        }
        let _vec = r.u32();
        let users = r.vec_count();
        if !(0..=1_000_000).contains(&users) {
            return;
        }
        for _ in 0..users {
            if !self.msg_user_in(r) {
                return;
            }
        }
        let _vec = r.u32();
        let chats = r.vec_count();
        if !(0..=1_000_000).contains(&chats) {
            return;
        }
        for _ in 0..chats {
            if !self.msg_chat_in(r) {
                return;
            }
        }
    }

    /// Consumes one Update from the reader. Returns false for unknown types.
    pub(crate) fn msg_update_typed(&mut self, r: &mut TlReader) -> bool {
        if r.remaining() < 4 {
            return false;
        }
        let crc = r.u32();
        match crc {
            crc227::UPDATE_NEW_MESSAGE => {
                if let Some(m) = parse_message_in(r) {
                    self.msg_dispatch_message(m);
                }
                let _pts = r.i32();
                let _cnt = r.i32();
                r.ok()
            }
            crc227::UPDATE_NEW_CHANNEL_MESSAGE => {
                // updateNewChannelMessage#5c467992 message:Message pts:int
                // count:int — supergroup and channel posts. Without this a
                // /play in a supergroup never reaches the router at all.
                if let Some(m) = parse_message_in(r) {
                    self.msg_dispatch_message(m);
                }
                let _pts = r.i32();
                let _cnt = r.i32();
                r.ok()
            }
            crc227::UPDATE_BOT_CALLBACK => {
                self.msg_handle_callback(r);
                r.ok()
            }
            crc227::UPDATE_MESSAGE_ID => {
                let id = r.i32() as i64;
                let random = r.i64();
                self.msg.remap.insert(random, id);
                r.ok()
            }
            _ => false,
        }
    }
}
