//! ubot Context — the assistant's session-side voice chat controller.
//!
//! Keeps the MTProto transport and the WebRTC binding (libntgcalls) together
//! and replicates the join/connect sequence exactly:
//! SetStreamSources -> CreateCall -> GetInputGroupCall -> PhoneJoinGroupCall
//! -> Connect(serverParams) -> wait for the connection callback.
use std::any::Any;
use std::collections::HashMap;
use std::sync::mpsc::Sender;

use parvati_mtproto::client::Client as MtClient;
use parvati_ntg::{media_from_file, MediaDesc, OnStreamEnd};

#[allow(unused_imports)]
use crate::{parv_debug, parv_error};

/// Abstraction over libntgcalls so tests (and the monitor) can drive the
/// join state machine without a real voice engine.
pub trait VoiceBinding: Send {
    fn set_stream_sources(&mut self, chat_id: i64, desc: MediaDesc) -> bool;
    fn create(&mut self, chat_id: i64, sdp_out: &mut String) -> bool;
    fn connect(&mut self, chat_id: i64, params: &str) -> bool;
    fn pause(&mut self, chat_id: i64) -> bool;
    fn resume(&mut self, chat_id: i64) -> bool;
    fn mute(&mut self, chat_id: i64) -> bool;
    fn unmute(&mut self, chat_id: i64) -> bool;
    fn stop(&mut self, chat_id: i64) -> bool;
    fn wait_connected(&mut self, chat_id: i64, timeout_ms: u64) -> bool;
    fn connection_state(&mut self, chat_id: i64) -> String;
    fn on_stream_end(&mut self, cb: OnStreamEnd);

    // Lets tests observe and steer concrete doubles without downcasting.
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// Thin 1:1 wrapper over the ntg crate (libntgcalls).
pub struct NtgBinding {
    pub ntg: parvati_ntg::Client,
}

impl NtgBinding {
    pub fn new() -> Self {
        Self { ntg: parvati_ntg::Client::new() }
    }
}

impl Default for NtgBinding {
    fn default() -> Self {
        Self::new()
    }
}

impl VoiceBinding for NtgBinding {
    fn set_stream_sources(&mut self, chat_id: i64, desc: MediaDesc) -> bool {
        self.ntg.set_stream_sources(chat_id, desc)
    }
    fn create(&mut self, chat_id: i64, sdp_out: &mut String) -> bool {
        self.ntg.create(chat_id, sdp_out)
    }
    fn connect(&mut self, chat_id: i64, params: &str) -> bool {
        self.ntg.connect(chat_id, params)
    }
    fn pause(&mut self, chat_id: i64) -> bool {
        self.ntg.pause(chat_id)
    }
    fn resume(&mut self, chat_id: i64) -> bool {
        self.ntg.resume(chat_id)
    }
    fn mute(&mut self, chat_id: i64) -> bool {
        self.ntg.mute(chat_id)
    }
    fn unmute(&mut self, chat_id: i64) -> bool {
        self.ntg.unmute(chat_id)
    }
    fn stop(&mut self, chat_id: i64) -> bool {
        self.ntg.stop(chat_id)
    }
    fn wait_connected(&mut self, chat_id: i64, timeout_ms: u64) -> bool {
        self.ntg.wait_connected(chat_id, timeout_ms)
    }
    fn connection_state(&mut self, chat_id: i64) -> String {
        self.ntg.connection_state(chat_id)
    }
    fn on_stream_end(&mut self, cb: OnStreamEnd) {
        self.ntg.on_stream_end(cb);
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Test double: records every op as `"{op} {chat_id}"` so tests assert the
/// exact call ordering; `fail_create` mirrors a broken webrtc engine.
pub struct FakeBinding {
    pub ops: Vec<String>,
    pub fail_create: bool,
}

impl FakeBinding {
    pub fn new() -> Self {
        Self { ops: Vec::new(), fail_create: false }
    }
    fn record(&mut self, op: &str, chat_id: i64) {
        self.ops.push(format!("{op} {chat_id}"));
    }
}

impl Default for FakeBinding {
    fn default() -> Self {
        Self::new()
    }
}

impl VoiceBinding for FakeBinding {
    fn set_stream_sources(&mut self, chat_id: i64, _desc: MediaDesc) -> bool {
        self.record("set_stream_sources", chat_id);
        true
    }
    fn create(&mut self, chat_id: i64, sdp_out: &mut String) -> bool {
        self.record("create", chat_id);
        if self.fail_create {
            return false;
        }
        sdp_out.push_str("OFFER");
        true
    }
    fn connect(&mut self, chat_id: i64, _params: &str) -> bool {
        self.record("connect", chat_id);
        true
    }
    fn pause(&mut self, chat_id: i64) -> bool {
        self.record("pause", chat_id);
        true
    }
    fn resume(&mut self, chat_id: i64) -> bool {
        self.record("resume", chat_id);
        true
    }
    fn mute(&mut self, chat_id: i64) -> bool {
        self.record("mute", chat_id);
        true
    }
    fn unmute(&mut self, chat_id: i64) -> bool {
        self.record("unmute", chat_id);
        true
    }
    fn stop(&mut self, chat_id: i64) -> bool {
        self.record("stop", chat_id);
        true
    }
    fn wait_connected(&mut self, chat_id: i64, _timeout_ms: u64) -> bool {
        self.record("wait_connected", chat_id);
        true
    }
    fn connection_state(&mut self, chat_id: i64) -> String {
        format!("unknown({chat_id})")
    }
    fn on_stream_end(&mut self, _cb: OnStreamEnd) {}
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

struct Mount {
    joined: bool,
    sending_video: bool,
}

fn wants_video(desc: &MediaDesc) -> bool {
    desc.camera.is_some()
}

/// Assistant-side voice chat controller. A `None` session mirrors the /// layout where an unparsable STRING_SESSION yields a Context with no client
/// (play() then reports the assistant-unavailable error).
pub struct Context {
    binding: Box<dyn VoiceBinding>,
    mt: Option<MtClient>,
    calls: HashMap<i64, Mount>,
    stream_tx: Option<Sender<i64>>,
}

impl Context {
    pub fn new(mt: Option<MtClient>, binding: Box<dyn VoiceBinding>) -> Self {
        Self { binding, mt, calls: HashMap::new(), stream_tx: None }
    }

    pub fn mt(&self) -> Option<&MtClient> {
        self.mt.as_ref()
    }

    pub fn mt_mut(&mut self) -> Option<&mut MtClient> {
        self.mt.as_mut()
    }

    pub fn joined(&self, chat_id: i64) -> bool {
        self.calls.get(&chat_id).map(|m| m.joined).unwrap_or(false)
    }

    pub fn connected(&self, chat_id: i64) -> bool {
        self.calls.get(&chat_id).map(|c| c.joined).unwrap_or(false)
    }

 /// Mirror of join_locked: every failing step stops the binding and
 /// returns false with the error string. Rust handlers run single
    /// threaded per room, so no inner lock exists — the `_locked` suffix is
 /// kept for parity with the name.
    pub fn join_locked(
        &mut self,
        chat_id: i64,
        desc: MediaDesc,
        err: &mut String,
    ) -> bool {
        let send_video = wants_video(&desc);
        let mount = self.calls.entry(chat_id).or_insert(Mount {
            joined: false,
            sending_video: false,
        });
        // Sources MUST be configured before ntg_create: the SDP offer carries
        // the video m-line only for streams already configured, and Telegram
        // discards every video frame from a call joined without one. A
        // pre-create set can be rejected by the engine — fall back to
        // post-create setup (the old audio-only offer behaviour).
        let preconfigured = self.binding.set_stream_sources(chat_id, desc.clone());
        if !preconfigured {
 // info-level in ; debug keeps test output quiet.
            parv_debug!("ubot", "pre-create set_stream rejected; applying after create");
        }
        let mut offer = String::new();
        if !self.binding.create(chat_id, &mut offer) {
            self.binding.stop(chat_id);
            *err = "webrtc create failed".into();
            return false;
        }
        if !preconfigured && !self.binding.set_stream_sources(chat_id, desc.clone()) {
            self.binding.stop(chat_id);
            *err = "set_stream_sources failed".into();
            return false;
        }
        let Some(mt) = self.mt.as_mut() else {
            self.binding.stop(chat_id);
            *err = NO_SESSION_ERR.into();
            return false;
        };
        if let Err(e) = mt.connect() {
            self.binding.stop(chat_id);
            *err = if e.is_empty() {
                "mtproto connect failed".into()
            } else {
                format!("mtproto: {e}")
            };
            return false;
        }
        let call = match mt.get_group_call(chat_id) {
            Ok(c) => c,
            Err(e) => {
                parv_error!("ubot", "get_group_call failed chat={chat_id}: {e}");
                self.binding.stop(chat_id);
                *err = e;
                return false;
            }
        };
        let server = match mt.join_group_call(&call, &offer, send_video) {
            Ok(p) => p,
            Err(e) => {
                parv_error!("ubot", "join_group_call failed chat={chat_id}: {e}");
                self.binding.stop(chat_id);
                *err = e;
                return false;
            }
        };
        if !self.binding.connect(chat_id, &server) {
            self.binding.stop(chat_id);
            *err = "webrtc connect failed".into();
            return false;
        }
        // ubot.connect_call waits on the connection callback; without this a
        // failed WebRTC handshake looks like success and the user hears
        // nothing.
        if !self.binding.wait_connected(chat_id, 20_000) {
            self.binding.stop(chat_id);
            let state = self.binding.connection_state(chat_id);
            *err = format!("webrtc connection {state}");
            return false;
        }
        mount.joined = true;
        mount.sending_video = send_video;
        true
    }

    /// Join (if needed) and start streaming desc into the chat's group call.
    pub fn play(&mut self, chat_id: i64, desc: MediaDesc, err: &mut String) -> bool {
        if self.mt.is_none() {
            *err = NO_SESSION_ERR.into();
            return false;
        }
        let mount = self.calls.entry(chat_id).or_insert(Mount {
            joined: false,
            sending_video: false,
        });
        if !mount.joined {
            return self.join_locked(chat_id, desc, err);
        }
        // Audio <-> video switches cannot be negotiated inside a live call:
        // the SDP offer was fixed at join time, so a track arriving in the
        // other mode would stream silently. Re-join with the new sources.
        let video = wants_video(&desc);
        if mount.sending_video != video {
            parv_debug!("ubot", "video mode changed chat={chat_id} -> re-join call");
            self.binding.stop(chat_id);
            mount.joined = false;
            return self.join_locked(chat_id, desc, err);
        }
        if !self.binding.set_stream_sources(chat_id, desc) {
            *err = "set_stream_sources failed".into();
            return false;
        }
        true
    }

    /// Convenience wrapper for call sites (Task 12+): build the ntg
    /// MediaDescription from file/position/speed and play it.
    pub fn play_file(
        &mut self,
        chat_id: i64,
        path: &str,
        pos: i64,
        speed: f64,
    ) -> Result<(), String> {
        let video = std::path::Path::new(path)
            .extension()
            .map(|e| e.eq_ignore_ascii_case("mp4") || e.eq_ignore_ascii_case("mkv"))
            .unwrap_or(false);
        self.play_file_mode(chat_id, path, pos, speed, video)
    }

    pub fn play_file_mode(
        &mut self,
        chat_id: i64,
        path: &str,
        pos: i64,
        speed: f64,
        video: bool,
    ) -> Result<(), String> {
        let mut err = String::new();
        let desc = media_from_file(path, pos, speed, video);
        if self.play(chat_id, desc, &mut err) {
            Ok(())
        } else {
            Err(err)
        }
    }

    pub fn pause(&mut self, chat_id: i64) -> bool {
        self.binding.pause(chat_id)
    }
    pub fn resume(&mut self, chat_id: i64) -> bool {
        self.binding.resume(chat_id)
    }
    pub fn mute(&mut self, chat_id: i64) -> bool {
        self.binding.mute(chat_id)
    }
    pub fn unmute(&mut self, chat_id: i64) -> bool {
        self.binding.unmute(chat_id)
    }

    pub fn stop(&mut self, chat_id: i64) -> bool {
        let ok = self.binding.stop(chat_id);
        self.calls.remove(&chat_id);
        ok
    }

    /// Forward stream-end events to `tx` (the monitor's advance loop).
    pub fn on_stream_end(&mut self, tx: Sender<i64>) {
        let sink = tx.clone();
        self.binding.on_stream_end(std::sync::Arc::new(move |chat| {
            let _ = sink.send(chat);
        }));
        self.stream_tx = Some(tx);
    }

    pub fn close(&mut self) {
        self.calls.clear();
    }

    pub fn binding_ops(&self) -> Vec<String> {
        self.binding
            .as_any()
            .downcast_ref::<FakeBinding>()
            .map(|b| b.ops.clone())
            .unwrap_or_default()
    }

    pub fn binding_fail_create(&mut self) {
        if let Some(b) = self.binding.as_any_mut().downcast_mut::<FakeBinding>() {
            b.fail_create = true;
        }
    }

    pub fn fire_stream_end(&mut self, chat_id: i64) {
        if let Some(tx) = &self.stream_tx {
            let _ = tx.send(chat_id);
        }
    }
}

pub const NO_SESSION_ERR: &str =
    "assistant session unavailable (STRING_SESSION failed to parse or was not set)";

