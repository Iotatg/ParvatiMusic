//! Abridged TCP transport (socket_connect / write_msg / recv_chunk / read_msg).
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

pub struct Transport {
    stream: TcpStream,
    recv_buf: Vec<u8>,
    /// Obfuscated-transport state (transport-obfuscation layer above the
    /// abridged framing): when set, every byte after the 64-byte init is
    /// AES-256-CTR encrypted/decrypted with a persistent stream.
    obf: Option<(crate::crypto::Aes256Ctr, crate::crypto::Aes256Ctr)>,
}

// Mono-clock helper matching mono_ms.
pub(crate) fn mono_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn rand_fill(buf: &mut [u8]) {
    let n = buf.len();
    let out = crate::client::random_bytes(n);
    buf.copy_from_slice(&out);
}

fn socket_setup(ip: &str, port: u16) -> Result<TcpStream, String> {    let addrs: Vec<_> = (ip, port)
        .to_socket_addrs()
        .map_err(|_| format!("mtproto: dns failed for {}", ip))?
        .filter(|a| a.is_ipv4())
        .collect();
    let addr = *addrs.first().ok_or_else(|| format!("mtproto: dns failed for {}", ip))?;
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(15))
        .map_err(|_| format!("mtproto: tcp connect failed to {}", ip))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .map_err(|_| "mtproto: setsockopt failed".to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(15)))
        .map_err(|_| "mtproto: setsockopt failed".to_string())?;
    Ok(stream)
}

impl Transport {
    /// AF_INET connect with 15s socket timeouts, then the abridged
    /// transport announcement (single 0xEF byte).
    pub fn connect(ip: &str, port: u16) -> Result<Transport, String> {
        let stream = socket_setup(ip, port)?;
        let mut t = Transport { stream, recv_buf: Vec::new(), obf: None };
        // abridged transport announcement
        t.send_all(&[0xef])
            .map_err(|_| "mtproto: transport announcement failed".to_string())?;
        Ok(t)
    }

    /// Connect with the transport-obfuscation layer (random 64-byte init
    /// carrying the abridged protocol id at offset 56, then persistent
    /// AES-256-CTR streams; the init itself goes through the CTR too).
    pub fn connect_obf(ip: &str, port: u16) -> Result<Transport, String> {
        let stream = socket_setup(ip, port)?;

        let proto: [u8; 4] = [0xef; 4];
        let init = loop {
            let mut init = [0u8; 64];
            rand_fill(&mut init);
            init[56..60].copy_from_slice(&proto);
            let first = u32::from_be_bytes([init[0], init[1], init[2], init[3]]);
            let second = u32::from_be_bytes([init[4], init[5], init[6], init[7]]);
            let forbidden = first == 0xefefefef
                || first == u32::from_le_bytes(*b"HEAD")
                || first == u32::from_le_bytes(*b"POST")
                || first == u32::from_le_bytes(*b"GET ")
                || first == u32::from_le_bytes(*b"OPTI")
                || first == 0xdddddddd
                || first == 0xeeeeeeee
                || second == 0;
            if !forbidden && init[0] != 0xef {
                break init;
            }
        };

        let enc_key: [u8; 32] = init[8..40].try_into().unwrap();
        let enc_iv: [u8; 16] = init[40..56].try_into().unwrap();
        let mut rev = init;
        rev.reverse();
        let dec_key: [u8; 32] = rev[8..40].try_into().unwrap();
        let dec_iv: [u8; 16] = rev[40..56].try_into().unwrap();

        let mut out_stream = crate::crypto::Aes256Ctr::new(&enc_key, &enc_iv);
        // the init itself is the first thing pushed through the CTR; bytes
        // 56..64 are replaced by the encrypted ones in the wire copy
        let mut scratch = init;
        out_stream.apply(&mut scratch);
        let wire = {
            let mut w = [0u8; 64];
            w[..56].copy_from_slice(&init[..56]);
            w[56..].copy_from_slice(&scratch[56..]);
            w
        };

        // init goes out raw (before the stream cipher starts consuming)
        let mut t = Transport { stream, recv_buf: Vec::new(), obf: None };
        t.send_all(&wire)
            .map_err(|_| "mtproto: obfuscation init failed".to_string())?;
        t.obf = Some((out_stream, crate::crypto::Aes256Ctr::new(&dec_key, &dec_iv)));
        Ok(t)
    }

    pub fn is_connected(&self) -> bool {
 true // tracks fd_ >= 0; a dropped connection surfaces on I/O
    }

    fn send_all(&mut self, data: &[u8]) -> Result<(), String> {
        let payload: Vec<u8> = if let Some((out, _)) = self.obf.as_mut() {
            let mut c = data.to_vec();
            out.apply(&mut c);
            c
        } else {
            data.to_vec()
        };
        let mut rest: &[u8] = &payload;
        while !rest.is_empty() {
            match self.stream.write(rest) {
                Ok(0) => return Err("mtproto: send failed".into()),
                Ok(n) => rest = &rest[n..],
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err("mtproto: send failed".into()),
            }
        }
        Ok(())
    }

    /// Framed send: length prefix in 32-bit words.
    pub fn write_msg(&mut self, msg: &[u8]) -> Result<(), String> {
        let words = msg.len() / 4;
        let mut framed = Vec::with_capacity(msg.len() + 4);
        if words < 0x7f {
            framed.push(words as u8);
        } else {
            framed.push(0x7f);
            framed.push((words & 0xff) as u8);
            framed.push(((words >> 8) & 0xff) as u8);
            framed.push(((words >> 16) & 0xff) as u8);
        }
        framed.extend_from_slice(msg);
        self.send_all(&framed)
    }

    /// Receives some bytes into the buffer. Ok(false) = timeout (not an error),
 /// Err = connection closed/failed. Mirrors recv_chunk.
    fn recv_chunk(&mut self, timeout_ms: u64) -> Result<bool, String> {
        self.stream
            .set_read_timeout(Some(Duration::from_millis(timeout_ms)))
            .map_err(|e| e.to_string())?;
        let mut buf = [0u8; 65536];
        match self.stream.read(&mut buf) {
            Ok(0) => Err("mtproto: connection closed".into()),
            Ok(n) => {
                let raw = &buf[..n];
                let decrypted: Vec<u8> = match self.obf.as_mut() {
                    Some((_, inp)) => {
                        let mut d = raw.to_vec();
                        inp.apply(&mut d);
                        d
                    }
                    None => raw.to_vec(),
                };
                self.recv_buf.extend_from_slice(&decrypted);
                Ok(true)
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted
                ) =>
            {
                Ok(false)
            }
            Err(_) => Err("mtproto: recv failed".into()),
        }
    }

    /// Reads one framed message from buffered data, receiving as needed.
    /// Ok(Some(msg)) = consumed (may be empty quick-ack);
    /// Ok(None) = no complete frame within the 3s deadline;
    /// Err = connection dead.
    ///
 /// Deviation note: the reference only recvs when its buffer is
    /// EMPTY, so a partially-received frame spins without progress until the
    /// caller's deadline (latent stall, third bug found in the reference).
    /// Here a partial frame triggers further recv chunks, so progress is
    /// guaranteed while data keeps arriving.
    pub fn read_msg(&mut self) -> Result<Option<Vec<u8>>, String> {
        let deadline = mono_ms() + 3000;
        loop {
            // framing parse attempt
            if !self.recv_buf.is_empty() {
                let b0 = self.recv_buf[0];
                let (hdr, len) = if b0 < 0x7f {
                    (1usize, b0 as usize * 4)
                } else {
                    if self.recv_buf.len() < 4 {
                        if mono_ms() >= deadline {
                            return Ok(None);
                        }
                        let _ = self.recv_chunk(200)?;
                        continue;
                    }
                    (
                        4usize,
                        ((self.recv_buf[1] as usize)
                            | ((self.recv_buf[2] as usize) << 8)
                            | ((self.recv_buf[3] as usize) << 16))
                            * 4,
                    )
                };
                if self.recv_buf.len() >= hdr + len {
                    let msg = self.recv_buf[hdr..hdr + len].to_vec();
                    self.recv_buf.drain(..hdr + len);
                    return Ok(Some(msg));
                }
            }
            // need more data
            if mono_ms() >= deadline {
                return Ok(None);
            }
            let _ = self.recv_chunk(200)?;
            if mono_ms() >= deadline {
                return Ok(None);
            }
        }
    }

    pub fn close(&mut self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        self.recv_buf.clear();
    }
}

/// Mono deadline helper for callers (kept pub for engine parity).
pub fn now_mono_ms() -> i64 {
    mono_ms()
}

#[allow(unused)]
fn unused_instant_hint() -> Instant {
    Instant::now()
}
