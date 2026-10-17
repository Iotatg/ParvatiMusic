//! Assistant manager.
//!
//! One assistant per STRING_SESSIONS entry: logs a parse failure and
//! keeps the slot with a null session Context, so `db::get_assistant`'s
//! modulo counts stay stable for every chat.
use std::sync::Mutex;

use crate::config::Config;
use crate::context::{Context, NtgBinding, NO_SESSION_ERR};
use crate::logger::Logger;

pub struct Assistant {
 /// 1-based position.
    pub index: usize,
    /// user_id from the session string, 0 when parsing failed.
    pub user_id: i64,
    /// Interior-mutable: the stream-end thread and the room watchdog both
 /// drive joins through a shared `&AssistantManager` (keeps assistants
    /// behind a global and single-threads the handlers).
    ntg: Mutex<Context>,
}

impl Assistant {
    pub fn new(index: usize, ntg: Context) -> Self {
        Assistant { index, user_id: 0, ntg: Mutex::new(ntg) }
    }

    pub fn ntg(&self) -> std::sync::MutexGuard<'_, Context> {
        self.ntg.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn mt(&self) -> Option<&parvati_mtproto::client::Client> {
        None
    }

 /// The error surfaced by play on a session-less assistant.
    pub fn session_error() -> String {
        NO_SESSION_ERR.to_string()
    }
}

pub struct AssistantManager {
    list: Vec<Assistant>,
}

impl AssistantManager {
 /// One manager over every session string: unparsable ones keep the slot
    /// with a null session.
    pub fn from_sessions(cfg: &Config) -> Result<AssistantManager, String> {
        let mut list = Vec::with_capacity(cfg.string_sessions.len());
        for (i, raw) in cfg.string_sessions.iter().enumerate() {
            let index = i + 1;
            match parvati_mtproto::session::parse_session_string(raw) {
                Ok(ses) => {
                    let user_id = ses.user_id;
                        Logger::instance().log(
                            crate::logger::LogLevel::Info,
                            "main",
                            &format!("assistant #{index} user id {user_id}"),
                        );
                        let mt = parvati_mtproto::client::Client::new(
                            ses,
                            parvati_mtproto::client::ClientConfig::default(),
                        );
                        let last = list.len();
                        list.push(Assistant::new(
                            index,
                            Context::new(Some(mt), Box::new(NtgBinding::new())),
                        ));
                        list[last].user_id = user_id;
                    }
                    Err(e) => {
                        Logger::instance().log(
                            crate::logger::LogLevel::Error,
                            "main",
                            &format!("assistant session parse failed: {e}"),
                        );
                        list.push(Assistant::new(index, Context::new(None, Box::new(NtgBinding::new()))));
                    }
                }
        }
        Ok(AssistantManager { list })
    }

 /// Boot-failure path: no sessions at all (mirrors 's default-constructed
    /// manager before any assistant was added).
    pub fn empty() -> Self {
        AssistantManager { list: Vec::new() }
    }

 /// AssistantManager::add: appends a prepared assistant (tests and the
    /// harness clients build their own Context).
    pub fn add(&mut self, a: Assistant) {
        self.list.push(a);
    }

    pub fn count(&self) -> usize {
        self.list.len()
    }

    pub fn get(&self, idx: usize) -> Option<&Assistant> {
        if idx == 0 || idx > self.list.len() {
            return None;
        }
        self.list.get(idx - 1)
    }

    pub fn first(&self) -> Option<&Assistant> {
        self.get(1)
    }

 /// for_chat: db-stored per-chat index (1-based), falling back to the
    /// first assistant when the stored index is out of range.
    pub fn for_chat(&self, chat_id: i64) -> Option<&Assistant> {
        if self.list.is_empty() {
            return None;
        }
        let idx = crate::db::get_assistant(chat_id);
        self.get(idx as usize).or_else(|| self.first())
    }

    pub fn for_each(&self, mut f: impl FnMut(&Assistant)) {
        for a in &self.list {
            f(a);
        }
    }

    pub fn for_each_mut(&mut self, mut f: impl FnMut(&mut Assistant)) {
        for a in &mut self.list {
            f(a);
        }
    }
}
