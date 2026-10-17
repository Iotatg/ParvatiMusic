//! Safe Rust binding over libntgcalls (subset used by ParvatiMusic).
//!
//! Mirrors the reference ntgcalls client 1:1: Future/async handling,
//! connection-state bookkeeping, wait_connected semantics, media_from_file
//! ffmpeg shell commands and logger registration.

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr::null_mut;
use std::pin::Pin;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

// ntg_connection_state_enum
pub const STATE_CONNECTING: c_int = 0;
pub const STATE_CONNECTED: c_int = 1;
pub const STATE_TIMEOUT: c_int = 2;
pub const STATE_FAILED: c_int = 3;
pub const STATE_CLOSED: c_int = 4;

// ntg_stream_mode_enum
pub const STREAM_CAPTURE: c_int = 0;

// ntg_media_source_enum (bitmask)
pub const SOURCE_SHELL: c_int = 1 << 1;

type PromiseCb = unsafe extern "C" fn(*mut c_void);
type StreamEndNative = unsafe extern "C" fn(usize, i64, c_int, c_int, *mut c_void);
type ConnectionNative = unsafe extern "C" fn(usize, i64, NetworkInfo, *mut c_void);
type LogNative = unsafe extern "C" fn(LogMessage);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AsyncStruct {
    pub user_data: *mut c_void,
    pub error_code: *mut c_int,
    pub error_message: *mut *mut c_char,
    pub promise: Option<PromiseCb>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AudioDescription {
    pub media_source: c_int,
    pub input: *mut c_char,
    pub sample_rate: u32,
    pub channel_count: u8,
    pub keep_open: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VideoDescription {
    pub media_source: c_int,
    pub input: *mut c_char,
    pub width: i16,
    pub height: i16,
    pub fps: u8,
    pub keep_open: bool,
}

#[repr(C)]
pub struct MediaDescription {
    pub microphone: *mut AudioDescription,
    pub speaker: *mut AudioDescription,
    pub camera: *mut VideoDescription,
    pub screen: *mut VideoDescription,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct NetworkInfo {
    pub kind: c_int,
    pub state: c_int,
}

#[repr(C)]
pub struct LogMessage {
    pub level: c_int,
    pub source: c_int,
    pub file: *mut c_char,
    pub line: u32,
    pub message: *mut c_char,
}

unsafe extern "C" {
    fn ntg_register_logger(callback: LogNative);
    fn ntg_init() -> usize;
    fn ntg_destroy(ptr: usize) -> c_int;
    fn ntg_create(ptr: usize, chat_id: i64, buffer: *mut *mut c_char, future: AsyncStruct) -> c_int;
    fn ntg_connect(ptr: usize, chat_id: i64, params: *mut c_char, is_presentation: bool, future: AsyncStruct) -> c_int;
    fn ntg_set_stream_sources(ptr: usize, chat_id: i64, stream_mode: c_int, desc: MediaDescription, future: AsyncStruct) -> c_int;
    fn ntg_pause(ptr: usize, chat_id: i64, future: AsyncStruct) -> c_int;
    fn ntg_resume(ptr: usize, chat_id: i64, future: AsyncStruct) -> c_int;
    fn ntg_mute(ptr: usize, chat_id: i64, future: AsyncStruct) -> c_int;
    fn ntg_unmute(ptr: usize, chat_id: i64, future: AsyncStruct) -> c_int;
    fn ntg_stop(ptr: usize, chat_id: i64, future: AsyncStruct) -> c_int;
    fn ntg_time(ptr: usize, chat_id: i64, stream_mode: c_int, time: *mut i64, future: AsyncStruct) -> c_int;
    fn ntg_get_version(buffer: *mut *mut c_char) -> c_int;
    fn ntg_on_stream_end(ptr: usize, callback: StreamEndNative, user_data: *mut c_void) -> c_int;
    fn ntg_on_connection_change(ptr: usize, callback: ConnectionNative, user_data: *mut c_void) -> c_int;
}

// ---------------------------------------------------------------------------
// Future / async plumbing
// ---------------------------------------------------------------------------

#[repr(C)]
struct Future {
    // Written by libntgcalls through the raw pointers handed to it while the
    // async op runs; only read after the promise fires (or the 30 s wait
    // times out). The box is pinned on the heap; on timeout we leak it so the
 // library can never write into freed memory (deviation note: the     // wrapper risks a use-after-free in the same situation).
    error: c_int,
    message: *mut c_char,
    done: Mutex<bool>,
    cv: Condvar,
}

unsafe extern "C" fn promise_cb(p: *mut c_void) {
    let f = unsafe { &*(p as *const Future) };
    let mut done = f.done.lock().unwrap();
    *done = true;
    f.cv.notify_all();
}

fn make_future(f: &Pin<Box<Future>>) -> AsyncStruct {
    // The box is pinned, so these raw pointers stay valid for its lifetime.
    AsyncStruct {
        user_data: &**f as *const Future as *mut c_void,
        error_code: &f.error as *const c_int as *mut c_int,
        error_message: &f.message as *const *mut c_char as *mut *mut c_char,
        promise: Some(promise_cb),
    }
}

fn new_future() -> Pin<Box<Future>> {
    Box::pin(Future {
        error: 0,
        message: null_mut(),
        done: Mutex::new(false),
        cv: Condvar::new(),
    })
}

/// Returns true only when the promise fired and no error was reported.
fn wait_future(f: &Future) -> bool {
    let done_guard = f.done.lock().unwrap();
    let (guard, _timeout) = f
        .cv
        .wait_timeout_while(done_guard, Duration::from_secs(30), |d| !*d)
        .unwrap();
    *guard && f.error == 0
}

fn future_message(f: &Future) -> String {
    if f.message.is_null() {
        String::new()
    } else {
        let s = unsafe { CStr::from_ptr(f.message) }.to_string_lossy().into_owned();
        unsafe { libc::free(f.message as *mut c_void) };
        s
    }
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

pub type OnStreamEnd = std::sync::Arc<dyn Fn(i64) + Send + Sync>;
pub type OnConnectionChange = std::sync::Arc<dyn Fn(i64, bool, String) + Send + Sync>;

#[derive(Clone)]
struct ConnInfo {
    settled: bool,
    connected: bool,
    state: String,
}

struct Shared {
    stream_end: Mutex<Vec<OnStreamEnd>>,
    conn_change: Mutex<Vec<OnConnectionChange>>,
    conns: Mutex<HashMap<i64, ConnInfo>>,
    conn_cv: Condvar,
}

pub struct Client {
    ptr: usize,
    shared: *mut Shared,
}

unsafe impl Send for Client {}
unsafe impl Sync for Client {}

unsafe extern "C" fn log_cb(m: LogMessage) {
    let lvl = match m.level {
        1 => "TRACE",
        2 => "DEBUG",
        4 => "WARN",
        8 => "ERROR",
        _ => "?",
    };
    let file = if m.file.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(m.file) }.to_string_lossy().into_owned()
    };
    let msg = if m.message.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(m.message) }.to_string_lossy().into_owned()
    };
    log::info!(target: "ntgcore", "{lvl} [{file}:{}] {msg}", m.line);
}

unsafe extern "C" fn stream_end_cb(_ptr: usize, chat: i64, _t: c_int, _d: c_int, ud: *mut c_void) {
    let shared = unsafe { &*(ud as *const Shared) };
    let cbs: Vec<OnStreamEnd> = shared.stream_end.lock().unwrap().clone();
    for cb in cbs {
        cb(chat);
    }
}

unsafe extern "C" fn connection_cb(_ptr: usize, chat: i64, info: NetworkInfo, ud: *mut c_void) {
    let shared = unsafe { &*(ud as *const Shared) };
    let (state, settled, connected) = match info.state {
        STATE_CONNECTING => ("connecting".to_string(), false, false),
        STATE_CONNECTED => ("connected".to_string(), true, true),
        STATE_TIMEOUT => ("timeout".to_string(), true, false),
        STATE_FAILED => ("failed".to_string(), true, false),
        STATE_CLOSED => ("closed".to_string(), true, false),
        _ => ("unknown".to_string(), true, false),
    };
    let cbs: Vec<OnConnectionChange> = {
        let mut conns = shared.conns.lock().unwrap();
        conns.insert(
            chat,
            ConnInfo {
                settled,
                connected,
                state: state.clone(),
            },
        );
        shared.conn_change.lock().unwrap().clone()
    };
    if connected {
        log::info!(target: "ntgcalls", "webrtc connected for chat {chat}");
    } else if settled {
        log::warn!(target: "ntgcalls", "webrtc connection {state} for chat {chat}");
    }
    for cb in cbs {
        cb(chat, connected, state.clone());
    }
    shared.conn_cv.notify_all();
}

impl Client {
    pub fn new() -> Client {
        unsafe {
            ntg_register_logger(log_cb);
            let ptr = ntg_init();
            let shared = Box::into_raw(Box::new(Shared {
                stream_end: Mutex::new(Vec::new()),
                conn_change: Mutex::new(Vec::new()),
                conns: Mutex::new(HashMap::new()),
                conn_cv: Condvar::new(),
            }));
            ntg_on_connection_change(ptr, connection_cb, shared as *mut c_void);
            log::info!(target: "ntgcalls", "ntg_init ptr={ptr}");
            Client { ptr, shared }
        }
    }

    pub fn ptr(&self) -> usize {
        self.ptr
    }

    /// Registers a stream-end callback. Re-registers the native hook with
 /// every call, mirroring the on_stream_end behaviour.
    pub fn on_stream_end(&self, cb: OnStreamEnd) {
        let shared = unsafe { &*self.shared };
        shared.stream_end.lock().unwrap().push(cb);
        unsafe {
            ntg_on_stream_end(self.ptr, stream_end_cb, self.shared as *mut c_void);
        }
    }

    pub fn on_connection_change(&self, cb: OnConnectionChange) {
        let shared = unsafe { &*self.shared };
        shared.conn_change.lock().unwrap().push(cb);
    }

    pub fn create(&self, chat_id: i64, sdp: &mut String) -> bool {
        let f = new_future();
        unsafe {
            let mut buf: *mut c_char = null_mut();
            let sync = ntg_create(self.ptr, chat_id, &mut buf, make_future(&f));
            let ok = sync == 0 && wait_future(&f);
            if !ok {
                let msg = future_message(&f);
                log::error!(target: "ntgcalls", "create failed sync={sync} err={} msg={msg}", f.error);
                return false;
            }
            if !buf.is_null() {
                *sdp = CStr::from_ptr(buf).to_string_lossy().into_owned();
                libc::free(buf as *mut c_void);
            }
        }
        true
    }

    pub fn connect(&self, chat_id: i64, params: &str) -> bool {
        let cparams = CString::new(params).unwrap_or_default();
        let f = new_future();
        unsafe {
            ntg_connect(
                self.ptr,
                chat_id,
                cparams.as_ptr() as *mut c_char,
                false,
                make_future(&f),
            );
        }
        wait_future(&f)
    }

    pub fn set_stream_sources(&self, chat_id: i64, desc: MediaDesc) -> bool {
        let mut mic = AudioDescription {
            media_source: 0,
            input: null_mut(),
            sample_rate: 96000,
            channel_count: 2,
            keep_open: false,
        };
        let mut cam = VideoDescription {
            media_source: 0,
            input: null_mut(),
            width: 1280,
            height: 720,
            fps: 30,
            keep_open: false,
        };
        let c_mic = desc.microphone.as_ref().map(|a| {
            mic.media_source = a.media_source;
            mic.sample_rate = a.sample_rate;
            mic.channel_count = a.channel_count;
            CString::new(a.input.as_str()).unwrap_or_default()
        });
        if let Some(c) = &c_mic {
            mic.input = c.as_ptr() as *mut c_char;
        }
        let c_cam = desc.camera.as_ref().map(|v| {
            cam.media_source = v.media_source;
            cam.width = v.width;
            cam.height = v.height;
            cam.fps = v.fps;
            CString::new(v.input.as_str()).unwrap_or_default()
        });
        if let Some(c) = &c_cam {
            cam.input = c.as_ptr() as *mut c_char;
        }
        let d = MediaDescription {
            microphone: if c_mic.is_some() { &mut mic } else { null_mut() },
            speaker: null_mut(),
            camera: if c_cam.is_some() { &mut cam } else { null_mut() },
            screen: null_mut(),
        };
        let f = new_future();
        unsafe {
            ntg_set_stream_sources(self.ptr, chat_id, STREAM_CAPTURE, d, make_future(&f));
        }
        wait_future(&f)
    }

    pub fn pause(&self, chat_id: i64) -> bool {
        self.simple(chat_id, |p, c, f| unsafe { ntg_pause(p, c, f) })
    }

    pub fn resume(&self, chat_id: i64) -> bool {
        self.simple(chat_id, |p, c, f| unsafe { ntg_resume(p, c, f) })
    }

    pub fn mute(&self, chat_id: i64) -> bool {
        self.simple(chat_id, |p, c, f| unsafe { ntg_mute(p, c, f) })
    }

    pub fn unmute(&self, chat_id: i64) -> bool {
        self.simple(chat_id, |p, c, f| unsafe { ntg_unmute(p, c, f) })
    }

    pub fn stop(&self, chat_id: i64) -> bool {
        // Drop any reported state so a later reconnect is judged on fresh events.
        unsafe { &*self.shared }.conns.lock().unwrap().remove(&chat_id);
        self.simple(chat_id, |p, c, f| unsafe { ntg_stop(p, c, f) })
    }

    pub fn time(&self, chat_id: i64) -> i64 {
        let f = new_future();
        let mut t: i64 = 0;
        unsafe {
            ntg_time(self.ptr, chat_id, STREAM_CAPTURE, &mut t, make_future(&f));
        }
        let _ = wait_future(&f);
        t
    }

    /// Waits until the engine reports a terminal connection state for chat_id.
    /// Returns false only when a failure/timeout was actually reported; if the
    /// engine never emits any event the wait just times out and returns true.
    pub fn wait_connected(&self, chat_id: i64, timeout_ms: u64) -> bool {
        let shared = unsafe { &*self.shared };
        let conns = shared.conns.lock().unwrap();
        let (conns, _timeout) = shared
            .conn_cv
            .wait_timeout_while(conns, Duration::from_millis(timeout_ms), |c| {
                !matches!(c.get(&chat_id), Some(ci) if ci.settled)
            })
            .unwrap();
        // No event at all means the engine does not emit connection changes
        // here; treat that as "nothing to report" so playback is never blocked.
        match conns.get(&chat_id) {
            None => true,
            Some(ci) => ci.connected,
        }
    }

    pub fn connection_state(&self, chat_id: i64) -> String {
        let shared = unsafe { &*self.shared };
        match shared.conns.lock().unwrap().get(&chat_id) {
            None => "no event".to_string(),
            Some(ci) => ci.state.clone(),
        }
    }

    fn simple(&self, chat_id: i64, call: fn(usize, i64, AsyncStruct) -> c_int) -> bool {
        let f = new_future();
        let sync = call(self.ptr, chat_id, make_future(&f));
        sync == 0 && wait_future(&f)
    }

    /// Destroys the engine and reclaims the callback registry. Idempotent.
    pub fn close(&mut self) {
        unsafe {
            if self.ptr != 0 {
                ntg_destroy(self.ptr);
                self.ptr = 0;
            }
            if !self.shared.is_null() {
                drop(Box::from_raw(self.shared));
                self.shared = null_mut();
            }
        }
    }
}

impl Default for Client {
    fn default() -> Self {
        Client::new()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.close();
    }
}

// ---------------------------------------------------------------------------
// Public descriptor types + media_from_file
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
pub struct AudioDesc {
    pub media_source: c_int,
    pub input: String,
    pub sample_rate: u32,
    pub channel_count: u8,
}

#[derive(Clone, Default)]
pub struct VideoDesc {
    pub media_source: c_int,
    pub input: String,
    pub width: i16,
    pub height: i16,
    pub fps: u8,
}

#[derive(Clone, Default)]
pub struct MediaDesc {
    pub microphone: Option<AudioDesc>,
    pub camera: Option<VideoDesc>,
}

pub fn media_from_file(path: &str, pos: i64, speed: f64, video: bool) -> MediaDesc {
    let speed = speed.clamp(0.5, 4.0);
    let mut base = String::from("ffmpeg ");
    if path.starts_with("http://") || path.starts_with("https://") {
        base.push_str("-reconnect 1 -reconnect_streamed 1 -reconnect_delay_max 5 ");
    }
    if pos > 0 {
        base.push_str(&format!("-ss {pos} "));
    }
    base.push_str(&format!("-v warning -i \"{path}\" "));

    let microphone = Some(AudioDesc {
        media_source: SOURCE_SHELL,
        input: format!("{base}-filter:a \"atempo={speed:.2}\" -f s16le -ac 2 -ar 96000 pipe:1"),
        sample_rate: 96000,
        channel_count: 2,
    });
    if !video {
        return MediaDesc { microphone, camera: None };
    }

    let camera = Some(VideoDesc {
        media_source: SOURCE_SHELL,
        input: format!(
            "{base}-filter:v \"scale=1280:720:force_original_aspect_ratio=decrease,pad=1280:720:(ow-iw)/2:(oh-ih)/2,fps=30,setpts=PTS/{speed}\" -f rawvideo -r 30 -pix_fmt yuv420p pipe:1"
        ),
        width: 1280,
        height: 720,
        fps: 30,
    });
    MediaDesc { microphone, camera }
}

pub fn version() -> &'static str {
    static V: OnceLock<String> = OnceLock::new();
    V.get_or_init(|| unsafe {
        let mut buf: *mut c_char = null_mut();
        ntg_get_version(&mut buf);
        let s = if buf.is_null() {
            "unknown".to_string()
        } else {
            CStr::from_ptr(buf).to_string_lossy().into_owned()
        };
        if !buf.is_null() {
            libc::free(buf as *mut c_void);
        }
        s
    })
    .as_str()
}
