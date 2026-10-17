//! MTProto authorization-key handshake (auth_key.md): req_pq_multi ->
//! resPQ, RSA_PAD-wrapped p_q_inner_data_dc, req_DH_params, server_DH_params_ok
//! decrypt, DH agree, set_client_DH_params, dh_gen_ok -> auth_key + server salt.

#![allow(dead_code)]

use crate::auth_keys::{KEY_E, KEY_NS};
use crate::bigint::{factorize_u64, Big};
use crate::client::{random_bytes, Client};
use crate::crypto::{ige_decrypt, ige_encrypt, sha256};
use crate::tl::{TlReader, TlWriter};
use sha1::{Digest as Sha1Digest, Sha1};

const REQ_PQ_MULTI: u32 = 0xbe7e8ef1;
const RES_PQ: u32 = 0x05162463;
const P_Q_INNER_DATA_DC: u32 = 0xa9f55f95;
const REQ_DH_PARAMS: u32 = 0xd712e4be;
const SERVER_DH_PARAMS_OK: u32 = 0xd0e8075c;
const SERVER_DH_PARAMS_FAIL: u32 = 0x79cb045d;
const SERVER_DH_INNER_DATA: u32 = 0xb5890dba;
const CLIENT_DH_INNER_DATA: u32 = 0x6643b654;
const SET_CLIENT_DH_PARAMS: u32 = 0xf5045f1f;
const DH_GEN_OK: u32 = 0x3bcbf734;
const DH_GEN_RETRY: u32 = 0x46dc1fb9;
const DH_GEN_FAIL: u32 = 0xa69dae02;
const VECTOR: u32 = 0x1cb5c415;

/// TL string (1-byte-prefix variant) writer.
fn put_tl_bytes(w: &mut TlWriter, data: &[u8]) {
    if data.len() < 254 {
        w.raw(&[data.len() as u8]);
    } else {
        w.raw(&[0xfe]);
        w.raw(&(data.len() as u32).to_le_bytes()[..3]);
    }
    w.raw(data);
    let hdr = if data.len() < 254 { 1 } else { 4 };
    let pad = (4 - ((hdr + data.len()) % 4)) % 4;
    w.raw(&vec![0u8; pad]);
}

/// TL string reader.
fn read_tl_bytes<'a, 'b>(r: &'b mut TlReader<'a>) -> Vec<u8> {
    r.str().to_vec()
}

/// sha1 -> 20 bytes.
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h = Sha1::new();
    h.update(data);
    h.finalize().into()
}

/// Fingerprint of a bare `rsa_public_key` serialization: 64 lower bits of
/// SHA1(string(n) ++ string(e)), strings in the 1-byte-prefix TL form.
pub fn rsa_fingerprint(n: &[u8], e: &[u8]) -> i64 {
    let mut w = TlWriter::new();
    put_tl_bytes(&mut w, n);
    put_tl_bytes(&mut w, e);
    let h = sha1(w.out());
    i64::from_le_bytes(h[12..20].try_into().unwrap())
}

/// One pinned server RSA key: modulus part `n` plus the shared exponent.
pub struct ServerRsaKey {
    pub n: &'static [u8],
    pub e: &'static [u8],
}

impl ServerRsaKey {
    const fn from_parts(n: &'static [u8]) -> ServerRsaKey {
        ServerRsaKey { n, e: &KEY_E }
    }
}

/// RSA_PAD(data, key) per auth_key.md 4.1: random padding to 192 bytes, byte
/// reverse, AES256-IGE with a random temp key and zero IV, temp key xor
/// sha256, then textbook RSA; retries until the value is below the modulus.
fn rsa_pad(data: &[u8], key: &ServerRsaKey) -> Result<Vec<u8>, String> {
    if data.len() > 144 {
        return Err("handshake: pq inner data too long".into());
    }
    let n = Big::from_bytes_be(key.n);
    for _attempt in 0..20 {
        let mut data_with_padding = vec![0u8; 192];
        data_with_padding[..data.len()].copy_from_slice(data);
        let rnd = random_bytes(192 - data.len());
        data_with_padding[data.len()..].copy_from_slice(&rnd);
        let data_pad_reversed: Vec<u8> = data_with_padding.iter().rev().cloned().collect();

        let temp_key = random_bytes(32);
        let mut mh = Vec::with_capacity(32 + 192);
        mh.extend_from_slice(&temp_key);
        mh.extend_from_slice(&data_with_padding);
        let data_hash = sha256(&mh);

        let mut data_with_hash = Vec::with_capacity(224);
        data_with_hash.extend_from_slice(&data_pad_reversed);
        data_with_hash.extend_from_slice(&data_hash);

        let zero_iv = [0u8; 32];
        ige_encrypt(temp_key[..32].try_into().unwrap(), &zero_iv, &mut data_with_hash);

        let aes_hash = sha256(&data_with_hash);
        let mut temp_key_xor = vec![0u8; 32];
        for i in 0..32 {
            temp_key_xor[i] = temp_key[i] ^ aes_hash[i];
        }

        let mut key_aes_encrypted = Vec::with_capacity(256);
        key_aes_encrypted.extend_from_slice(&temp_key_xor);
        key_aes_encrypted.extend_from_slice(&data_with_hash);

        let x = Big::from_bytes_be(&key_aes_encrypted);
        if x.cmp_to(&n) != std::cmp::Ordering::Less {
            continue;
        }
        let e = Big::from_bytes_be(key.e);
        let c = Big::mod_pow(&x, &e, &n);
        return Ok(c.to_bytes_be(256));
    }
    Err("handshake: rsa_pad exhausted retries".into())
}

/// tmp_aes_key/tmp_aes_iv for the server_DH envelope (auth_key.md item 6).
fn temp_kdf(new_nonce: &[u8; 32], server_nonce: &[u8; 16]) -> ([u8; 32], [u8; 32]) {
    let h1 = {
        let mut t = Vec::with_capacity(48);
        t.extend_from_slice(new_nonce);
        t.extend_from_slice(server_nonce);
        sha1(&t)
    };
    let h2 = {
        let mut t = Vec::with_capacity(48);
        t.extend_from_slice(server_nonce);
        t.extend_from_slice(new_nonce);
        sha1(&t)
    };
    let h3 = {
        let mut t = Vec::with_capacity(64);
        t.extend_from_slice(new_nonce);
        t.extend_from_slice(new_nonce);
        sha1(&t)
    };

    let mut key = [0u8; 32];
    key[..20].copy_from_slice(&h1);
    key[20..].copy_from_slice(&h2[..12]);

    let mut iv = [0u8; 32];
    iv[..8].copy_from_slice(&h2[12..20]);
    iv[8..28].copy_from_slice(&h3[..20]);
    iv[28..].copy_from_slice(&new_nonce[..4]);
    (key, iv)
}

struct ResPq {
    nonce: [u8; 16],
    server_nonce: [u8; 16],
    pq: Vec<u8>,
    fingerprints: Vec<i64>,
}

fn parse_res_pq(body: &[u8]) -> Result<ResPq, String> {
    let mut r = TlReader::new(body);
    let ctor = r.u32();
    if ctor != RES_PQ {
        return Err(format!("handshake: expected resPQ, got {ctor:#010x}"));
    }
    let mut nonce = [0u8; 16];
    nonce.copy_from_slice(r.blob(16));
    let mut server_nonce = [0u8; 16];
    server_nonce.copy_from_slice(r.blob(16));
    let pq = r.str().to_vec();
    if pq.is_empty() || pq.len() > 8 {
        return Err("handshake: pq width outside 1..8 bytes".into());
    }
    if r.u32() != VECTOR {
        return Err("handshake: resPQ fingerprints vector header missing".into());
    }
    let count = r.i32();
    if !(0..=8).contains(&count) {
        return Err("handshake: bad fingerprint count".into());
    }
    let mut fingerprints = Vec::with_capacity(count as usize);
    for _ in 0..count {
        fingerprints.push(r.i64());
    }
    Ok(ResPq { nonce, server_nonce, pq, fingerprints })
}

/// Minimal big-endian bytes (no leading zeroes), grepping gogram's
/// big.Int.Bytes() behavior for p/q wire strings.
fn be_min_bytes(v: u64) -> Vec<u8> {
    let b = v.to_be_bytes();
    match b.iter().position(|&x| x != 0) {
        Some(i) => b[i..].to_vec(),
        None => vec![0],
    }
}

fn find_rsa_key(fingerprints: &[i64]) -> Option<ServerRsaKey> {
    KEY_NS
        .iter()
        .copied()
        .map(ServerRsaKey::from_parts)
        .find(|key| fingerprints.contains(&rsa_fingerprint(key.n, key.e)))
}

fn parse_server_dh_params(body: &[u8], expect_nonce: &[u8; 16]) -> Result<(Vec<u8>, Vec<u8>), String> {
    let mut r = TlReader::new(body);
    let ctor = r.u32();
    if ctor == SERVER_DH_PARAMS_FAIL {
        return Err("handshake: server_DH_params_fail".into());
    }
    if ctor != SERVER_DH_PARAMS_OK {
        return Err(format!("handshake: expected server_DH_params_ok, got {ctor:#010x}"));
    }
    let mut nonce = [0u8; 16];
    nonce.copy_from_slice(r.blob(16));
    if nonce != *expect_nonce {
        return Err("handshake: server_DH_params nonce mismatch".into());
    }
    let mut server_nonce = [0u8; 16];
    server_nonce.copy_from_slice(r.blob(16));
    Ok((server_nonce.to_vec(), r.str().to_vec()))
}

/// Decrypts and validates server_DH_inner_data; returns (dh_prime, g, g_a, server_time).
fn decrypt_server_dh_inner(
    encrypted: &[u8],
    nonce: &[u8; 16],
    server_nonce: &[u8; 16],
    new_nonce: &[u8; 32],
) -> Result<(Vec<u8>, i32, Vec<u8>, i32), String> {
    if encrypted.is_empty() || encrypted.len() % 16 != 0 {
        return Err("handshake: encrypted_answer not block aligned".into());
    }
    let (key, iv) = temp_kdf(new_nonce, server_nonce);
    let mut answer = encrypted.to_vec();
    ige_decrypt(&key, &iv, &mut answer);

    // answer_with_hash := SHA1(answer) ++ answer ++ (0..15 random bytes)
    let mut inner_len = None;
    if answer.len() >= 20 {
        for candidate in (0..=answer.len().saturating_sub(20)).rev() {
            if sha1(&answer[20..20 + candidate])[..] == answer[..20] {
                inner_len = Some(candidate);
                break;
            }
        }
    }
    let inner_len = inner_len.ok_or("handshake: answer sha1 mismatch")?;
    let mut r = TlReader::new(&answer[20..20 + inner_len]);
    let ctor = r.u32();
    if ctor != SERVER_DH_INNER_DATA {
        return Err(format!("handshake: expected server_DH_inner_data, got {ctor:#010x}"));
    }
    let mut n = [0u8; 16];
    n.copy_from_slice(r.blob(16));
    if n != *nonce {
        return Err("handshake: inner nonce mismatch".into());
    }
    n.copy_from_slice(r.blob(16));
    if n != *server_nonce {
        return Err("handshake: inner server nonce mismatch".into());
    }
    let g = r.i32();
    let dh_prime = r.str().to_vec();
    let g_a = r.str().to_vec();
    let server_time = r.i32();
    Ok((dh_prime, g, g_a, server_time))
}

pub struct HandshakeResult {
    pub auth_key: [u8; 256],
    pub server_salt: i64,
}

impl Client {
    /// Runs the full auth-key handshake over the (obfuscated, plaintext)
    /// transport and stores auth_key + server_salt in the client state.
    pub fn handshake(&mut self) -> Result<HandshakeResult, String> {
        if !self.connected() {
            self.connect()?;
        }

        // 1) req_pq_multi -> resPQ
        let nonce: [u8; 16] = random_bytes(16).try_into().unwrap();
        let mut w = TlWriter::new();
        w.u32(REQ_PQ_MULTI);
        w.raw(&nonce);
        let resp = match self.plaintext_call(w.out())? {
            crate::client::Exit::Answer(a) => a,
            crate::client::Exit::Retry => return Err("handshake: unexpected retry".into()),
        };
        let res = parse_res_pq(&resp)?;
        if res.nonce != nonce {
            return Err("handshake: resPQ nonce mismatch".into());
        }
        let server_nonce: [u8; 16] = res.server_nonce;

        // 2) key select + pq factorization
        let rsa_key =
            find_rsa_key(&res.fingerprints).ok_or("handshake: no server RSA key matches fingerprints")?;
        let pq_num = {
            let mut b = [0u8; 8];
            b[8 - res.pq.len()..].copy_from_slice(&res.pq);
            u64::from_be_bytes(b)
        };
        let (p_num, q_num) = factorize_u64(pq_num);
        if p_num <= 1 || q_num <= 1 || p_num.checked_mul(q_num) != Some(pq_num) {
            return Err("handshake: pq factorization failed".into());
        }

        // 3) p_q_inner_data_dc, RSA_PAD, req_DH_params
        let new_nonce: [u8; 32] = random_bytes(32).try_into().unwrap();
        let mut inner = TlWriter::new();
        inner.u32(P_Q_INNER_DATA_DC);
        put_tl_bytes(&mut inner, &res.pq);
        put_tl_bytes(&mut inner, &be_min_bytes(p_num));
        put_tl_bytes(&mut inner, &be_min_bytes(q_num));
        inner.raw(&nonce);
        inner.raw(&server_nonce);
        inner.raw(&new_nonce);
        inner.i32(self.session.dc_id);
        let encrypted = rsa_pad(inner.out(), &rsa_key)?;

        let mut req = TlWriter::new();
        req.u32(REQ_DH_PARAMS);
        req.raw(&nonce);
        req.raw(&server_nonce);
        put_tl_bytes(&mut req, &be_min_bytes(p_num));
        put_tl_bytes(&mut req, &be_min_bytes(q_num));
        req.i64(rsa_fingerprint(rsa_key.n, rsa_key.e));
        put_tl_bytes(&mut req, &encrypted);
        let resp = match self.plaintext_call(req.out())? {
            crate::client::Exit::Answer(a) => a,
            crate::client::Exit::Retry => return Err("handshake: unexpected retry".into()),
        };
        let (got_server_nonce, encrypted_answer) = parse_server_dh_params(&resp, &nonce)?;
        if got_server_nonce != server_nonce {
            return Err("handshake: server nonce mismatch in DH params".into());
        }

        // 4) server_DH_inner_data
        let (dh_prime, g, g_a, _server_time) =
            decrypt_server_dh_inner(&encrypted_answer, &nonce, &server_nonce, &new_nonce)?;
        if dh_prime.len() != 256 || g_a.len() != 256 {
            return Err("handshake: bad DH parameter sizes".into());
        }

        // 5) DH agree: g_b = g^b mod p, auth_key = g_a^b mod p
        let prime = Big::from_bytes_be(&dh_prime);
        let g_a = Big::from_bytes_be(&g_a);
        let g = Big::from_bytes_be(&(g as i64).to_be_bytes()[4..]);
        let b = Big::from_bytes_be(&random_bytes(256));
        let g_b = Big::mod_pow(&g, &b, &prime);
        let g_ab = Big::mod_pow(&g_a, &b, &prime);
        let auth_key: [u8; 256] = g_ab.to_bytes_be(256).try_into().unwrap();

        // 6) client_DH_inner_data + envelope, set_client_DH_params
        let mut inner_data = TlWriter::new();
        inner_data.u32(CLIENT_DH_INNER_DATA);
        inner_data.raw(&nonce);
        inner_data.raw(&server_nonce);
        inner_data.i64(0); // retry_id
        // gB minus leading zeroes, matching big.Int.Bytes() in live clients.
        let g_b_min: Vec<u8> = {
            let full = g_b.to_bytes_be(256);
            match full.iter().position(|&x| x != 0) {
                Some(i) => full[i..].to_vec(),
                None => vec![0],
            }
        };
        put_tl_bytes(&mut inner_data, &g_b_min);

        let body = inner_data.out().to_vec();
        let mut with_hash = Vec::with_capacity(20 + body.len() + 16);
        with_hash.extend_from_slice(&sha1(&body));
        with_hash.extend_from_slice(&body);
        let pad = (16 - (with_hash.len() % 16)) % 16;
        with_hash.extend_from_slice(&random_bytes(pad));

        let (key, iv) = temp_kdf(&new_nonce, &server_nonce);
        ige_encrypt(&key, &iv, &mut with_hash);

        let mut req = TlWriter::new();
        req.u32(SET_CLIENT_DH_PARAMS);
        req.raw(&nonce);
        req.raw(&server_nonce);
        put_tl_bytes(&mut req, &with_hash);
        let resp = match self.plaintext_call(req.out())? {
            crate::client::Exit::Answer(a) => a,
            crate::client::Exit::Retry => return Err("handshake: unexpected retry".into()),
        };

        // 7) dh_gen answer
        let mut r = TlReader::new(&resp);
        let ctor = r.u32();
        if ctor != DH_GEN_OK {
            return Err(format!("handshake: dh_gen not ok ({ctor:#010x})"));
        }
        let mut n16 = [0u8; 16];
        n16.copy_from_slice(r.blob(16));
        if n16 != nonce {
            return Err("handshake: dh_gen nonce mismatch".into());
        }
        n16.copy_from_slice(r.blob(16));
        if n16 != server_nonce {
            return Err("handshake: dh_gen server nonce mismatch".into());
        }
        let hash16 = r.blob(16).to_vec();
        let aux = sha1(&auth_key);
        let mut t = Vec::with_capacity(41);
        t.extend_from_slice(&new_nonce);
        t.push(1);
        t.extend_from_slice(&aux[..8]);
        if sha1(&t)[4..20] != hash16[..] {
            return Err("handshake: new_nonce_hash1 mismatch".into());
        }

        // 8) server_salt = new_nonce[0..8] xor server_nonce[0..8]
        let mut salt_bytes = [0u8; 8];
        for i in 0..8 {
            salt_bytes[i] = new_nonce[i] ^ server_nonce[i];
        }
        let server_salt = i64::from_le_bytes(salt_bytes);

        self.session.auth_key = auth_key;
        self.server_salt = server_salt;
        self.salt_retry = false;
        Ok(HandshakeResult { auth_key, server_salt })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_production_key_fingerprint() {
        // The fingerprint of the canonical production key must match the
        // well-known constant.
        let fp = rsa_fingerprint(KEY_NS[2], &KEY_E);
        assert_eq!(fp as u64, 0xc3b42b026ce86b21);
    }

    #[test]
    fn fingerprints_differ_between_keys() {
        let fp_a = rsa_fingerprint(KEY_NS[0], &KEY_E);
        let fp_b = rsa_fingerprint(KEY_NS[1], &KEY_E);
        assert_ne!(fp_a, fp_b);
    }

    #[test]
    fn temp_kdf_is_deterministic() {
        let n = [7u8; 32];
        let s = [9u8; 16];
        let (k1, i1) = temp_kdf(&n, &s);
        let (k2, i2) = temp_kdf(&n, &s);
        assert_eq!(k1, k2);
        assert_eq!(i1, i2);
    }

    #[test]
    fn res_pq_roundtrip() {
        let mut body = Vec::new();
        body.extend_from_slice(&RES_PQ.to_le_bytes());
        body.extend_from_slice(&[1u8; 16]);
        body.extend_from_slice(&[2u8; 16]);
        body.push(0x04);
        body.extend_from_slice(&[0x00, 0x00, 0x7f, 0xff]); // pq string: hdr=1+4 data
        body.extend_from_slice(&[0, 0, 0]); // align to 4
        body.extend_from_slice(&VECTOR.to_le_bytes());
        body.extend_from_slice(&2i32.to_le_bytes());
        body.extend_from_slice(&11i64.to_le_bytes());
        body.extend_from_slice(&22i64.to_le_bytes());
        let parsed = parse_res_pq(&body).unwrap();
        assert_eq!(parsed.nonce, [1u8; 16]);
        assert_eq!(parsed.server_nonce, [2u8; 16]);
        assert_eq!(parsed.pq, vec![0, 0, 0x7f, 0xff]);
        assert_eq!(parsed.fingerprints, vec![11, 22]);
    }

    #[test]
    fn rsa_pad_shape() {
        let key = ServerRsaKey::from_parts(KEY_NS[2]);
        let out = rsa_pad(&[1u8; 16], &key).unwrap();
        assert_eq!(out.len(), 256);
    }
}
