//! Module registration entry point.
//! Cards and helpers live in common.rs;
//! command families are registered by their own modules.
//!
//! Deviation note: handlers receive queued events via the bus instead of
//! inline dispatch: the mtproto client dispatches commands from inside an
//! RPC wait holding `&mut Client`,
//! so handlers, which need the full Bot facade, cannot run in that frame.
//! Registrations therefore push `Event`s onto a bus and the run loop drains
//! them via `pump`.
pub mod basic;
pub mod admin;
pub mod chat_state;
pub mod common;
pub mod events;
pub mod help_texts;
pub mod lists;
pub mod moderation;
pub mod pause;
pub mod play;
pub mod playlists;
pub mod queuecmd;
pub mod roomcb;
pub mod settings_cb;
pub mod skip;
pub mod sticker;
pub mod stop;
pub mod system;
pub mod vc;

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};

use parvati_mtproto::messaging::{TgCallback, TgMessage};

use crate::assistant::AssistantManager;
use crate::config::Config;
use crate::ferogram_bot::FerogramBot;
use crate::locales::Locales;
use crate::mod_tg::Bot;
use crate::room::Rooms;

/// file-scope globals: modules are free functions and reach
/// process state through these slots, set once by `init`.
static LOC: OnceLock<Arc<Locales>> = OnceLock::new();
static ASSIST: OnceLock<Arc<AssistantManager>> = OnceLock::new();
static ROOMS: OnceLock<Rooms> = OnceLock::new();
/// Mutable singleton (Config::get parity): tests swap limits via
/// set_cfg before the process's first cfg() use; handlers read cached values.
static CFG: Mutex<Option<Arc<Config>>> = Mutex::new(None);

/// Locales for locale-bearing handlers; falls back to the vendored copy so
/// handlers stay callable without `init` (tests, ad-hoc use).
pub fn loc() -> &'static Locales {
    let arc: &'static Arc<Locales> = LOC.get_or_init(|| {
        Arc::new(
            Locales::load(concat!(env!("CARGO_MANIFEST_DIR"), "/locales"))
                .unwrap_or_else(|e| panic!("vendored locales unreadable: {e}")),
        )
    });
    arc.as_ref()
}

/// The exact Locales singleton main passed to `init`, shared with the
/// monitor threads (`None` before init, vendored fallback untouched).
pub fn locales_arc() -> Option<Arc<Locales>> {
    LOC.get().cloned()
}

/// Config for markup/limit handlers; lazily loads from the environment once
/// and caches, like the Config::get singleton.
pub fn cfg() -> Arc<Config> {
    let mut slot = CFG.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = slot.as_ref() {
        return c.clone();
    }
    let c = Arc::new(crate::config::load().unwrap_or_default());
    *slot = Some(c.clone());
    c
}

/// Replaces the cached config (setup seam for main/tests).
pub fn set_cfg(c: Config) {
    *CFG.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(c));
}

/// Installs the shared rooms handle (main/test setup).
pub fn set_rooms(rooms: Rooms) {
    let _ = ROOMS.set(rooms);
}

/// Installs the assistant manager (main/test setup).
pub fn set_assistants(assistants: Arc<AssistantManager>) {
    let _ = ASSIST.set(assistants);
}

/// Assistant manager once `init` ran (None before that).
pub fn assistants() -> Option<&'static Arc<AssistantManager>> {
    ASSIST.get()
}

/// Shared rooms once `init` ran (None before that).
pub fn rooms() -> Option<&'static Rooms> {
    ROOMS.get()
}

pub type CmdHandler = fn(&mut dyn Bot, &TgMessage);
pub type CbHandler = fn(&mut dyn Bot, &TgCallback);
pub type MsgHandler = fn(&mut dyn Bot, &TgMessage);

/// name -> handler map.
#[derive(Default)]
pub struct Registry {
    cmds: HashMap<String, CmdHandler>,
    cbs: Vec<(String, CbHandler)>,
    msgs: Vec<MsgHandler>,
}

impl Registry {
    pub fn command(&mut self, name: &str, f: CmdHandler) {
        self.cmds.insert(name.to_string(), f);
    }

    pub fn callback(&mut self, prefix: &str, f: CbHandler) {
        self.cbs.push((prefix.to_string(), f));
    }

    /// Registers a handler run for every plain (non-command) message.
    pub fn on_message(&mut self, f: MsgHandler) {
        self.msgs.push(f);
    }

    pub fn command_names(&self) -> Vec<String> {
        self.cmds.keys().cloned().collect()
    }

    pub fn callback_prefixes(&self) -> Vec<String> {
        self.cbs.iter().map(|(p, _)| p.clone()).collect()
    }

 /// Runs one bus event through the registered handlers.
    pub fn dispatch(&self, ev: &Event, bot: &mut dyn Bot) {
        match ev {
            Event::Command { name, m } => {
                if let Some(h) = self.cmds.get(name) {
                    h(bot, m);
                }
            }
            Event::Callback { prefix, cb } => {
                for (p, h) in &self.cbs {
                    if p == prefix {
                        h(bot, cb);
                        return;
                    }
                }
            }
            Event::Message(m) => {
                for h in &self.msgs {
                    h(bot, m);
                }
            }
        }
    }
}

/// One queued update waiting for the run loop (see module deviation note).
#[derive(Clone, Debug)]
pub enum Event {
    Command { name: String, m: TgMessage },
    Callback { prefix: String, cb: TgCallback },
    Message(TgMessage),
}

/// Sender half of the update bus; cloned into every mtproto registration.
#[derive(Clone)]
pub struct Bus {
    tx: Sender<Event>,
}

impl Bus {
    fn new(tx: Sender<Event>) -> Bus {
        Bus { tx }
    }

    pub fn send(&self, ev: Event) {
        let _ = self.tx.send(ev);
    }
}

/// Creates a bus and the receiver the run loop drains with `pump`.
pub fn bus() -> (Bus, Receiver<Event>) {
    let (tx, rx) = mpsc::channel();
    (Bus::new(tx), rx)
}

/// Drains every queued update through the registry (run-loop entry point).
pub fn pump(bot: &mut dyn Bot, rx: &Receiver<Event>) {
    while let Ok(ev) = rx.try_recv() {
        if let Some(reg) = REGISTRY.get() {
            reg.dispatch(&ev, bot);
        }
    }
}

/// Registry the bus dispatches through, installed by `init`.
static REGISTRY: OnceLock<Registry> = OnceLock::new();

/// Registry accessor for tests and later init callers.
pub fn registry() -> Option<&'static Registry> {
    REGISTRY.get()
}

/// init: install globals, register every command/callback
/// group onto the mtproto client, returning the bus receiver the run loop
/// drains. Registration order follows the file; later tasks add the
/// remaining groups (play family, vc control, admin, playlists, ...).
pub fn init(
    tg: &mut FerogramBot,
    assistants: Arc<AssistantManager>,
    rooms: Rooms,
    loc: Locales,
    cfg: Config,
) -> Receiver<Event> {
    let _ = LOC.set(Arc::new(loc));
    set_cfg(cfg);
    set_assistants(assistants);
    set_rooms(rooms);

    let mut reg = Registry::default();
    basic::register(&mut reg);
    play::register(&mut reg);
    queuecmd::register(&mut reg);
    skip::register(&mut reg);
    pause::register(&mut reg);
    stop::register(&mut reg);
    vc::register(&mut reg);
    roomcb::register(&mut reg);
    playlists::register(&mut reg);
    settings_cb::register(&mut reg);
    events::register(&mut reg);
    admin::register(&mut reg);
    moderation::register(&mut reg);
    lists::register(&mut reg);
    system::register(&mut reg);
    sticker::register(&mut reg);
    let _ = REGISTRY.set(reg);

    let (bus, rx) = bus();
    if let Some(reg) = REGISTRY.get() {
        for name in reg.command_names() {
            let bus = bus.clone();
            let moved = name.clone();
            tg.on_command(&name, move |m| {
                bus.send(Event::Command { name: moved.clone(), m: m.clone() });
            });
        }
        for prefix in reg.callback_prefixes() {
            let bus = bus.clone();
            let moved = prefix.clone();
            tg.on_callback(&prefix, move |cb| {
                bus.send(Event::Callback { prefix: moved.clone(), cb: cb.clone() });
            });
        }
        {
            let bus = bus.clone();
            tg.on_message(move |m| {
                bus.send(Event::Message(m.clone()));
            });
        }
    }
    rx
}
