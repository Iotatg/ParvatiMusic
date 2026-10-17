// room — RoomState queue engine.
// (per-chat playback state machine; ntg calls live in the Task 9 context layer).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::models::{normalize_track_title, Track};

const MIN_SPEED: f64 = 0.50;
const MAX_SPEED: f64 = 4.0;
const SEEK_END_THRESHOLD: i64 = 10;
const SEEK_SAFETY_MARGIN: i64 = 5;
const HISTORY_CAP: usize = 100;

/// RoomState::play outcome: queued behind current playback, or started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayAction {
    Queued,
    Started,
 /// Native streaming failed (play returns false with an err).
    Failed,
}

pub struct RoomState {
    pub id: i64,
    pub chat_id: i64,
    /// Live now-playing card message id (0 = no card on screen).
    pub status_msg: i64,
    pub status_is_photo: bool,

    pub track: Option<Track>,
    pub queue: Vec<Track>,
    pub file_path: String,
    pub playing: bool,
    pub paused: bool,
    pub muted: bool,
    pub speed: f64,
    pub position: i64,
    pub loop_n: i64,
    pub shuffle: bool,
    pub autoplay: bool,
    pub played_history: Vec<String>,
    pub engine_token: String,
    pub updated_at: i64,

    destroyed: AtomicBool,
    advancing: AtomicBool,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

impl RoomState {
    pub fn new(chat_id: i64, autoplay: bool) -> RoomState {
        RoomState {
            id: chat_id,
            chat_id,
            status_msg: 0,
            status_is_photo: false,
            track: None,
            queue: Vec::new(),
            file_path: String::new(),
            playing: false,
            paused: false,
            muted: false,
            speed: 1.0,
            position: 0,
            loop_n: 0,
            shuffle: false,
            autoplay,
            played_history: Vec::new(),
            engine_token: String::new(),
            updated_at: 0,
            destroyed: AtomicBool::new(false),
            advancing: AtomicBool::new(false),
        }
    }

    /// Progress-bar markup view (buttons::play_markup takes this shape).
    pub fn view(&self) -> crate::buttons::RoomView {
        crate::buttons::RoomView {
            id: self.id,
            paused: self.paused,
            autoplay: self.autoplay,
            position: self.position,
            duration: self.track.as_ref().map(|t| t.duration).unwrap_or(0),
        }
    }

    pub fn active(&self) -> bool {
        self.playing && self.track.is_some()
    }

    pub fn destroyed(&self) -> bool {
        self.destroyed.load(Ordering::SeqCst)
    }

    pub fn destroy(&self) {
        self.destroyed.store(true, Ordering::SeqCst);
    }

    pub fn try_lock_advance(&self) -> bool {
        self.advancing
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    pub fn unlock_advance(&self) {
        self.advancing.store(false, Ordering::SeqCst);
    }

 /// add_played_locked: "id:" always, "title:" when normalization is
    /// non-empty; capped at the newest 100 entries.
    fn add_played_locked(&mut self, t: &Track) {
        self.played_history.push(format!("id:{}", t.id));
        let n = normalize_track_title(&t.title);
        if !n.is_empty() {
            self.played_history.push(format!("title:{n}"));
        }
        if self.played_history.len() > HISTORY_CAP {
            let excess = self.played_history.len() - HISTORY_CAP;
            self.played_history.drain(..excess);
        }
    }

 /// Public wrapper for the current track.
    pub fn add_played(&mut self) {
        if let Some(t) = self.track.clone() {
            self.add_played_locked(&t);
        }
    }

 /// RoomState::play rollback on native failure: drop the tentative
    /// track so the retry attempt rebuilds state from scratch.
    pub fn roll_back_failed_start(&mut self) {
        self.track = None;
        self.playing = false;
        self.file_path = String::new();
    }

 /// An empty path still becomes the current track but
    /// is re-queued so the monitor downloads and starts it.
    pub fn play(&mut self, t: Track, path: String, force: bool) -> PlayAction {
        if self.destroyed() {
            return PlayAction::Queued;
        }
        if !force && self.playing && self.track.is_some() {
            self.queue.push(t);
            return PlayAction::Queued;
        }
        if self.track.as_ref().map(|c| c.id != t.id).unwrap_or(true) {
            self.loop_n = 0;
        }
        self.track = Some(t.clone());
        self.add_played_locked(&t);
        self.playing = true;
        self.file_path = path.clone();
        self.position = 0;
        self.paused = false;
        self.muted = false;
        self.updated_at = now_secs();
        if path.is_empty() {
            self.queue.push(t);
        }
        PlayAction::Started
    }

 /// loop first, then queue head (random under shuffle).
    pub fn next_track(&mut self) -> Option<Track> {
        if self.destroyed() {
            return None;
        }
        if self.track.is_some() && self.loop_n > 0 {
            self.position = 0;
            self.playing = false;
            self.paused = false;
            self.muted = false;
            self.loop_n -= 1;
            self.updated_at = now_secs();
            return self.track.clone();
        }
        if self.queue.is_empty() {
            return None;
        }
        let mut index = 0usize;
        if self.shuffle && self.queue.len() > 1 {
            index = rand_below(self.queue.len());
        }
        let next = self.queue.remove(index);
        self.track = Some(next.clone());
        self.add_played_locked(&next);
        self.position = 0;
        self.playing = false;
        self.paused = false;
        self.muted = false;
        self.updated_at = now_secs();
        Some(next)
    }

    pub fn queue(&self) -> Vec<Track> {
        self.queue.clone()
    }

    pub fn clear_queue(&mut self) {
        self.queue.clear();
    }

    pub fn remove_at(&mut self, index: usize) -> bool {
        if index >= self.queue.len() {
            return false;
        }
        self.queue.remove(index);
        true
    }

 /// RoomState::move: reinsert queue[from] at index `to`.
    pub fn move_to(&mut self, from: usize, to: usize) -> bool {
        if from >= self.queue.len() || to >= self.queue.len() {
            return false;
        }
        let t = self.queue.remove(from);
        self.queue.insert(to, t);
        true
    }

 /// RoomState::shuffle: shuffle the queue and enable shuffle mode.
    pub fn shuffle_queue(&mut self) {
        let mut seed = nanos();
        for i in (1..self.queue.len()).rev() {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let j = ((seed >> 33) as usize) % (i + 1);
            self.queue.swap(i, j);
        }
        self.shuffle = true;
    }

    pub fn set_shuffle(&mut self, v: bool) {
        self.shuffle = v;
    }

    pub fn set_loop(&mut self, v: i64) {
        self.loop_n = v;
    }

    pub fn set_autoplay(&mut self, v: bool) {
        self.autoplay = v;
    }

    pub fn file_path(&self) -> String {
        self.file_path.clone()
    }

    pub fn played_history(&self) -> Vec<String> {
        self.played_history.clone()
    }

 /// update_position: seconds weighted by speed, skipped when paused.
    fn update_position(&mut self) {
        if !self.playing || self.paused || self.updated_at == 0 {
            return;
        }
        let now = now_secs();
        let delta = ((now - self.updated_at) as f64 * self.speed) as i64;
        if delta > 0 {
            self.position += delta;
        }
        self.updated_at = now;
    }

    pub fn tick_position(&mut self) {
        self.update_position();
    }

 /// relative seek with end refusal and safety margin.
    pub fn seek(&mut self, seconds: i64) -> Option<i64> {
        if self.destroyed() || self.track.is_none() || self.file_path.is_empty() {
            return None;
        }
        self.update_position();
        let duration = self.track.as_ref().map(|t| t.duration).unwrap_or(0);
        if seconds > 0 && duration - self.position <= SEEK_END_THRESHOLD {
            return None;
        }
        let mut new_pos = self.position + seconds;
        if new_pos >= duration {
            new_pos = duration - SEEK_SAFETY_MARGIN;
        }
        if new_pos < 0 {
            new_pos = 0;
        }
        self.position = new_pos;
        self.paused = false;
        self.muted = false;
        Some(new_pos)
    }

    pub fn set_speed(&mut self, speed: f64) -> Option<f64> {
        if self.destroyed() || speed < MIN_SPEED || speed > MAX_SPEED {
            return None;
        }
        if self.track.is_none() || self.file_path.is_empty() {
            return None;
        }
        if self.speed == speed {
            return Some(speed);
        }
        self.update_position();
        self.speed = speed;
        self.playing = true;
        self.paused = false;
        self.muted = false;
        Some(speed)
    }

    pub fn speed(&self) -> f64 {
        self.speed
    }

    pub fn pause(&mut self) -> bool {
        if self.destroyed() {
            return false;
        }
        if self.paused {
            return true;
        }
        self.update_position();
        self.paused = true;
        self.muted = false;
        true
    }

    pub fn resume(&mut self) -> bool {
        if self.destroyed() {
            return false;
        }
        if !self.active() {
            return false;
        }
        if !self.paused {
            return true;
        }
        self.paused = false;
        self.muted = false;
        self.playing = true;
        self.updated_at = now_secs();
        true
    }

    pub fn replay(&mut self) -> bool {
        if self.destroyed() || self.track.is_none() || self.file_path.is_empty() {
            return false;
        }
        self.position = 0;
        self.paused = false;
        self.muted = false;
        self.playing = true;
        true
    }

    pub fn mute(&mut self) -> bool {
        if self.destroyed() {
            return false;
        }
        if self.muted {
            return true;
        }
        if self.paused {
            self.paused = false;
        }
        self.muted = true;
        true
    }

    pub fn unmute(&mut self) -> bool {
        if self.destroyed() {
            return false;
        }
        self.update_position();
        self.muted = false;
        self.paused = false;
        true
    }

 /// (ntg stop stays in the context layer).
    pub fn stop(&mut self) -> bool {
        if self.destroyed() {
            return false;
        }
        self.track = None;
        self.position = 0;
        self.playing = false;
        self.paused = false;
        self.muted = false;
        self.updated_at = 0;
        self.autoplay = false;
        true
    }
}

/// std::mt19937 seeding parity: SystemTime nanos, xorshift-advanced.
fn nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

fn rand_below(n: usize) -> usize {
    let mut x = nanos() ^ 0x9E3779B97F4A7C15;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    (x as usize) % n
}

/// global rooms map: chat_id -> RoomState (kept unique_ptrs; Rust uses
/// Arc<Mutex<..>> so the context layer shares state with the monitor loop).
#[derive(Default)]
pub struct Rooms {
    map: Arc<Mutex<HashMap<i64, Arc<Mutex<RoomState>>>>>,
}

impl Clone for Rooms {
    /// Shared handle: the map is process-global state, so cloned guards
    /// (monitor thread, stream-end threads) observe the same rooms.
    fn clone(&self) -> Rooms {
        Rooms::new_shared(self.map_handle())
    }
}

impl Rooms {
    pub fn new() -> Rooms {
        Rooms::default()
    }

    fn map_handle(&self) -> Arc<Mutex<HashMap<i64, Arc<Mutex<RoomState>>>>> {
        // Rooms is constructed once per process; the map handle is stored
        // behind an Arc from creation (see new_shared).
        self.map.clone()
    }

    fn new_shared(shared: Arc<Mutex<HashMap<i64, Arc<Mutex<RoomState>>>>>) -> Rooms {
        Rooms { map: shared }
    }

 /// create_room: existing rooms are returned as-is.
    pub fn create_room(&self, chat_id: i64, autoplay: bool) -> Arc<Mutex<RoomState>> {
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(chat_id)
            .or_insert_with(|| Arc::new(Mutex::new(RoomState::new(chat_id, autoplay))))
            .clone()
    }

    pub fn room_for(&self, chat_id: i64) -> Option<Arc<Mutex<RoomState>>> {
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&chat_id)
            .cloned()
    }

    pub fn drop_room(&self, chat_id: i64) -> bool {
        let room = self
            .map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&chat_id);
        if let Some(r) = room {
            let mut g = r.lock().unwrap_or_else(|e| e.into_inner());
            g.stop();
            g.destroy();
            true
        } else {
            false
        }
    }

    pub fn all_rooms(&self) -> Vec<(i64, Arc<Mutex<RoomState>>)> {
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect()
    }

    pub fn count(&self) -> usize {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}
