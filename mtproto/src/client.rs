//! MTProto message engine.
#![allow(dead_code)]

use crate::crypto::{ige_decrypt, ige_encrypt};
use crate::messaging::MsgState;
use crate::session::Session;
use crate::tl::{TlReader, TlWriter};
use crate::transport::{mono_ms, Transport};
use sha2::Digest as _;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// CRC constants from the deployed schema (layer 227, docs/tl/api-227.tl).
pub mod crc {
    pub const PING: u32 = 0x7abe77ec;
    pub const PONG: u32 = 0x23b63b77; // second #define in the reference wins
    pub const HELP_GET_CONFIG: u32 = 0xc4f9186b;
    pub const UPDATES_GET_STATE: u32 = 0xedd4882a;
    pub const UPDATES_GET_DIFFERENCE: u32 = 0x19c2f763;
    pub const HIGH_PONG: u32 = 0x347773c5;
    pub const INVOKE_WITH_LAYER: u32 = 0xda9b0d0d;
    pub const INIT_CONNECTION: u32 = 0xc1cd5ea9;
    pub const RPC_ERROR: u32 = 0x2144ca19;
    pub const RPC_ANSWER: u32 = 0x1b2b2800;
    pub const RPC_RESULT: u32 = 0xf35c6d01;
    pub const MESSAGE_CONTAINER: u32 = 0x73f1f8dc;
    pub const MSGS_ACK: u32 = 0x62d6b459;
    pub const NEW_SESSION_CREATED: u32 = 0x9ec453c9;
    pub const NEW_SESSION_ALTERED: u32 = 0x9ec20908;
    pub const BAD_MSG_NOTIFICATION: u32 = 0xa7eff811;
    pub const BAD_SERVER_SALT: u32 = 0xedab447b;
    pub const GZIP_PACKED: u32 = 0x3072cfa1;
    pub const HTTP_WAIT: u32 = 0x9299359f;
    pub const PING_REQ: u32 = 0x7abe77ec;
    pub const GET_CONFIG: u32 = 0xc4f9186b;
    pub const RPC_ANSWER_DROPPED_RUNNING: u32 = 0x5bc0ee87;
    pub const RPC_ANSWER_DROPPED: u32 = 0x0de53dab;
}

/// Deployed schema layer (docs/tl/api-227.tl). Everything this client
/// sends is byte-identical across layers 227..230.
pub const LAYER: i32 = 227;

pub type UpdateHandler = Box<dyn Fn(&UpdateEvent) + Send>;

#[derive(Clone, Debug)]
pub struct ClientConfig {
    pub app_version: String,
    pub device_model: String,
    pub system_version: String,
    pub lang_code: String,
    /// Wrap the connection in transport obfuscation (production default);
    /// loopback engine tests use plain abridged instead.
    pub obf_transport: bool,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            app_version: "ParvatiMusic".into(),
            device_model: "Linux x86_64".into(),
            system_version: "Linux".into(),
            lang_code: "en".into(),
            obf_transport: true,
        }
    }
}

impl ClientConfig {
    /// Plain-abridged config for loopback engine tests (no obfuscation layer).
    pub fn loopback() -> Self {
        Self { obf_transport: false, ..Self::default() }
    }
}

/// Server-side update summary (decoded from RPC + push updates).
#[derive(Clone, Debug)]
pub struct UpdateEvent {
    pub kind: String, // "update:<crc8x>", "signaling", "connection_state", ...
    pub group_call_id: i64,
    pub chat_id: i64,
    pub payload: Vec<u8>,
}

/// Wire-level trace, enabled with MT_WIRE=1 (live debugging aid).
pub(crate) fn mt_trace(msg: &str) {
    if std::env::var("MT_WIRE").ok().as_deref() == Some("1") {
        eprintln!("mtproto: {msg}");
    }
}

pub fn sha1_20(data: &[u8]) -> [u8; 20] {    use sha1::Digest;
    let mut h = sha1::Sha1::new();
    h.update(data);
    h.finalize().into()
}

fn sha256_concat(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = sha2::Sha256::new();
    for p in parts {
        h.update(p);
    }
    sha2::Digest::finalize(h).into()
}

pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut out = vec![0u8; n];
    getrandom::fill(&mut out).expect("rng");
    out
}

/// MTProto 2.0 key/iv derivation (the_prayer of msg_key + auth_key slices).
pub fn derive_key_iv(auth_key: &[u8; 256], msg_key16: &[u8; 16], decode: bool) -> ([u8; 32], [u8; 32]) {
    let x = if decode { 8 } else { 0 };
    let a = sha256_concat(&[msg_key16, &auth_key[x..x + 36]]);
    let b = sha256_concat(&[&auth_key[40 + x..40 + x + 36], msg_key16]);
    let mut key32 = [0u8; 32];
    key32[..8].copy_from_slice(&a[..8]);
    key32[8..24].copy_from_slice(&b[8..24]);
    key32[24..].copy_from_slice(&a[24..32]);
    let mut iv32 = [0u8; 32];
    iv32[..8].copy_from_slice(&b[..8]);
    iv32[8..24].copy_from_slice(&a[8..24]);
    iv32[24..].copy_from_slice(&b[24..32]);
    (key32, iv32)
}

fn gunzip(input: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut dec = flate2::read::MultiGzDecoder::new(input);
    use std::io::Read;
    dec.read_to_end(&mut out).map_err(|e| format!("mtproto: gunzip failed: {e}"))?;
    if out.len() > 64 << 20 {
        return Err("mtproto: gunzip too large".into());
    }
    Ok(out)
}

fn next_msg_id(last: &mut i64) -> i64 {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs() as i64;
    let mut id = (secs << 32) | ((now.subsec_millis() as i64 * 1_000_000) & !3);
    if id <= *last {
        id = *last + 4;
    }
    *last = id;
    id
}

pub struct Client {
    pub(crate) session: Session,
    cfg: ClientConfig,
    transport: Option<Transport>,
    pub(crate) session_id: i64,
    pub(crate) last_msg_id: i64,
    pub(crate) seq_no: i64,
    pub(crate) server_salt: i64,
    pub(crate) salt_retry: bool,
    expect_pong: bool,
    rpc_ready: bool,
    init_active: bool,
    pub(crate) self_ah: i64,
    pub(crate) self_id: i64,
    pending_acks: Vec<i64>,
    pub(crate) update_handler: Option<UpdateHandler>,
    pub(crate) msg: MsgState,
    /// mono-ms of the last getDifference poll.
    pub(crate) last_poll_ms: i64,
    pub(crate) last_ping_ms: i64,
}

impl Client {
    pub fn new(session: Session, cfg: ClientConfig) -> Self {
        Self {
            session,
            cfg,
            transport: None,
            session_id: 0,
            last_msg_id: 0,
            seq_no: 0,
            server_salt: 0,
            salt_retry: false,
            expect_pong: false,
            rpc_ready: false,
            init_active: false,
            self_ah: 0,
            self_id: 0,
            pending_acks: Vec::new(),
            update_handler: None,
            msg: MsgState::new(),
            last_poll_ms: 0,
            last_ping_ms: 0,
        }
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn connected(&self) -> bool {
        self.transport.is_some()
    }

    /// TCP connect + fresh session id. Idempotent.
    pub fn connect(&mut self) -> Result<(), String> {
        if self.connected() {
            return Ok(());
        }
        let t = if self.cfg.obf_transport {
            Transport::connect_obf(&self.session.ip, self.session.port as u16)?
        } else {
            Transport::connect(&self.session.ip, self.session.port as u16)?
        };
        self.session_id = i64::from_be_bytes(random_bytes(8)[..].try_into().unwrap());
        self.last_msg_id = 0;
        self.seq_no = 0;
        self.transport = Some(t);
        Ok(())
    }

    pub fn disconnect(&mut self) {
        if let Some(t) = self.transport.as_mut() {
            t.close();
        }
        self.transport = None;
        self.rpc_ready = false;
        self.pending_acks.clear();
    }

    /// Builds one encrypted message frame. Returns (frame, msg_id).
    pub fn encrypt_body(&mut self, body: &[u8], content_related: bool) -> (Vec<u8>, i64) {
        let mid = next_msg_id(&mut self.last_msg_id);

        let seq: i64 = if content_related {
            let seq = 2 * self.seq_no + 1;
            self.seq_no = seq / 2 + 1;
            seq
        } else {
            2 * self.seq_no
        };

        let mut plain = Vec::with_capacity(body.len() + 64);
        plain.extend_from_slice(&self.server_salt.to_le_bytes());
        plain.extend_from_slice(&self.session_id.to_le_bytes());
        plain.extend_from_slice(&mid.to_le_bytes());
        plain.extend_from_slice(&(seq as i32).to_le_bytes());
        plain.extend_from_slice(&(body.len() as i32).to_le_bytes());
        plain.extend_from_slice(body);
        let pad = 16 + ((16 - (plain.len() % 16)) & 15);
        plain.extend_from_slice(&random_bytes(pad));

        // msg_key = SHA256(auth_key[88..120] ++ padded)[8..24]
        let large = sha256_concat(&[&self.session.auth_key[88..120], &plain]);
        let mut msg_key16 = [0u8; 16];
        msg_key16.copy_from_slice(&large[8..24]);
        let (key32, iv32) = derive_key_iv(&self.session.auth_key, &msg_key16, false);
        ige_encrypt(&key32, &iv32, &mut plain);

        let akh = &sha1_20(&self.session.auth_key)[12..20];
        let mut out = Vec::with_capacity(24 + plain.len());
        out.extend_from_slice(akh);
        out.extend_from_slice(&msg_key16);
        out.extend_from_slice(&plain);
        (out, mid)
    }

    /// Decrypts one encrypted_message; returns (plaintext body after the
    /// salt/session/msg_id/seq/len header, msg_id).
    pub fn decrypt_message(&self, blob: &[u8]) -> Option<(Vec<u8>, i64)> {
        if blob.len() < 24 {
            return None;
        }
        let mut msg_key16 = [0u8; 16];
        msg_key16.copy_from_slice(&blob[8..24]);
        let cipher = &blob[24..];
        if cipher.is_empty() || cipher.len() % 16 != 0 {
            return None;
        }
        let (key32, iv32) = derive_key_iv(&self.session.auth_key, &msg_key16, true);
        let mut plain = cipher.to_vec();
        ige_decrypt(&key32, &iv32, &mut plain);
        if plain.len() < 32 {
            return None;
        }
        let mut r = TlReader::new(&plain);
        let _salt = r.i64();
        let _sid = r.i64();
        let mid = r.i64();
        let _seq = r.i32();
        let blen = r.i32();
        if !r.ok() || blen < 4 || blen > plain.len() as i32 - 32 {
            return None;
        }
        Some((r.blob(blen as usize).to_vec(), mid))
    }

    pub(crate) fn dispatch_update(&self, body: &[u8]) {
        // Raw pass-through used by group-call updates; messaging feeds its
        // decoded hooks first and re-emits only unknown payloads here.
        if let Some(handler) = &self.update_handler {
            let crc = if body.len() >= 4 {
                u32::from_le_bytes(body[..4].try_into().unwrap())
            } else {
                0
            };
            let ev = UpdateEvent {
                kind: format!("update:{crc:08x}"),
                group_call_id: 0,
                chat_id: 0,
                payload: body.to_vec(),
            };
            handler(&ev);
        }
    }

    fn queue_ack(&mut self, msg_id: i64) {
        if msg_id == 0 {
            return;
        }
        if self.pending_acks.len() < 512 {
            self.pending_acks.push(msg_id);
        }
    }

    fn flush_acks(&mut self) {
        if self.pending_acks.is_empty() || !self.connected() {
            return;
        }
        let ids = std::mem::take(&mut self.pending_acks);
        let mut w = TlWriter::new();
        w.u32(crc::MSGS_ACK);
        w.u32(0x1cb5c415);
        w.u32(ids.len() as u32);
        for id in ids {
            w.i64(id);
        }
        let (enc, _) = self.encrypt_body(w.out(), false);
        if let Some(t) = self.transport.as_mut() {
            let _ = t.write_msg(&enc);
        }
    }

    /// Processes one decrypted body. Returns Ok(matched). On match, out receives the result body.
    fn handle_plaintext(
        &mut self,
        body: &[u8],
        req_id: i64,
        out_result: &mut Option<Vec<u8>>,
    ) -> Result<bool, String> {
        if body.len() < 4 {
            return Ok(false);
        }
        let crc = u32::from_le_bytes(body[..4].try_into().unwrap());
        let mut r = TlReader::new(body);
        match crc {
            crc::MESSAGE_CONTAINER => {
                r.u32();
                let count = r.i32() as u32;
                if !r.ok() || count > 4096 {
                    return Err("mtproto: malformed container".into());
                }
                for _ in 0..count {
                    let _mid = r.i64();
                    let _seq = r.i32();
                    let blen = r.i32() as usize;
                    if !r.ok() {
                        break;
                    }
                    let _blen32_ok = blen <= r.remaining();
                    let child = r.blob(blen);
                    if !r.ok() {
                        break;
                    }
                    let m = self.handle_plaintext(child, req_id, out_result)?;
                    if m {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            crc::RPC_RESULT => {
                r.u32(); // skip object crc (reader starts at 0)
                let answer = r.i64();
                if answer == req_id {
                    *out_result = Some(body[r.pos()..].to_vec());
                    return Ok(r.ok());
                }
                Ok(false)
            }
            crc::RPC_ANSWER => {
                r.u32(); // skip object crc
                let answer = r.i64();
                let inner = &body[r.pos()..];
                if answer == req_id {
                    if inner.len() >= 4 {
                        let ic = u32::from_le_bytes(inner[..4].try_into().unwrap());
                        if ic == crc::RPC_RESULT {
                            let mut ir = TlReader::new(inner);
                            ir.u32();
                            let _req = ir.i64();
                            *out_result = Some(inner[ir.pos()..].to_vec());
                            return Ok(ir.ok());
                        } else if ic == crc::MSGS_ACK
                            || ic == crc::RPC_ANSWER_DROPPED_RUNNING
                            || ic == crc::RPC_ANSWER_DROPPED
                        {
                            return Ok(false);
                        } else {
                            return self.handle_plaintext(inner, 0, out_result);
                        }
                    }
                    return Ok(false);
                }
                Ok(false)
            }
            crc::GZIP_PACKED => {
                // gzip_packed crc + pdmcstr (TL string) of the packed data
                r.u32(); // skip object crc
                let packed = r.str();
                if !r.ok() || packed.is_empty() {
                    return Err("mtproto: malformed gzip_packed".into());
                }
                let inflated = gunzip(packed)?;
                self.handle_plaintext(&inflated, req_id, out_result)
            }
            crc::BAD_SERVER_SALT => {
                // bad_msg_id:long bad_msg_seqno:int error_code:int new_server_salt:long
                r.u32(); // skip object crc
                let _ = r.i64();
                let _ = r.i32();
                let _ = r.i32();
                let salt = r.i64();
                if r.ok() {
                    self.server_salt = salt;
                    self.salt_retry = true;
                }
                Ok(false)
            }
            crc::NEW_SESSION_CREATED | crc::NEW_SESSION_ALTERED => {
                r.u32(); // skip object crc
                let _ = r.i64();
                let _ = r.i64();
                let salt = r.i64();
                if r.ok() {
                    self.server_salt = salt;
                    self.salt_retry = true;
                }
                Ok(false)
            }
            crc::PONG | crc::HIGH_PONG => {
                if self.expect_pong {
                    *out_result = Some(body.to_vec());
                    return Ok(true);
                }
                Ok(false)
            }
            _ => {
                self.msg_process(body);
                Ok(false)
            }
        }
    }

    pub fn set_update_handler<F>(&mut self, f: F)
    where
        F: Fn(&UpdateEvent) + Send + 'static,
    {
        self.update_handler = Some(Box::new(f));
    }

    /// one-time init: Ping(0) first, then
    /// InvokeWithLayer(InitConnection(help.getConfig)). Both tolerate one salt retry.
    fn init_connection(&mut self) -> Result<(), String> {
        let mut pw = TlWriter::new();
        pw.u32(crc::PING_REQ);
        pw.i64(0);
        let pong = self.call_rpc(pw.out(), false, true)?;
        let _ = pong;
        let mut gc = TlWriter::new();
        gc.u32(crc::GET_CONFIG);
        self.call_rpc(gc.out(), true, false)?;
        self.rpc_ready = true;
        Ok(())
    }

    /// Serialized TL request -> serialized TL response object (rpc_result body).
    pub fn call_rpc(&mut self, request: &[u8], wrap_init: bool, match_pong: bool) -> Result<Vec<u8>, String> {
 // lazy init like the engine; init_active spans BOTH nested calls
 // (ping + getConfig), exactly like the guard in call_rpc
        if !self.rpc_ready && !self.init_active {
            self.init_active = true;
            let r = self.init_connection();
            self.init_active = false;
            r?;
        }
        
        let to_send: Vec<u8> = if wrap_init {
            wrap_init_connection(request, self.session.api_id, &self.cfg)
        } else {
            request.to_vec()
        };

        // plaintext framing when there is no auth key yet (auth-key-id 0,
        // no IGE): this is the transport state before req_pq/dh completes.
        let auth_keyless = self.session.auth_key == [0u8; 256];

        for _attempt in 0..3 {
            self.connect()?;
            self.expect_pong = match_pong;
            let result = if auth_keyless {
                self.plaintext_call(request)
            } else {
                self.call_once(&to_send, match_pong)
            };
            self.expect_pong = false;
            match result {
                Ok(Exit::Answer(a)) => return Ok(a),
                Ok(Exit::Retry) => continue, // salt_updated: resend
                Err(e) => {
                    self.disconnect();
                    return Err(e);
                }
            }
        }
        Err("mtproto: rpc attempts exhausted".into())
    }

    /// Pre-auth transport call: plaintext framing (auth_key_id 0, no IGE)
    /// — used for req_pq_multi and the rest of the key-creation dance.
    pub(crate) fn plaintext_call(&mut self, body: &[u8]) -> Result<Exit, String> {
        let mid = next_msg_id(&mut self.last_msg_id);
        let mut out = Vec::with_capacity(20 + body.len());
        out.extend_from_slice(&[0u8; 8]); // auth_key_id = 0
        out.extend_from_slice(&mid.to_le_bytes());
        out.extend_from_slice(&(body.len() as i32).to_le_bytes());
        out.extend_from_slice(body);
        mt_trace(&format!("plaintext_call: send {} bytes", out.len()));
        {
            let t = self.transport.as_mut().ok_or("mtproto: not connected")?;
            t.write_msg(&out).map_err(|_| "mtproto: send failed".to_string())?;
        }

        let deadline = mono_ms() + 25000;
        while mono_ms() < deadline {
            let msg = {
                let t = self.transport.as_mut().ok_or("mtproto: connection closed")?;
                t.read_msg()?
            };
            let Some(msg) = msg else {
                mt_trace("plaintext_call: read timeout (no data)");
                continue;
            };
            mt_trace(&format!("plaintext_call: recv {} bytes", msg.len()));
            if msg.len() < 20 {
                mt_trace(&format!("plaintext_call: short frame {:02x?}", &msg[..msg.len().min(8)]));
                continue;
            }
            if msg[..8] != [0u8; 8] {
                if msg.len() == 4 {
                    let code = i32::from_le_bytes(msg[..4].try_into().unwrap());
                    return Err(format!("mtproto: transport error {code}"));
                }
                mt_trace("plaintext_call: nonzero auth key id, ignoring frame");
                continue;
            }
            let pay_len =
                i32::from_le_bytes(msg[16..20].try_into().unwrap()) as usize;
            if msg.len() < 20 + pay_len {
                mt_trace("plaintext_call: truncated payload");
                continue;
            }
            let payload = msg[20..20 + pay_len].to_vec();
            if payload.len() >= 4 {
                let c = u32::from_le_bytes(payload[..4].try_into().unwrap());
                mt_trace(&format!("plaintext_call: answer crc {c:#010x}"));
                if self.expect_pong && (c == crc::PONG || c == crc::HIGH_PONG) {
                    self.flush_acks();
                    return Ok(Exit::Answer(payload));
                }
            }
            self.flush_acks();
            return Ok(Exit::Answer(payload));
        }
        Err("mtproto: plaintext rpc timeout".into())
    }

    fn call_once(&mut self, to_send: &[u8], match_pong: bool) -> Result<Exit, String> {
        let (enc, req_id) = self.encrypt_body(to_send, !match_pong);
        mt_trace(&format!(
            "call_once: send {} bytes (wrap={} pong_match={})",
            enc.len(),
            !match_pong,
            match_pong
        ));
        {
            let t = self.transport.as_mut().ok_or("mtproto: not connected")?;
            t.write_msg(&enc).map_err(|_| "mtproto: send failed".to_string())?;
        }

        let deadline = mono_ms() + 25000;
        let mut unmatched = 0;
        while mono_ms() < deadline {
            let msg = {
                let t = self.transport.as_mut().ok_or("mtproto: connection closed")?;
                t.read_msg()?
            };
            match &msg {
                None => mt_trace("call_once: read timeout (no data)"),
                Some(m) => mt_trace(&format!("call_once: recv {} bytes", m.len())),
            }
            let Some(msg) = msg else { continue }; // timeout with no data
            if msg.is_empty() {
                continue; // quick ack
            }
            // verify auth_key_id matches
            let akh = &sha1_20(&self.session.auth_key)[12..20];
            if msg.len() >= 8 && msg[..8] != *akh {
                if msg.len() == 4 {
                    let code = i32::from_le_bytes(msg[..4].try_into().unwrap());
                    return Err(format!("mtproto: transport error {code}"));
                }
                return Err(format!("mtproto: auth_key_id mismatch len={}", msg.len()));
            }
            let Some((body, mid)) = self.decrypt_message(&msg) else {
                mt_trace(&format!(
                    "call_once: decrypt failed len={} head={:02x?}",
                    msg.len(),
                    &msg[..msg.len().min(16)]
                ));
                // -404 on an encrypted frame means the auth key/salt binding is
                // dead on this connection; after it the server goes silent, so
                // surface it as an error (caller must reconnect + handshake).
                if msg.len() == 4 {
                    return Err("mtproto: server -404 (auth key reset, reconnect required)".into());
                }
                continue;
            };
            if mid != 0 {
                self.queue_ack(mid);
            }
            mt_trace(&format!(
                "call_once: decrypted body {} bytes crc {:010x}",
                body.len(),
                u32::from_le_bytes(body.get(..4).and_then(|b| <[u8; 4]>::try_from(b).ok()).unwrap_or([0; 4]))
            ));
            if match_pong && body.len() >= 12 {
                let pcrc = u32::from_le_bytes(body[..4].try_into().unwrap());
                if pcrc == crc::PONG || pcrc == crc::HIGH_PONG {
                    self.flush_acks();
                    return Ok(Exit::Answer(body));
                }
            }
            let mut result: Option<Vec<u8>> = None;
            let matched = self.handle_plaintext(&body, req_id, &mut result)?;
            mt_trace(&format!("call_once: matched={matched} salt_retry={}", self.salt_retry));
            if self.salt_retry && !matched {
                // Fresh salt/session arrived and the server discarded the
                // in-flight message: resend it. An answer that arrived in
                // the same frame still wins (new salt applies to the next
                // call, not this completed one).
                self.salt_retry = false;
                return Ok(Exit::Retry);
            }
            if matched {
                self.salt_retry = false;
                let mut result = result.ok_or("mtproto: matched without result")?;
                // unwrap nested gzip-packed answers
                while result.len() > 4 {
                    let c = u32::from_le_bytes(result[..4].try_into().unwrap());
                    if c != crc::GZIP_PACKED {
                        break;
                    }
                    let mut gr = TlReader::new(&result);
                    gr.u32();
                    let packed = gr.str().to_vec();
                    if !gr.ok() || packed.is_empty() {
                        break;
                    }
                    result = gunzip(&packed)?;
                }
                // rpc_error objects become errors
                if result.len() >= 4 {
                    let c = u32::from_le_bytes(result[..4].try_into().unwrap());
                    if c == crc::RPC_ERROR {
                        let mut rr = TlReader::new(&result);
                        rr.u32();
                        let code = rr.i32();
                        let text = String::from_utf8_lossy(rr.str()).into_owned();
                        return Err(format!(
                            "telegram_error:{}:{}",
                            code,
                            if rr.ok() { text } else { "malformed".to_string() }
                        ));
                    }
                }
                self.flush_acks();
                return Ok(Exit::Answer(result));
            }
            unmatched += 1;
            self.flush_acks();
        }
        Err(format!("mtproto: rpc timeout (unmatched={unmatched})"))
    }

    /// Convenience: bare ping, no init wrap.
    pub fn ping(&mut self, ping_id: i64) -> Result<Vec<u8>, String> {
        let mut w = TlWriter::new();
        w.u32(crc::PING_REQ);
        w.i64(ping_id);
        self.call_rpc(w.out(), false, true)
    }

    /// Pump step behind TgBot's idle loop (main-thread pump): blocks up
    /// to `wait` (3s transport-read granularity) consuming incoming
    /// messages — detected updates reach the registered handler through
    /// msg_process, pongs and acks are absorbed — then flushes pending
    /// acks. After a silent window it sends a keepalive ping (the server
    /// pings on its own; the pump keeps NAT paths warm without it). Service
    /// transport errors (4-byte codes / key mismatch) bubble out like
    /// call_once.
    pub fn run_step(&mut self, wait: Duration) -> Result<(), String> {
        self.connect()?;
        let deadline = mono_ms() + wait.as_millis() as i64;
        while mono_ms() < deadline {
            let msg = {
                let t = self.transport.as_mut().ok_or("mtproto: not connected")?;
                t.read_msg()?
            };
            let Some(msg) = msg else { continue };
            if msg.is_empty() {
                continue; // transport quick-ack
            }
            let akh = &sha1_20(&self.session.auth_key)[12..20];
            if msg.len() >= 8 && msg[..8] != *akh {
                if msg.len() == 4 {
                    let code = i32::from_le_bytes(msg[..4].try_into().unwrap());
                    return Err(format!("mtproto: transport error {code}"));
                }
                return Err(format!("mtproto: auth_key_id mismatch len={}", msg.len()));
            }
            let Some((body, mid)) = self.decrypt_message(&msg) else {
                continue;
            };
            if mid != 0 {
                self.queue_ack(mid);
            }
            let mut none: Option<Vec<u8>> = None;
            self.handle_plaintext(&body, 0, &mut none)?;
        }
        // Keepalive ping: the DC kills server-side idle sessions after
        // roughly 100s, so the pump sends a bare ping on its own cadence
        // (getDifference traffic alone is not always enough).
        if mono_ms() - self.last_ping_ms >= 45000 {
            self.last_ping_ms = mono_ms();
            let mut pw = TlWriter::new();
            pw.u32(crc::PING_REQ);
            pw.i64(mono_ms());
            let _ = self.call_rpc(pw.out(), false, true);
        }
        // Poll updates.getDifference on a tight budget so commands are picked
        // up in well under a second (reference bots use push updates). Each
        // call is one RPC round-trip; 400ms keeps latency low without
        // hammering the DC. Skipped while a recent read already got traffic.
        if mono_ms() - self.last_poll_ms >= 400 && self.msg.up_pts > 0 {
            self.last_poll_ms = mono_ms();
            let _ = self.poll_difference();
        }
        self.flush_acks();
        Ok(())
    }
}

pub enum Exit {
    Answer(Vec<u8>),
    Retry,
}

fn wrap_init_connection(raw_request: &[u8], api_id: i32, cfg: &ClientConfig) -> Vec<u8> {
    // invokeWithLayer#da9b0d0d {layer} initConnection#c1cd5ea9 {flags} {... query}
    let mut w = TlWriter::new();
    w.u32(crc::INVOKE_WITH_LAYER);
    w.i32(LAYER);
    w.u32(crc::INIT_CONNECTION);
    w.i32(0); // flags: no proxy, no params
    w.i32(api_id);
    w.str(&cfg.device_model);
    w.str(&cfg.system_version);
    w.str(&cfg.app_version);
    w.str(&cfg.lang_code); // system_lang_code
    w.str(""); // lang_pack
    w.str(&cfg.lang_code);
    w.raw(raw_request);
    w.out().to_vec()
}
