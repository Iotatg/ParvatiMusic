//! String-session parsers.
//!
//! Two formats are accepted:
//!   * 271-byte string (standard or base64url), big-endian layout:
//!     dc_id(1) api_id(4) test_mode(1) auth_key(256) user_id(8) bot(1)
//!   * ascii "1BvE" prefix + base64url( json { key, hash, dc_id, ip_addr, app_id, ... } )
#![allow(dead_code)]

/// Authorized session obtained from a userbot string session (271-byte
/// binary or `1BvE` JSON format).
#[derive(Clone, Debug)]
pub struct Session {
    pub dc_id: i32,
    pub ip: String, // literal address, no port
    pub port: i32,
    pub auth_key: [u8; 256],
    pub api_id: i32,
    pub user_id: i64,
}

impl Default for Session {
    /// Production DC2 entry point — fresh logins (bot auth) start here;
    /// parsed session strings override every field.
    fn default() -> Self {
        Self {
            dc_id: 2,
            ip: dc_ip(2).to_string(),
            port: 443,
            auth_key: [0u8; 256],
            api_id: 0,
            user_id: 0,
        }
    }
}

/// Default data-center table (main net), by dc_id.
pub fn dc_ip(dc_id: i32) -> &'static str {
    match dc_id {
        1 => "149.154.175.58",
        2 => "149.154.167.50",
        3 => "149.154.175.100",
        4 => "149.154.167.91",
        5 => "91.108.56.151",
        _ => "",
    }
}

/// Base64 decoder accepting both standard (+/) and base64url (-_) alphabets,
/// skipping '=' and CR/LF, like the b64url_decode.
pub(crate) fn b64_decode_any(in_str: &str) -> Result<Vec<u8>, String> {
    const ALPHA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut rev = [-1i32; 256];
    for (i, &c) in ALPHA.iter().enumerate() {
        rev[c as usize] = i as i32;
    }
    rev[b'+' as usize] = 62;
    rev[b'/' as usize] = 63;
    let mut out = Vec::new();
    let mut bits = 0i32;
    let mut acc = 0u32;
    for &c in in_str.as_bytes() {
        if c == b'=' || c == b'\n' || c == b'\r' {
            continue;
        }
        let v = rev[c as usize];
        if v < 0 {
            return Err("mtproto: bad base64url char".into());
        }
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

fn be32(s: &[u8], off: usize) -> u32 {
    ((s[off] as u32) << 24) | ((s[off + 1] as u32) << 16) | ((s[off + 2] as u32) << 8) | (s[off + 3] as u32)
}

fn be64(s: &[u8], off: usize) -> u64 {
    let mut v: u64 = 0;
    for i in 0..8 {
        v = (v << 8) | s[off + i] as u64;
    }
    v
}

// Minimal flat JSON walker, ports the MiniJson: string/number values only,
// no unescaping (parity with reference).
struct MiniJson<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> MiniJson<'a> {
    fn skip_ws(&mut self) {
        while self.i < self.s.len() && (self.s[self.i] as char).is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn key(&mut self) -> Result<String, String> {
        self.skip_ws();
        if self.i >= self.s.len() || self.s[self.i] != b'"' {
            return Err("mtproto: json key".into());
        }
        self.i += 1;
        let st = self.i;
        while self.i < self.s.len() && self.s[self.i] != b'"' {
            self.i += 1;
        }
        let k = String::from_utf8_lossy(&self.s[st..self.i]).into_owned();
        self.i += 1; // closing quote
        self.skip_ws();
        if self.i >= self.s.len() || self.s[self.i] != b':' {
            return Err("mtproto: json colon".into());
        }
        self.i += 1;
        self.skip_ws();
        Ok(k)
    }

    fn raw_value(&mut self) -> String {
        let st = self.i;
        if self.i < self.s.len() && self.s[self.i] == b'"' {
            self.i += 1;
            while self.i < self.s.len() && self.s[self.i] != b'"' {
                self.i += 1;
            }
 // Some encoders leave the cursor ON the closing quote
            // for string values. Consume it so subsequent members parse
            // correctly.
            if self.i < self.s.len() {
                self.i += 1;
            }
            return String::from_utf8_lossy(&self.s[st + 1..self.i - 1]).into_owned();
        }
        if self.i < self.s.len() && (self.s[self.i] == b'{' || self.s[self.i] == b'[') {
            let mut depth = 0;
            while self.i < self.s.len() {
                let c = self.s[self.i];
                if c == b'{' || c == b'[' {
                    depth += 1;
                } else if c == b'}' || c == b']' {
                    depth -= 1;
                    if depth == 0 {
                        self.i += 1;
                        break;
                    }
                }
                self.i += 1;
            }
            return String::from_utf8_lossy(&self.s[st..self.i]).into_owned();
        }
        while self.i < self.s.len()
            && self.s[self.i] != b','
            && self.s[self.i] != b'}'
            && self.s[self.i] != b']'
        {
            self.i += 1;
        }
        String::from_utf8_lossy(&self.s[st..self.i]).into_owned()
    }
}

fn atoll(v: &str) -> i64 {
    // atoi-style: parse leading digits (with optional sign), ignore the rest.
    let t = v.trim_start();
    let mut n: i64 = 0;
    let mut neg = false;
    let bytes = t.as_bytes();
    let mut k = 0;
    if k < bytes.len() && (bytes[k] == b'-' || bytes[k] == b'+') {
        neg = bytes[k] == b'-';
        k += 1;
    }
    while k < bytes.len() && bytes[k].is_ascii_digit() {
        n = n.saturating_mul(10).saturating_add((bytes[k] - b'0') as i64);
        k += 1;
    }
    if neg {
        -n
    } else {
        n
    }
}

pub fn parse_session_string(encoded: &str) -> Result<Session, String> {
    let mut ses = Session::default();

    // `1BvE` JSON format: version prefix + base64url(json)
    if encoded.len() > 4 && encoded.starts_with("1BvE") {
        let js = b64_decode_any(&encoded[4..])?;
        let js = String::from_utf8_lossy(&js).into_owned();
        let mut j = MiniJson { s: js.as_bytes(), i: 0 };
        let mut key_b64 = String::new();
        let mut dc_id: i64 = 0;
        let mut ip = String::new();
        let mut app_id: i64 = 0;
        let mut i = 0usize;
        while i < js.len() {
            // walk flat object members
            let start = js[i..]
                .char_indices()
                .find(|(_, c)| !" \t\n\r{,".contains(*c))
                .map(|(o, _)| i + o);
            let Some(start) = start else { break };
            if js.as_bytes()[start] == b'}' {
                break;
            }
 // Some encoders never sync the json cursor with `start`;
            // sync it here so the documented format always parses.
            j.i = start;
            let k = j.key()?;
            let v = j.raw_value();
            j.skip_ws();
            i = j.i;
            match k.as_str() {
                "key" => key_b64 = v,
                "dc_id" => dc_id = atoll(&v),
                "ip_addr" => ip = v,
                "app_id" => app_id = atoll(&v),
                _ => {}
            }
        }
        if key_b64.is_empty() || dc_id == 0 {
            return Err("mtproto: string session missing key/dc".into());
        }
        let key = b64_decode_any(&key_b64)?;
        if key.len() != 256 {
            return Err(format!("mtproto: session key length {}", key.len()));
        }
        ses.auth_key.copy_from_slice(&key);
        ses.dc_id = dc_id as i32;
        ses.ip = if !ip.is_empty() { ip } else { dc_ip(ses.dc_id).to_string() };
        ses.api_id = app_id as i32;
        return Ok(ses);
    }

    // 271-byte format: standard base64 of 271 bytes.
    // Also accepts plain base64url variants.
    let raw = b64_decode_any(encoded)?;
    if raw.len() == 271 {
        ses.dc_id = raw[0] as i32;
        ses.api_id = be32(&raw, 1) as i32;
        // raw[5] = test_mode
        ses.auth_key.copy_from_slice(&raw[6..262]);
        ses.user_id = be64(&raw, 262) as i64;
        // raw[270] = bot flag
        if !(1..=5).contains(&ses.dc_id) {
            return Err(format!("mtproto: session dc={} out of range", ses.dc_id));
        }
        ses.ip = dc_ip(ses.dc_id).to_string();
        return Ok(ses);
    }
    Err(format!("mtproto: unknown session format len={}", raw.len()))
}
