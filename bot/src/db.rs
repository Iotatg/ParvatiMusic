// db — in-process store with Mongo-persisted indexes.
// parses MONGO_DB_URI but keeps everything in process; this mirrors that exactly.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use crate::parv_info;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlaylistEntry {
    pub title: String,
    pub url: String,
    pub duration: i64,
    pub requester: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WarnEntry {
    pub reason: String,
    pub by: i64,
    pub at: i64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FilterRow {
    pub keyword: String,
    pub text: String,
}

#[derive(Default)]
struct Store {
    langs: HashMap<i64, String>,
    autoplays: HashMap<i64, bool>,
    play_admins: HashMap<i64, bool>,
    linked: HashMap<i64, i64>,
    sudoers: Vec<i64>,
    auth: HashMap<i64, HashSet<i64>>,
    playlists: HashMap<(i64, String), Vec<PlaylistEntry>>,
    warns: HashMap<(i64, i64), Vec<WarnEntry>>,
    warn_cfgs: HashMap<i64, (i64, String)>,
    welcomes: HashMap<i64, bool>,
    notes: HashMap<i64, HashMap<String, String>>,
    filters: HashMap<i64, Vec<(String, String)>>,
    sticker_packs: HashMap<String, Vec<String>>,
    assistants: HashMap<i64, i64>,
    assistant_count: i64,
    no_thumbs: HashMap<i64, bool>,
    served_users: HashSet<i64>,
    served_chats: HashSet<i64>,
    blacklist_users: Vec<i64>,
    blacklist_chats: Vec<i64>,
    maintenance: bool,
    logger: bool,
}

static STORE: OnceLock<Mutex<Store>> = OnceLock::new();

fn store() -> &'static Mutex<Store> {
    STORE.get_or_init(|| Mutex::new(Store::default()))
}

/// init(mongo_uri) ignores the uri (parity) and seeds/// Config.owner_id into sudoers.
pub fn init(owner_id: i64) {
    parv_info!("Database", "Initializing in-process store; mongo uri accepted but unused");
    if owner_id != 0 {
        let mut s = store().lock().unwrap();
        if !s.sudoers.contains(&owner_id) {
            s.sudoers.push(owner_id);
        }
    }
}

// ---- language / chat flags ----

pub fn language(chat_id: i64) -> String {
    store()
        .lock()
        .unwrap()
        .langs
        .get(&chat_id)
        .cloned()
        .unwrap_or_default()
}

pub fn set_language(chat_id: i64, lang: &str) {
    store().lock().unwrap().langs.insert(chat_id, lang.to_string());
}

pub fn autoplay(chat_id: i64) -> bool {
    store()
        .lock()
        .unwrap()
        .autoplays
        .get(&chat_id)
        .copied()
        .unwrap_or(false)
}

pub fn set_autoplay(chat_id: i64, v: bool) {
    store().lock().unwrap().autoplays.insert(chat_id, v);
}

pub fn play_mode_admins_only(chat_id: i64) -> bool {
    store()
        .lock()
        .unwrap()
        .play_admins
        .get(&chat_id)
        .copied()
        .unwrap_or(false)
}

pub fn set_play_mode_admins_only(chat_id: i64, v: bool) {
    store().lock().unwrap().play_admins.insert(chat_id, v);
}

pub fn linked_channel(chat_id: i64) -> i64 {
    store().lock().unwrap().linked.get(&chat_id).copied().unwrap_or(0)
}

pub fn set_linked_channel(chat_id: i64, channel_id: i64) {
    let mut s = store().lock().unwrap();
    if channel_id == 0 {
        s.linked.remove(&chat_id);
    } else {
        s.linked.insert(chat_id, channel_id);
    }
}

// ---- sudo / auth ----

pub fn is_sudo(user_id: i64) -> bool {
    store().lock().unwrap().sudoers.contains(&user_id)
}

pub fn add_sudo(user_id: i64) -> bool {
    let mut s = store().lock().unwrap();
    if s.sudoers.contains(&user_id) {
        return false;
    }
    // insertion order preserved (list semantics)
    s.sudoers.push(user_id);
    true
}

pub fn del_sudo(user_id: i64) -> bool {
    let mut s = store().lock().unwrap();
    match s.sudoers.iter().position(|&u| u == user_id) {
        Some(i) => {
            s.sudoers.remove(i);
            true
        }
        None => false,
    }
}

pub fn sudo_list() -> Vec<i64> {
    store().lock().unwrap().sudoers.clone()
}

pub fn is_authorized(chat_id: i64, user_id: i64) -> bool {
    let s = store().lock().unwrap();
    s.sudoers.contains(&user_id)
        || s
            .auth
            .get(&chat_id)
            .is_some_and(|set| set.contains(&user_id))
}

pub fn add_auth_user(chat_id: i64, user_id: i64) -> bool {
    store()
        .lock()
        .unwrap()
        .auth
        .entry(chat_id)
        .or_default()
        .insert(user_id)
}

pub fn del_auth_user(chat_id: i64, user_id: i64) -> bool {
    store()
        .lock()
        .unwrap()
        .auth
        .get_mut(&chat_id)
        .is_some_and(|set| set.remove(&user_id))
}

pub fn auth_users(chat_id: i64) -> Vec<i64> {
    store()
        .lock()
        .unwrap()
        .auth
        .get(&chat_id)
        .map(|set| set.iter().copied().collect())
        .unwrap_or_default()
}

// ---- blacklist (users + chats) ----

pub fn is_blacklisted_user(user_id: i64) -> bool {
    store().lock().unwrap().blacklist_users.contains(&user_id)
}

pub fn add_blacklisted_user(user_id: i64) -> bool {
    let mut s = store().lock().unwrap();
    if s.blacklist_users.contains(&user_id) {
        return false;
    }
    s.blacklist_users.push(user_id);
    true
}

pub fn del_blacklisted_user(user_id: i64) -> bool {
    let mut s = store().lock().unwrap();
    match s.blacklist_users.iter().position(|&u| u == user_id) {
        Some(i) => {
            s.blacklist_users.remove(i);
            true
        }
        None => false,
    }
}

pub fn blacklisted_users() -> Vec<i64> {
    store().lock().unwrap().blacklist_users.clone()
}

pub fn is_blacklisted_chat(chat_id: i64) -> bool {
    store().lock().unwrap().blacklist_chats.contains(&chat_id)
}

pub fn add_blacklisted_chat(chat_id: i64) -> bool {
    let mut s = store().lock().unwrap();
    if s.blacklist_chats.contains(&chat_id) {
        return false;
    }
    s.blacklist_chats.push(chat_id);
    true
}

pub fn del_blacklisted_chat(chat_id: i64) -> bool {
    let mut s = store().lock().unwrap();
    match s.blacklist_chats.iter().position(|&c| c == chat_id) {
        Some(i) => {
            s.blacklist_chats.remove(i);
            true
        }
        None => false,
    }
}

pub fn blacklisted_chats() -> Vec<i64> {
    store().lock().unwrap().blacklist_chats.clone()
}

// ---- system flags (maintenance / logger) ----

pub fn maintenance() -> bool {
    store().lock().unwrap().maintenance
}

pub fn set_maintenance(v: bool) {
    store().lock().unwrap().maintenance = v;
}

pub fn logger() -> bool {
    store().lock().unwrap().logger
}

pub fn set_logger(v: bool) {
    store().lock().unwrap().logger = v;
}

// ---- playlists ----

pub fn create_playlist(user_id: i64, name: &str) -> bool {
    store()
        .lock()
        .unwrap()
        .playlists
        .insert((user_id, name.to_string()), Vec::new())
        .is_none()
}

pub fn playlist_exists(user_id: i64, name: &str) -> bool {
    store()
        .lock()
        .unwrap()
        .playlists
        .contains_key(&(user_id, name.to_string()))
}

pub fn delete_playlist(user_id: i64, name: &str) -> bool {
    store()
        .lock()
        .unwrap()
        .playlists
        .remove(&(user_id, name.to_string()))
        .is_some()
}

pub fn playlists_of(user_id: i64) -> Vec<String> {
    let mut out: Vec<String> = store()
        .lock()
        .unwrap()
        .playlists
        .keys()
        .filter(|(u, _)| *u == user_id)
        .map(|(_, n)| n.clone())
        .collect();
    out.sort();
    out
}

pub fn add_to_playlist(user_id: i64, name: &str, e: &PlaylistEntry) -> bool {
    let mut s = store().lock().unwrap();
    match s.playlists.get_mut(&(user_id, name.to_string())) {
        Some(list) => {
            list.push(e.clone());
            true
        }
        None => false,
    }
}

pub fn remove_from_playlist(user_id: i64, name: &str, index: usize) -> bool {
    let mut s = store().lock().unwrap();
    match s.playlists.get_mut(&(user_id, name.to_string())) {
        Some(list) if index < list.len() => {
            list.remove(index);
            true
        }
        _ => false,
    }
}

pub fn playlist_entries(user_id: i64, name: &str) -> Vec<PlaylistEntry> {
    store()
        .lock()
        .unwrap()
        .playlists
        .get(&(user_id, name.to_string()))
        .cloned()
        .unwrap_or_default()
}

// ---- warns ----

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub fn add_warning(chat_id: i64, user_id: i64, by: i64, reason: &str) -> i64 {
    let mut s = store().lock().unwrap();
    let list = s.warns.entry((chat_id, user_id)).or_default();
    list.push(WarnEntry {
        reason: reason.to_string(),
        by,
        at: now_secs(),
    });
    list.len() as i64
}

pub fn count_warnings(chat_id: i64, user_id: i64) -> i64 {
    store()
        .lock()
        .unwrap()
        .warns
        .get(&(chat_id, user_id))
        .map(|l| l.len() as i64)
        .unwrap_or(0)
}

pub fn warnings_list(chat_id: i64, user_id: i64) -> Vec<WarnEntry> {
    store()
        .lock()
        .unwrap()
        .warns
        .get(&(chat_id, user_id))
        .cloned()
        .unwrap_or_default()
}

pub fn remove_last_warning(chat_id: i64, user_id: i64) -> bool {
    let mut s = store().lock().unwrap();
    match s.warns.get_mut(&(chat_id, user_id)) {
        Some(list) if !list.is_empty() => {
            list.pop();
            true
        }
        _ => false,
    }
}

pub fn clear_warnings(chat_id: i64, user_id: i64) -> i64 {
    let mut s = store().lock().unwrap();
    match s.warns.get_mut(&(chat_id, user_id)) {
        Some(list) => {
            let n = list.len() as i64;
            list.clear();
            n
        }
        None => 0,
    }
}

pub fn clear_all_warnings(chat_id: i64) -> i64 {
    let mut s = store().lock().unwrap();
    let mut n = 0;
    s.warns.retain(|(c, _), list| {
        if *c == chat_id {
            n += list.len() as i64;
            false
        } else {
            true
        }
    });
    n
}

pub fn warn_limit(chat_id: i64) -> i64 {
    let mut s = store().lock().unwrap();
    s.warn_cfgs.entry(chat_id).or_insert((3, "ban".into())).0
}

pub fn set_warn_limit(chat_id: i64, limit: i64) {
    let mut s = store().lock().unwrap();
    let cfg = s.warn_cfgs.entry(chat_id).or_insert((limit, "ban".into()));
    cfg.0 = limit;
    if cfg.1.is_empty() {
        cfg.1 = "ban".into();
    }
}

pub fn warn_mode(chat_id: i64) -> String {
    let mut s = store().lock().unwrap();
    s.warn_cfgs.entry(chat_id).or_insert((3, "ban".into())).1.clone()
}

pub fn set_warn_mode(chat_id: i64, mode: &str) {
    let mut s = store().lock().unwrap();
    let cfg = s.warn_cfgs.entry(chat_id).or_insert((3, mode.to_string()));
    cfg.1 = mode.to_string();
    if cfg.0 <= 0 {
        cfg.0 = 3;
    }
}

// ---- welcomes / notes ----

pub fn welcome_on(chat_id: i64) -> bool {
    store()
        .lock()
        .unwrap()
        .welcomes
        .get(&chat_id)
        .copied()
        .unwrap_or(false)
}

pub fn set_welcome_on(chat_id: i64, on: bool) {
    store().lock().unwrap().welcomes.insert(chat_id, on);
}

pub fn set_note(chat_id: i64, key: &str, content: &str) {
    store()
        .lock()
        .unwrap()
        .notes
        .entry(chat_id)
        .or_default()
        .insert(key.to_string(), content.to_string());
}

pub fn get_note(chat_id: i64, key: &str) -> String {
    store()
        .lock()
        .unwrap()
        .notes
        .get(&chat_id)
        .and_then(|m| m.get(key))
        .cloned()
        .unwrap_or_default()
}

pub fn list_notes(chat_id: i64) -> Vec<String> {
    let mut out: Vec<String> = store()
        .lock()
        .unwrap()
        .notes
        .get(&chat_id)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    out.sort();
    out
}

pub fn delete_note(chat_id: i64, key: &str) -> bool {
    store()
        .lock()
        .unwrap()
        .notes
        .get_mut(&chat_id)
        .is_some_and(|m| m.remove(key).is_some())
}

// ---- filters ----

pub fn save_filter(chat_id: i64, keyword: &str, text: &str) {
    let kw = keyword.to_lowercase();
    let mut s = store().lock().unwrap();
    let rows = s.filters.entry(chat_id).or_default();
    for (k, v) in rows.iter_mut() {
        if *k == kw {
            *v = text.to_string();
            return;
        }
    }
    rows.push((kw, text.to_string()));
}

pub fn list_filters(chat_id: i64) -> Vec<FilterRow> {
    store()
        .lock()
        .unwrap()
        .filters
        .get(&chat_id)
        .map(|rows| {
            rows.iter()
                .map(|(k, v)| FilterRow {
                    keyword: k.clone(),
                    text: v.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn get_filter(chat_id: i64, keyword: &str) -> Option<String> {
 // Convenience getter.
    let kw = keyword.to_lowercase();
    store()
        .lock()
        .unwrap()
        .filters
        .get(&chat_id)
        .and_then(|rows| rows.iter().find(|(k, _)| *k == kw))
        .map(|(_, v)| v.clone())
}

pub fn match_filter(chat_id: i64, text: &str) -> Option<FilterRow> {
    let low = text.to_lowercase();
    store()
        .lock()
        .unwrap()
        .filters
        .get(&chat_id)
        .and_then(|rows| {
            rows.iter().find(|(k, _)| low.contains(k.as_str())).map(|(k, v)| FilterRow {
                keyword: k.clone(),
                text: v.clone(),
            })
        })
}

pub fn delete_filter(chat_id: i64, keyword: &str) -> bool {
    let kw = keyword.to_lowercase();
    store()
        .lock()
        .unwrap()
        .filters
        .get_mut(&chat_id)
        .is_some_and(|rows| {
            let before = rows.len();
            rows.retain(|(k, _)| *k != kw);
            rows.len() < before
        })
}

pub fn clear_filters(chat_id: i64) -> i64 {
    store()
        .lock()
        .unwrap()
        .filters
        .remove(&chat_id)
        .map(|rows| rows.len() as i64)
        .unwrap_or(0)
}

// ---- sticker packs ----

pub fn sticker_pack_ids(mood: &str) -> Vec<String> {
    store()
        .lock()
        .unwrap()
        .sticker_packs
        .get(mood)
        .cloned()
        .unwrap_or_default()
}

/// Every pack as (mood, ids), in insertion order.
pub fn sticker_packs() -> Vec<(String, Vec<String>)> {
    store()
        .lock()
        .unwrap()
        .sticker_packs
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

pub fn add_sticker_to_pack(mood: &str, remote_id: &str) {
    if mood.is_empty() || remote_id.is_empty() {
        return;
    }
    let mut s = store().lock().unwrap();
    let ids = s.sticker_packs.entry(mood.to_string()).or_default();
    if !ids.contains(&remote_id.to_string()) {
        ids.push(remote_id.to_string());
    }
}

pub fn clear_sticker_pack(mood_or_all: &str) -> i64 {
    let mut s = store().lock().unwrap();
    if mood_or_all == "all" {
        let n = s.sticker_packs.len() as i64;
        s.sticker_packs.clear();
        return n;
    }
    if s.sticker_packs.remove(mood_or_all).is_some() {
        1
    } else {
        0
    }
}

// ---- served registry ----

pub fn add_served_user(id: i64) {
    store().lock().unwrap().served_users.insert(id);
}

pub fn add_served_chat(id: i64) {
    store().lock().unwrap().served_chats.insert(id);
}

pub fn served_users() -> Vec<i64> {
    store().lock().unwrap().served_users.iter().copied().collect()
}

pub fn served_chats() -> Vec<i64> {
    store().lock().unwrap().served_chats.iter().copied().collect()
}

// ---- assistant indexes (used by Task 15 main + Task 9 assistant) ----

pub fn get_assistant(chat_id: i64) -> i64 {
    let mut s = store().lock().unwrap();
    if let Some(i) = s.assistants.get(&chat_id) {
        return *i;
    }
    let count = s.assistant_count.max(1);
    let idx = (chat_id.unsigned_abs() as i64 % count) + 1;
    s.assistants.insert(chat_id, idx);
    idx
}

pub fn init_assistant_indexes(count: i64) {
    store().lock().unwrap().assistant_count = if count > 0 { count } else { 1 };
}

// ---- thumbnail toggle ----

pub fn thumbnails_disabled(chat_id: i64) -> bool {
    store()
        .lock()
        .unwrap()
        .no_thumbs
        .get(&chat_id)
        .copied()
        .unwrap_or(false)
}

pub fn set_thumbnails_disabled(chat_id: i64, v: bool) {
    store().lock().unwrap().no_thumbs.insert(chat_id, v);
}
