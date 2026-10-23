//! Ferogram-backed bot facade.
//!
//! Replaces the hand-rolled `parvati_mtproto` client plus the Bot API HTTP
//! layer with the vendored [ferogram] MTProto framework. Ferogram is async,
//! while the entire command surface (the [`Bot`] trait, every module, every
//! test) is synchronous, so this module owns a dedicated Tokio runtime thread
//! and bridges requests onto it.
//!
//! Layout:
//! - a background thread runs a multi-thread Tokio runtime and drives the
//!   `ferogram::Client`;
//! - synchronous calls build a [`Job`] closure, hand it to the runtime over an
//!   mpsc channel, and block on a reply channel;
//! - incoming `Update`s are converted to `TgMessage`/`TgCallback` and pushed to
//!   registered handlers, which forward them onto the module bus — so the
//!   registry and pump loop stay untouched.

use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

use ferogram::tl;
use ferogram::update::Update;
use ferogram::Client;

use crate::config;
use crate::mod_tg::{Bot, MemberState, PromoteRights, StickerRef};
use parvati_mtproto::messaging::{Markup, TgCallback, TgMessage};

/// A unit of work executed on the runtime thread with access to the live
/// client. The closure sends its own typed reply before returning.
type Job = Box<dyn FnOnce(&Client) + Send>;

/// Handlers registered by the sync side, invoked on runtime when updates land.
type CmdFn = Arc<dyn Fn(&TgMessage) + Send + Sync>;
type CbFn = Arc<dyn Fn(&TgCallback) + Send + Sync>;

/// Identity captured at login; used for `me_*`.
#[derive(Clone, Default)]
struct Identity {
    id: i64,
    username: String,
    first_name: String,
}

/// Handler tables + identity, shared with the runtime thread (updates read
/// these; the sync side mutates them before the pump starts).
#[derive(Default)]
struct Shared {
    id: Identity,
    commands: Vec<(String, CmdFn)>,
    callbacks: Vec<(String, CbFn)>,
    messages: Vec<CmdFn>,
}

/// The sync handle the rest of the bot holds as `Box<dyn Bot>`.
pub struct FerogramBot {
    tx: Sender<Job>,
    shared: Arc<Mutex<Shared>>,
    _rt: std::thread::JoinHandle<()>,
}

impl FerogramBot {
    /// Connect and bot-login on a dedicated runtime thread. Returns once the
    /// handshake and login complete, or the error.
    pub fn new(token: &str) -> Result<FerogramBot, String> {
        let cfg = config::load()?;
        let api_id = cfg.api_id;
        let api_hash = cfg.api_hash.clone();
        let token = token.to_string();

        let (tx, rx) = channel::<Job>();
        let (ready_tx, ready_rx) = channel::<Result<Identity, String>>();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let shared_rt = shared.clone();

        let rt = std::thread::Builder::new()
            .name("ferogram-rt".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = ready_tx.send(Err(format!("tokio runtime: {e}")));
                        return;
                    }
                };
                rt.block_on(runtime_main(api_id, &api_hash, &token, rx, ready_tx, shared_rt));
            })
            .map_err(|e| format!("spawn ferogram runtime: {e}"))?;

        let identity = ready_rx
            .recv()
            .map_err(|_| "ferogram runtime died during startup".to_string())??;
        {
            let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
            s.id = identity;
        }

        Ok(FerogramBot { tx, shared, _rt: rt })
    }

    /// Runs `f` on the runtime thread and blocks for its typed result.
    fn call<R, F>(&self, f: F) -> Result<R, String>
    where
        R: Send + 'static,
        F: FnOnce(&Client) -> R + Send + 'static,
    {
        let (rtx, rrx) = channel::<R>();
        let job: Job = Box::new(move |client| {
            let out = f(client);
            let _ = rtx.send(out);
        });
        self.tx
            .send(job)
            .map_err(|_| "ferogram runtime stopped".to_string())?;
        rrx.recv()
            .map_err(|_| "ferogram runtime stopped before reply".to_string())
    }

    /// Like [`call`](Self::call) for closures that already return a
    /// `Result<T, String>`; flattens the runtime-layer error with the op error.
    fn call_r<T, F>(&self, f: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(&Client) -> Result<T, String> + Send + 'static,
    {
        self.call(f).and_then(|r| r)
    }

    /// Registers a command handler fed from Ferogram's update stream.
    pub fn on_command<F>(&mut self, name: &str, f: F)
    where
        F: Fn(&TgMessage) + Send + Sync + 'static,
    {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .commands
            .push((name.to_string(), Arc::new(f)));
    }

    /// Registers a callback-prefix handler fed from Ferogram's update stream.
    pub fn on_callback<F>(&mut self, prefix: &str, f: F)
    where
        F: Fn(&TgCallback) + Send + Sync + 'static,
    {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .callbacks
            .push((prefix.to_string(), Arc::new(f)));
    }

    /// Registers a plain-message handler fed from Ferogram's update stream.
    pub fn on_message<F>(&mut self, f: F)
    where
        F: Fn(&TgMessage) + Send + Sync + 'static,
    {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .messages
            .push(Arc::new(f));
    }
}

/// Runtime-thread main: connect, login, publish identity, server jobs and
/// pump updates until the sync side drops its sender.
async fn runtime_main(
    api_id: i32,
    api_hash: &str,
    token: &str,
    rx: std::sync::mpsc::Receiver<Job>,
    ready: Sender<Result<Identity, String>>,
    shared: Arc<Mutex<Shared>>,
) {
    let session_path = format!("cache/ferogram-{api_id}.session");
    let client = match Client::builder()
        .api_id(api_id)
        .api_hash(api_hash)
        .session(session_path)
        .connect()
        .await
    {
        Ok((c, _shutdown)) => c,
        Err(e) => {
            let _ = ready.send(Err(format!("ferogram connect: {e}")));
            return;
        }
    };
    if let Err(e) = ensure_login(&client, token).await {
        let _ = ready.send(Err(e));
        return;
    }
    let identity = match fetch_identity(&client).await {
        Ok(i) => i,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let _ = ready.send(Ok(identity));

    // Update pump: convert each Update and fan out to the registered handlers.
    let pump_client = client.clone();
    let pump_shared = shared.clone();
    tokio::spawn(async move {
        let mut stream = pump_client.stream_updates();
        while let Some(upd) = stream.next().await {
            dispatch_update(&upd, &pump_shared, &pump_client).await;
        }
    });

    // Job server: the sync side blocks on each reply, so run jobs inline on
    // the runtime thread. Jobs are short (one RPC), and the update pump runs
    // on a separate task.
    while let Ok(job) = rx.recv() {
        job(&client);
    }

    // Best-effort: keep the connection alive for the process lifetime.
    std::future::pending::<()>().await;
}

async fn ensure_login(client: &Client, token: &str) -> Result<(), String> {
    match client.is_authorized().await {
        Ok(true) => Ok(()),
        Ok(false) => {
            client
                .bot_sign_in(token)
                .await
                .map_err(|e| format!("ferogram bot_sign_in: {e}"))?;
            client
                .save_session()
                .await
                .map_err(|e| format!("ferogram save_session: {e}"))?;
            Ok(())
        }
        Err(e) => Err(format!("ferogram is_authorized: {e}")),
    }
}

async fn fetch_identity(client: &Client) -> Result<Identity, String> {
    let u = client
        .get_me()
        .await
        .map_err(|e| format!("ferogram get_me: {e}"))?;
    Ok(Identity {
        id: u.id,
        username: u.username.clone().unwrap_or_default(),
        first_name: u.first_name.clone().unwrap_or_default(),
    })
}

/// Converts one Ferogram update into our Tg types and fans it out.
async fn dispatch_update(upd: &Update, shared: &Arc<Mutex<Shared>>, client: &Client) {
    match upd {
        Update::NewMessage(m) => {
            let msg = incoming_to_message(m, client).await;
            let (cmds, msgs) = {
                let s = shared.lock().unwrap_or_else(|e| e.into_inner());
                (s.commands.clone(), s.messages.clone())
            };
            if let Some((name, _args)) = parse_command(&msg.text) {
                crate::parv_info!(
                    "dispatch",
                    "cmd /{name} sender_id={} chat_id={} private={}",
                    msg.sender_id,
                    msg.chat_id,
                    msg.is_private
                );
                if let Some((_, h)) = cmds.iter().find(|(n, _)| *n == name) {
                    h(&msg);
                }
            }
            for h in &msgs {
                h(&msg);
            }
        }
        Update::CallbackQuery(cb) => {
            let tg = callback_to_callback(cb, client).await;
            let handlers = {
                let s = shared.lock().unwrap_or_else(|e| e.into_inner());
                s.callbacks.clone()
            };
            // Registered prefixes are matched as prefixes: e.g. `room:` matches
            // `room:1:pause`. Each handler closure already carries its own
            // registered prefix in the event it emits, so we just invoke the
            // first handler whose prefix matches.
            let matched = handlers
                .iter()
                .find(|(p, _)| tg.data.starts_with(p.as_str()));
            crate::parv_info!(
                "dispatch",
                "callback user_id={} chat_id={} data={:?} matched={:?}",
                tg.user_id,
                tg.chat_id,
                tg.data,
                matched.map(|(p, _)| p.as_str()).unwrap_or("none")
            );
            if let Some((_, h)) = matched {
                h(&tg);
            } else {
                let _ = futures_block(client.invoke(&tl::functions::messages::SetBotCallbackAnswer {
                    alert: true,
                    query_id: tg.query_id,
                    message: Some("Unknown action".into()),
                    url: None,
                    cache_time: 0,
                }));
            }
        }
        _ => {}
    }
}

/// Splits a leading `/cmd` (or `/cmd@bot`) into (name, rest).
fn parse_command(text: &str) -> Option<(String, String)> {
    let rest = text.strip_prefix('/')?;
    let mut it = rest.splitn(2, ' ');
    let head = it.next().unwrap_or("");
    let name = head.split('@').next().unwrap_or("").to_ascii_lowercase();
    if name.is_empty() {
        return None;
    }
    Some((name, it.next().unwrap_or("").to_string()))
}

/// Peer -> our signed chat_id convention (user>0, chat<0, channel<0 with the
/// -100 prefix for supergroups/channels).
fn peer_i64(p: Option<&tl::enums::Peer>) -> i64 {
    match p {
        Some(tl::enums::Peer::User(u)) => u.user_id,
        Some(tl::enums::Peer::Chat(c)) => -c.chat_id,
        Some(tl::enums::Peer::Channel(c)) => {
            if c.channel_id > 1_000_000_000_000 {
                -c.channel_id
            } else {
                -(1_000_000_000_000 + c.channel_id)
            }
        }
        None => 0,
    }
}

async fn incoming_to_message(
    m: &ferogram::update::IncomingMessage,
    client: &Client,
) -> TgMessage {
    // Use our signed Bot-API-style id convention (user>0, basic-group<0,
    // channel `-100...`), the same one `peer_i64` produces for outgoing peers.
    let chat_id = peer_i64(m.peer_id());
    // Supergroups arrive as `Peer::Channel` just like broadcast channels; only
    // the channel's `megagroup` flag tells them apart. Without this check the
    // broadcast-channel guard (`if m.is_channel { return; }`) swallows every
    // command sent in a group.
    let is_private = m.is_private();
    let is_channel = m.is_channel() && !m.is_megagroup().await;
    // `sender_id()` is `None` in a private DM (the sender is implicitly the
    // peer), so use `effective_sender_id()` which fills that case in; otherwise
    // the owner/sudo checks would compare against 0 and fail in DMs.
    let sender_peer = m.effective_sender_id();
    let sender_id = peer_i64(sender_peer.as_ref());
    // The TL incoming message carries no user profile, so resolve the sender's
    // display name for greeting/mention text ("Hi <name>"); failure is
    // non-fatal and simply leaves the names empty.
    let (first_name, last_name) = resolve_user_name(client, sender_id)
        .await
        .unwrap_or_default();
    TgMessage {
        chat_id,
        id: m.id() as i64,
        sender_id,
        text: m.text().unwrap_or("").to_string(),
        reply_to: m.reply_to_message_id().map(|v| v as i64).unwrap_or(0),
        is_private,
        is_channel,
        first_name,
        last_name,
        ..Default::default()
    }
}

async fn callback_to_callback(
    cb: &ferogram::update::CallbackQuery,
    client: &Client,
) -> TgCallback {
    let chat_id = peer_i64(cb.chat_peer.as_ref());
    // Outgoing buttons carry base64-encoded `callback_data` (see
    // `buttons::data_btn`), so decode it back to the plain `prefix:payload`
    // form the module handlers expect. If it is not valid base64 (e.g. an
    // inline keyboard built elsewhere), fall back to the raw string as-is.
    let raw = cb.data().unwrap_or("");
    let data = decode_callback_data(raw);
    // The TL callback query carries only the sender id, so resolve the display
    // name via `users.getUsers`; card mentions need it ("by <user>"). Failure
    // is non-fatal: `roomcb::cb_mention` falls back to the literal "user".
    let (first_name, last_name) = resolve_user_name(client, cb.user_id)
        .await
        .unwrap_or_default();
    TgCallback {
        chat_id,
        message_id: cb.message_id.map(|v| v as i64).unwrap_or(0),
        user_id: cb.user_id,
        query_id: cb.query_id,
        data,
        first_name,
        last_name,
    }
}

/// Resolves a user id to `(first_name, last_name)`.
///
/// First tries the peer cache (`get_users_by_id`); if that misses (e.g. the
/// sender's `access_hash` was not cached, which yields an empty result), it
/// falls back to a full peer resolve to obtain the hash and re-issues the
/// lookup. Returns `None` when the user genuinely cannot be resolved.
async fn resolve_user_name(client: &Client, user_id: i64) -> Option<(String, String)> {
    if user_id == 0 {
        return None;
    }
    if let Ok(users) = client.get_users_by_id(&[user_id]).await {
        if let Some(u) = users.into_iter().flatten().next() {
            return Some((
                u.first_name().unwrap_or_default().to_string(),
                u.last_name().unwrap_or_default().to_string(),
            ));
        }
    }
    // Cache miss: resolve the peer (fetches + caches the access_hash) and retry.
    let peer = resolve_input_peer(client, user_id).await.ok()?;
    let tl::enums::InputPeer::User(u) = peer else {
        return None;
    };
    let req = tl::functions::users::GetUsers {
        id: vec![tl::enums::InputUser::InputUser(tl::types::InputUser {
            user_id: u.user_id,
            access_hash: u.access_hash,
        })],
    };
    let users = client.invoke(&req).await.ok()?;
    for user in users {
        if let tl::enums::User::User(u) = user {
            return Some((
                u.first_name.unwrap_or_default(),
                u.last_name.unwrap_or_default(),
            ));
        }
    }
    None
}

/// Decodes a button's `callback_data` back to its plain `prefix:payload` form.
/// Outgoing buttons base64-encode their payload (see `buttons::data_btn`), so
/// inbound data must be decoded before handlers can match on it.
fn decode_callback_data(raw: &str) -> String {
    crate::utils::base64_decode(raw)
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_else(|| raw.to_string())
}

// ---------------------------------------------------------------------------
// Markup conversion: our `Markup` -> Ferogram `ReplyMarkup` (with button style)
// ---------------------------------------------------------------------------

/// Converts our markup (with `keyboardButtonStyle` colours) into Ferogram's
/// `ReplyMarkup`, preserving each button's style.
fn markup_to_ferogram(markup: &Markup) -> tl::enums::ReplyMarkup {
    use ferogram::keyboard::Button as FButton;

    let rows: Vec<tl::enums::KeyboardInlineButtonRow> = markup
        .rows
        .iter()
        .map(|row| {
            let buttons: Vec<tl::enums::KeyboardInlineButton> = row
                .iter()
                .map(|b| {
                    let mut fb = match &b.kind {
                        parvati_mtproto::messaging::Btn::Callback(data) => {
                            FButton::callback(b.text.clone(), data.clone())
                        }
                        parvati_mtproto::messaging::Btn::Url(url) => {
                            FButton::url(b.text.clone(), url.clone())
                        }
                    };
                    fb = match b.style {
                        1 => fb.style(FButton::style_primary()),
                        2 => fb.style(FButton::style_danger()),
                        3 => fb.style(FButton::style_success()),
                        _ => fb,
                    };
                    fb.into_raw()
                })
                .collect();
            tl::enums::KeyboardInlineButtonRow::KeyboardInlineButtonRow(
                tl::types::KeyboardInlineButtonRow { buttons },
            )
        })
        .collect();

    tl::enums::ReplyMarkup::ReplyInlineMarkup(tl::types::ReplyInlineMarkup {
        force_reply: false,
        rows,
    })
}

// ---------------------------------------------------------------------------
// `Bot` trait implementation
// ---------------------------------------------------------------------------

use crate::parv_warn;

impl FerogramBot {
    fn markup_tl(markup: Option<&Markup>) -> Option<tl::enums::ReplyMarkup> {
        markup.map(markup_to_ferogram)
    }

    fn send_text_impl(
        &self,
        chat_id: i64,
        text: &str,
        markup: Option<&Markup>,
        reply_to: i64,
    ) -> Result<i64, String> {
        let text = text.to_string();
        let mk = Self::markup_tl(markup);
        self.call_r(move |client| {
            let mut msg = ferogram::InputMessage::html(text);
            if let Some(mk) = mk {
                msg = msg.reply_markup(mk);
            }
            if reply_to != 0 {
                msg = msg.reply_to(Some(reply_to as i32));
            }
            match futures_block(client.send_message(chat_id, msg)) {
                Ok(m) => Ok(m.id() as i64),
                Err(e) => Err(format!("ferogram send_message: {e}")),
            }
        })
    }

    fn edit_text_impl(
        &self,
        chat_id: i64,
        message_id: i64,
        text: &str,
        markup: Option<&Markup>,
    ) -> Result<bool, String> {
        let text = text.to_string();
        let mk = Self::markup_tl(markup);
        self.call_r(move |client| {
            let mut msg = ferogram::InputMessage::html(text);
            if let Some(mk) = mk {
                msg = msg.reply_markup(mk);
            }
            match futures_block(client.edit_message(chat_id, message_id as i32, msg)) {
                Ok(()) => Ok(true),
                Err(e) => Err(format!("ferogram edit_message: {e}")),
            }
        })
    }

    fn delete_impl(&self, chat_id: i64, message_id: i64) -> Result<bool, String> {
        let _ = chat_id;
        self.call_r(move |client| {
            match futures_block(client.delete_messages(&[message_id as i32], true)) {
                Ok(()) => Ok(true),
                Err(e) => Err(format!("ferogram delete_messages: {e}")),
            }
        })
    }

    /// Send a photo given a URL or a local path.
    fn send_photo_impl(
        &self,
        chat_id: i64,
        photo: &str,
        local_path: bool,
        caption: &str,
        markup: Option<&Markup>,
        reply_to: i64,
    ) -> Result<i64, String> {
        let photo = photo.to_string();
        let caption = caption.to_string();
        let mk = Self::markup_tl(markup);
        self.call_r(move |client| {
            let media: tl::enums::InputMedia = if local_path {
                let bytes = match std::fs::read(&photo) {
                    Ok(b) => b,
                    Err(e) => return Err(format!("read photo {photo}: {e}")),
                };
                let name = photo.rsplit('/').next().unwrap_or("photo.jpg").to_string();
                let mime = mime_of(&name);
                let data = Arc::new(bytes);
                match futures_block(client.upload_concurrent(data, &name, mime, None)) {
                    Ok(up) => up.as_photo_media(),
                    Err(e) => return Err(format!("ferogram upload: {e}")),
                }
            } else {
                tl::enums::InputMedia::PhotoExternal(tl::types::InputMediaPhotoExternal {
                    spoiler: false,
                    url: photo.clone(),
                    ttl_seconds: None,
                })
            };
            let mut msg = ferogram::InputMessage::html(caption);
            if let Some(mk) = mk {
                msg = msg.reply_markup(mk);
            }
            if reply_to != 0 {
                msg = msg.reply_to(Some(reply_to as i32));
            }
            match futures_block(client.send_file(chat_id, media, &msg)) {
                Ok(m) => Ok(m.id() as i64),
                Err(e) => Err(format!("ferogram send_file: {e}")),
            }
        })
    }

    fn pin_impl(&self, chat_id: i64, message_id: i64, silent: bool) -> Result<bool, String> {
        self.call_r(move |client| {
            match futures_block(client.pin_message(chat_id, message_id as i32, silent)) {
                Ok(()) => Ok(true),
                Err(e) => Err(format!("ferogram pin_message: {e}")),
            }
        })
    }

    fn unpin_impl(&self, chat_id: i64, message_id: i64) -> Result<bool, String> {
        self.call_r(move |client| {
            let peer = match futures_block(resolve_input_peer(client, chat_id)) {
                Ok(p) => p,
                Err(e) => return Err(e),
            };
            let req = tl::functions::messages::UpdatePinnedMessage {
                silent: true,
                unpin: true,
                pm_oneside: false,
                peer,
                id: message_id as i32,
            };
            match futures_block(client.invoke(&req)) {
                Ok(_) => Ok(true),
                Err(e) => Err(format!("ferogram unpin: {e}")),
            }
        })
    }

    fn unpin_all_impl(&self, chat_id: i64) -> Result<bool, String> {
        self.call_r(move |client| {
            match futures_block(client.unpin_all_messages(chat_id)) {
                Ok(()) => Ok(true),
                Err(e) => Err(format!("ferogram unpin_all: {e}")),
            }
        })
    }

    fn answer_callback_impl(&self, query_id: i64, text: &str, alert: bool) -> Result<bool, String> {
        let text = text.to_string();
        self.call_r(move |client| {
            let mut req = tl::functions::messages::SetBotCallbackAnswer {
                alert,
                query_id,
                message: if text.is_empty() { None } else { Some(text.clone()) },
                url: None,
                cache_time: 0,
            };
            let _ = &mut req;
            match futures_block(client.invoke(&req)) {
                Ok(_) => Ok(true),
                Err(e) => Err(format!("ferogram answer_callback: {e}")),
            }
        })
    }

    fn send_chat_action_impl(
        &self,
        chat_id: i64,
        action: tl::enums::SendMessageAction,
    ) -> Result<bool, String> {
        self.call_r(move |client| match futures_block(client.send_chat_action(chat_id, action)) {
            Ok(()) => Ok(true),
            Err(e) => Err(format!("ferogram send_chat_action: {e}")),
        })
    }
}

/// Drives a Ferogram future to completion on the runtime thread. Safe because
/// every [`FerogramBot::call`] closure already executes inside the Tokio
/// runtime; this simply pins the future and polls it without a nested
/// `block_on` (which would panic).
fn futures_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(fut)
    })
}

/// Best-effort MIME type from a file name (photos only need `image/*`).
fn mime_of(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else {
        "image/jpeg"
    }
}

impl Bot for FerogramBot {
    fn send_text(&mut self, chat_id: i64, text: &str, markup: Option<&Markup>) -> Result<bool, String> {
        self.send_text_impl(chat_id, text, markup, 0).map(|_| true)
    }

    fn send_text_sync(
        &mut self,
        chat_id: i64,
        text: &str,
        markup: Option<&Markup>,
        reply_to: i64,
    ) -> Result<i64, String> {
        self.send_text_impl(chat_id, text, markup, reply_to)
    }

    fn reply_text(&mut self, msg: &TgMessage, text: &str, markup: Option<&Markup>) -> Result<bool, String> {
        self.send_text_impl(msg.chat_id, text, markup, msg.id).map(|_| true)
    }

    fn edit_text(
        &mut self,
        chat_id: i64,
        message_id: i64,
        text: &str,
        markup: Option<&Markup>,
    ) -> Result<bool, String> {
        self.edit_text_impl(chat_id, message_id, text, markup)
    }

    fn send_photo(
        &mut self,
        chat_id: i64,
        photo: &str,
        caption: &str,
        markup: Option<&Markup>,
    ) -> Result<bool, String> {
        let local = std::path::Path::new(photo).is_file();
        self.send_photo_impl(chat_id, photo, local, caption, markup, 0).map(|_| true)
    }

    fn send_photo_sync(
        &mut self,
        chat_id: i64,
        photo: &str,
        local_path: bool,
        caption: &str,
        markup: Option<&Markup>,
        reply_to: i64,
    ) -> Result<i64, String> {
        self.send_photo_impl(chat_id, photo, local_path, caption, markup, reply_to)
    }

    fn edit_caption(
        &mut self,
        chat_id: i64,
        message_id: i64,
        caption: &str,
        markup: Option<&Markup>,
        quiet: bool,
    ) -> Result<bool, String> {
        if message_id == 0 {
            return Ok(false);
        }
        match self.edit_text_impl(chat_id, message_id, caption, markup) {
            Ok(v) => Ok(v),
            Err(e) => {
                if !quiet {
                    parv_warn!("ferogram", "editMessageCaption failed: {e}");
                }
                Ok(false)
            }
        }
    }

    fn edit_markup(
        &mut self,
        chat_id: i64,
        message_id: i64,
        markup: Option<&Markup>,
        quiet: bool,
    ) -> Result<bool, String> {
        if message_id == 0 {
            return Ok(false);
        }
        let Some(mk) = markup else {
            return Ok(false);
        };
        // Ferogram's edit_message carries reply_markup with button styles,
        // so a single raw edit preserves keyboardButtonStyle colours.
        let tl_mk = markup_to_ferogram(mk);
        self.call_r(move |client| {
            let peer = match futures_block(resolve_input_peer(client, chat_id)) {
                Ok(p) => p,
                Err(e) => {
                    if !quiet {
                        parv_warn!("ferogram", "resolve peer for editMarkup failed: {e}");
                    }
                    return Ok(false);
                }
            };
            let req = tl::functions::messages::EditMessage {
                no_webpage: false,
                invert_media: false,
                peer,
                id: message_id as i32,
                message: None,
                media: None,
                reply_markup: Some(tl_mk),
                entities: None,
                schedule_date: None,
                quick_reply_shortcut_id: None,
                schedule_repeat_period: None,
                rich_message: None,
            };
            match futures_block(client.invoke(&req)) {
                Ok(_) => Ok(true),
                Err(e) => {
                    if !quiet {
                        parv_warn!("ferogram", "editMessageReplyMarkup failed: {e}");
                    }
                    Ok(false)
                }
            }
        })
    }

    fn answer_callback(&mut self, query_id: i64, text: &str, alert: bool) -> Result<bool, String> {
        self.answer_callback_impl(query_id, text, alert)
    }

    fn delete_message(&mut self, chat_id: i64, message_id: i64) -> Result<bool, String> {
        if message_id == 0 {
            return Ok(false);
        }
        self.delete_impl(chat_id, message_id)
    }

    fn ban_member(&mut self, chat_id: i64, user_id: i64, until: i64) -> Result<bool, String> {
        self.call_r(move |client| {
            match futures_block(client.ban(chat_id, user_id, if until > 0 { Some(until as i32) } else { None }))
            {
                Ok(()) => Ok(true),
                Err(e) => Err(format!("ferogram ban: {e}")),
            }
        })
    }

    fn unban_member(&mut self, chat_id: i64, user_id: i64) -> Result<bool, String> {
        self.call_r(move |client| {
            match futures_block(client.restrict(chat_id, user_id, |b| b)) {
                Ok(()) => Ok(true),
                Err(e) => Err(format!("ferogram unban: {e}")),
            }
        })
    }

    fn mute_member(&mut self, chat_id: i64, user_id: i64) -> Result<bool, String> {
        self.call_r(move |client| {
            match futures_block(client.restrict(chat_id, user_id, |b| b.send_messages(false))) {
                Ok(()) => Ok(true),
                Err(e) => Err(format!("ferogram mute: {e}")),
            }
        })
    }

    fn promote_member(&mut self, chat_id: i64, user_id: i64, rights: &PromoteRights) -> Result<bool, String> {
        let r = *rights;
        self.call_r(move |client| {
            let built = futures_block(client.set_admin(chat_id, user_id, move |mut b| {
                b = b.change_info(r.change_info);
                b = b.delete_messages(r.del);
                b = b.invite_users(r.invite);
                b = b.ban_users(r.restrict);
                b = b.pin_messages(r.pin);
                b = b.add_admins(r.promote);
                b = b.manage_call(r.video);
                b
            }));
            match built {
                Ok(()) => Ok(true),
                Err(e) => Err(format!("ferogram set_admin: {e}")),
            }
        })
    }

    fn demote_member(&mut self, chat_id: i64, user_id: i64) -> Result<bool, String> {
        self.promote_member(chat_id, user_id, &PromoteRights::default())
    }

    fn pin_message(&mut self, chat_id: i64, message_id: i64, silent: bool) -> Result<bool, String> {
        self.pin_impl(chat_id, message_id, silent)
    }

    fn unpin_message(&mut self, chat_id: i64, message_id: i64) -> Result<bool, String> {
        self.unpin_impl(chat_id, message_id)
    }

    fn unpin_all_messages(&mut self, chat_id: i64) -> Result<bool, String> {
        self.unpin_all_impl(chat_id)
    }

    fn get_chat_member_state(&mut self, chat_id: i64, user_id: i64) -> MemberState {
        let r = self.call(move |client| -> Result<_, String> {
            let channel = futures_block(resolve_input_channel(client, chat_id))?;
            let req = tl::functions::channels::GetParticipant {
                channel,
                participant: tl::enums::InputPeer::User(tl::types::InputPeerUser {
                    user_id,
                    access_hash: 0,
                }),
            };
            futures_block(client.invoke(&req)).map_err(|e| format!("{e}"))
        });
        let mut out = MemberState::default();
        match r {
            Ok(Ok(resp)) => {
                out.ok = true;
                out.present = true;
                classify_participant(&resp, &mut out);
                out
            }
            Ok(Err(e)) => {
                let s = e.to_string();
                if s.contains("USER_NOT_PARTICIPANT") {
                    out.ok = true;
                    out.present = false;
                } else {
                    out.error = format!("telegram_error:0:{s}");
                }
                out
            }
            Err(e) => {
                out.error = format!("telegram_error:0:{e}");
                out
            }
        }
    }

    fn chat_public_username(&mut self, chat_id: i64) -> String {
        let r = self.call(move |client| -> Result<_, String> {
            let channel = futures_block(resolve_input_channel(client, chat_id))?;
            let req = tl::functions::channels::GetFullChannel { channel };
            futures_block(client.invoke(&req)).map_err(|e| format!("{e}"))
        });
        match r {
            Ok(Ok(resp)) => extract_chat_username(&resp).unwrap_or_default(),
            _ => String::new(),
        }
    }

    fn create_chat_invite_link(&mut self, chat_id: i64) -> Result<String, String> {
        self.call_r(move |client| {
            let peer = futures_block(resolve_input_peer(client, chat_id))?;
            let req = tl::functions::messages::ExportChatInvite {
                legacy_revoke_permanent: false,
                request_needed: false,
                peer,
                expire_date: None,
                usage_limit: None,
                title: None,
                subscription_pricing: None,
            };
            match futures_block(client.invoke(&req)) {
                Ok(resp) => match extract_invite_link(&resp) {
                    Some(link) if !link.is_empty() => Ok(link),
                    _ => Err("ferogram: exportChatInvite returned an empty link".into()),
                },
                Err(e) => Err(format!("ferogram exportChatInvite: {e}")),
            }
        })
    }

    fn approve_join_request(&mut self, chat_id: i64, user_id: i64) -> Result<bool, String> {
        self.call_r(move |client| {
            let peer = futures_block(resolve_input_peer(client, chat_id))?;
            let req = tl::functions::messages::HideChatJoinRequest {
                approved: true,
                peer,
                user_id: tl::enums::InputUser::InputUser(tl::types::InputUser {
                    user_id,
                    access_hash: 0,
                }),
            };
            match futures_block(client.invoke(&req)) {
                Ok(_) => Ok(true),
                Err(e) => Err(format!("ferogram hideChatJoinRequest: {e}")),
            }
        })
    }

    fn get_replied_sender(&mut self, chat_id: i64, message_id: i64) -> i64 {
        let rid = self.call(move |client| {
            futures_block(client.get_messages(chat_id, &[message_id as i32]))
                .ok()
                .and_then(|v| v.into_iter().next())
                .and_then(|m| m.reply_to_message_id())
                .map(|v| v as i64)
        });
        let Ok(Some(rid)) = rid else {
            return 0;
        };
        let sender = self.call(move |client| {
            futures_block(client.get_messages(chat_id, &[rid as i32]))
                .ok()
                .and_then(|v| v.into_iter().next())
                .map(|m| peer_i64(m.sender_id()))
                .unwrap_or(0)
        });
        sender.unwrap_or(0).max(0)
    }

    fn send_typing(&mut self, chat_id: i64) -> Result<bool, String> {
        self.send_chat_action_impl(
            chat_id,
            tl::enums::SendMessageAction::SendMessageTypingAction,
        )
    }

    fn send_chat_action_choosing_sticker(&mut self, chat_id: i64) -> Result<bool, String> {
        self.send_chat_action_impl(
            chat_id,
            tl::enums::SendMessageAction::SendMessageChooseStickerAction,
        )
    }

    fn send_sticker(
        &mut self,
        chat_id: i64,
        doc_id: i64,
        doc_access_hash: i64,
        doc_file_ref: &[u8],
        reply_to: i64,
    ) -> Result<i64, String> {
        let file_ref = doc_file_ref.to_vec();
        self.call_r(move |client| {
            let media = tl::enums::InputMedia::Document(tl::types::InputMediaDocument {
                spoiler: false,
                id: tl::enums::InputDocument::InputDocument(tl::types::InputDocument {
                    id: doc_id,
                    access_hash: doc_access_hash,
                    file_reference: file_ref.clone(),
                }),
                video_cover: None,
                video_timestamp: None,
                ttl_seconds: None,
                query: None,
            });
            let mut msg = ferogram::InputMessage::text(String::new());
            if reply_to != 0 {
                msg = msg.reply_to(Some(reply_to as i32));
            }
            match futures_block(client.send_file(chat_id, media, &msg)) {
                Ok(m) => Ok(m.id() as i64),
                Err(e) => Err(format!("ferogram send_sticker: {e}")),
            }
        })
    }

    fn get_replied_sticker(&mut self, chat_id: i64, message_id: i64) -> Option<StickerRef> {
        let rid = self.call(move |client| {
            futures_block(client.get_messages(chat_id, &[message_id as i32]))
                .ok()
                .and_then(|v| v.into_iter().next())
                .and_then(|m| m.reply_to_message_id())
                .map(|v| v as i64)
        });
        let rid = rid.ok().flatten()?;
        let doc = self.call(move |client| {
            let m = futures_block(client.get_messages(chat_id, &[rid as i32]))
                .ok()
                .and_then(|v| v.into_iter().next());
            m.and_then(|m| extract_sticker(&m))
        });
        doc.ok().flatten()
    }

    fn me_id(&self) -> i64 {
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).id.id
    }

    fn me_username(&self) -> String {
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).id.username.clone()
    }

    fn me_first_name(&self) -> String {
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).id.first_name.clone()
    }

    fn ready(&self) -> bool {
        self.me_id() != 0
    }
}

/// Resolves a signed chat_id into an `InputPeer` following the
/// user>0 / channel<-1e12 / basic-group<0 convention.
async fn resolve_input_peer(
    client: &Client,
    chat_id: i64,
) -> Result<tl::enums::InputPeer, String> {
    let peer = ferogram::PeerRef::from(chat_id)
        .resolve(client)
        .await
        .map_err(|e| format!("resolve peer {chat_id}: {e}"))?;
    client
        .resolve_to_input_peer(&peer)
        .await
        .map_err(|e| format!("input peer {chat_id}: {e}"))
}

/// Resolves a signed supergroup/channel chat_id into an `InputChannel`.
async fn resolve_input_channel(
    client: &Client,
    chat_id: i64,
) -> Result<tl::enums::InputChannel, String> {
    match resolve_input_peer(client, chat_id).await? {
        tl::enums::InputPeer::Channel(c) => {
            Ok(tl::enums::InputChannel::InputChannel(tl::types::InputChannel {
                channel_id: c.channel_id,
                access_hash: c.access_hash,
            }))
        }
        other => Err(format!("chat {chat_id} is not a channel: {other:?}")),
    }
}

fn classify_participant(resp: &tl::enums::channels::ChannelParticipant, out: &mut MemberState) {
    use tl::enums::ChannelParticipant as CP;
    match resp {
        tl::enums::channels::ChannelParticipant::ChannelParticipant(cp) => match &cp.participant {
            CP::Creator(_) => {
                out.admin = true;
                out.can_restrict = true;
                out.can_promote = true;
                out.can_pin = true;
                out.can_delete = true;
            }
            CP::Admin(a) => {
                out.admin = true;
                let tl::enums::ChatAdminRights::ChatAdminRights(r) = &a.admin_rights;
                out.can_restrict = r.ban_users;
                out.can_promote = r.add_admins;
                out.can_pin = r.pin_messages;
                out.can_delete = r.delete_messages;
            }
            CP::Banned(b) => {
                let tl::enums::ChatBannedRights::ChatBannedRights(r) = &b.banned_rights;
                if r.view_messages {
                    out.banned = true;
                    out.present = false;
                }
            }
            _ => {}
        },
    }
}

fn extract_chat_username(resp: &tl::enums::messages::ChatFull) -> Option<String> {
    let tl::enums::messages::ChatFull::ChatFull(cf) = resp;
    match &cf.chats.first() {
        Some(tl::enums::Chat::Channel(c)) => c.username.clone(),
        _ => None,
    }
}

fn extract_invite_link(resp: &tl::enums::ExportedChatInvite) -> Option<String> {
    match resp {
        tl::enums::ExportedChatInvite::ChatInviteExported(e) => Some(e.link.clone()),
        _ => None,
    }
}

fn extract_sticker(m: &ferogram::update::IncomingMessage) -> Option<StickerRef> {
    use tl::enums::MessageMedia as MM;
    let tl::enums::Message::Message(msg) = &m.raw else {
        return None;
    };
    let Some(MM::Document(md)) = &msg.media else {
        return None;
    };
    let Some(tl::enums::Document::Document(doc)) = &md.document else {
        return None;
    };
    let is_sticker = doc
        .attributes
        .iter()
        .any(|a| matches!(a, tl::enums::DocumentAttribute::Sticker(_)));
    if !is_sticker {
        return None;
    }
    let emoji = doc
        .attributes
        .iter()
        .find_map(|a| match a {
            tl::enums::DocumentAttribute::Sticker(s) => Some(s.alt.clone()),
            _ => None,
        })
        .unwrap_or_default();
    Some(StickerRef {
        doc_id: doc.id,
        doc_access_hash: doc.access_hash,
        doc_file_ref: doc.file_reference.clone(),
        emoji,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use parvati_mtproto::messaging::{Btn, Button, Markup};

    #[test]
    fn parse_command_splits_name_and_args() {        assert_eq!(
            parse_command("/play hello world"),
            Some(("play".to_string(), "hello world".to_string()))
        );
        assert_eq!(
            parse_command("/Pause"),
            Some(("pause".to_string(), String::new()))
        );
        assert_eq!(
            parse_command("/skip@parvati_bot 2"),
            Some(("skip".to_string(), "2".to_string()))
        );
        assert_eq!(parse_command("not a command"), None);
        assert_eq!(parse_command("/"), None);
    }

    #[test]
    fn peer_i64_applies_our_chat_id_convention() {
        let user = tl::enums::Peer::User(tl::types::PeerUser { user_id: 555 });
        assert_eq!(peer_i64(Some(&user)), 555);

        let chat = tl::enums::Peer::Chat(tl::types::PeerChat { chat_id: 42 });
        assert_eq!(peer_i64(Some(&chat)), -42);

        let channel = tl::enums::Peer::Channel(tl::types::PeerChannel {
            channel_id: 3915522165,
        });
        assert_eq!(peer_i64(Some(&channel)), -1003915522165);

        assert_eq!(peer_i64(None), 0);
    }

    #[test]
    fn markup_to_ferogram_preserves_style_and_kind() {
        let mk = Markup {
            rows: vec![vec![
                Button {
                    text: "x".into(),
                    kind: Btn::Callback(b"room:1:pause".to_vec()),
                    style: 1,
                },
                Button {
                    text: "y".into(),
                    kind: Btn::Url("https://t.me/z".into()),
                    style: 0,
                },
            ]],
        };
        let tl::enums::ReplyMarkup::ReplyInlineMarkup(rm) = markup_to_ferogram(&mk) else {
            panic!("expected inline markup");
        };
        assert_eq!(rm.rows.len(), 1);
        let tl::enums::KeyboardInlineButtonRow::KeyboardInlineButtonRow(row) = &rm.rows[0];
        assert_eq!(row.buttons.len(), 2);

        let tl::enums::KeyboardInlineButton::KeyboardInlineButton(cb) = &row.buttons[0];
        assert_eq!(cb.text, "x");
        assert!(cb.style.is_some(), "primary style must be preserved");
        assert!(matches!(
            cb.r#type,
            tl::enums::InlineButtonType::Callback(_)
        ));

        let tl::enums::KeyboardInlineButton::KeyboardInlineButton(url) = &row.buttons[1];
        assert_eq!(url.text, "y");
        assert!(url.style.is_none());
        assert!(matches!(url.r#type, tl::enums::InlineButtonType::Url(_)));
    }

    #[test]
    fn decode_callback_data_round_trips_through_base64() {
        let encoded = crate::utils::base64_encode(b"room:1:pause");
        assert_eq!(decode_callback_data(&encoded), "room:1:pause");
        assert_eq!(decode_callback_data(&crate::utils::base64_encode(b"restart:replay")), "restart:replay");
        assert_eq!(decode_callback_data(&crate::utils::base64_encode(b"help:play")), "help:play");
    }

    #[test]
    fn decode_callback_data_falls_back_to_raw_on_non_base64() {
        // A legacy/plain payload must pass through untouched rather than be
        // silently dropped, so handlers still see *something* matchable.
        assert_eq!(decode_callback_data("not base64!!"), "not base64!!");
    }

    #[test]
    fn callback_prefix_matches_registered_prefix_the_way_dispatch_does() {
        let registered = ["room:", "help:", "restart:", "set", "close"];
        let matches = |data: &str| {
            registered
                .iter()
                .find(|p| data.starts_with(**p))
                .copied()
        };
        assert_eq!(matches("room:1:pause"), Some("room:"));
        assert_eq!(matches("help:play"), Some("help:"));
        assert_eq!(matches("restart:replay"), Some("restart:"));
        assert_eq!(matches("set:adminmode"), Some("set"));
        assert_eq!(matches("close"), Some("close"));
        assert_eq!(matches("unknown:x"), None);
    }
}


