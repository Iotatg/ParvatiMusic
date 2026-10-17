//! SHA-256 and AES-IGE for the MTProto crypto layer.
//!
//! AES-IGE (openssl semantics):
//!   encrypt: c_i = E_k(m_i XOR c_{i-1}) XOR m_{i-1}
//!   decrypt: m_i = D_k(c_i XOR m_{i-1}) XOR c_{i-1}
//! iv = [c_0, m_0] (first 16 bytes cipher chain, last 16 bytes plain chain).
#![allow(dead_code)]

use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use sha2::{Digest, Sha256};

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().into()
}

/// AES-256-CTR keystream over the 128-bit big-endian counter block
/// (OpenSSL semantics: the whole 16-byte block increments).
pub struct Aes256Ctr {
    cipher: aes::Aes256,
    counter: [u8; 16],
    keystream: [u8; 16],
    pos: usize,
}

impl Aes256Ctr {
    pub fn new(key: &[u8; 32], iv: &[u8; 16]) -> Aes256Ctr {
        Aes256Ctr {
            cipher: aes::Aes256::new_from_slice(key).unwrap(),
            counter: *iv,
            keystream: [0u8; 16],
            pos: 16,
        }
    }

    fn refill(&mut self) {
        let mut block = aes::Block::clone_from_slice(&self.counter);
        self.cipher.encrypt_block(&mut block);
        self.keystream = block.into();
        for i in (0..16).rev() {
            self.counter[i] = self.counter[i].wrapping_add(1);
            if self.counter[i] != 0 {
                break;
            }
        }
        self.pos = 0;
    }

    /// XORs the keystream into `data`, keeping the stream position.
    pub fn apply(&mut self, data: &mut [u8]) {
        for b in data.iter_mut() {
            if self.pos == 16 {
                self.refill();
            }
            *b ^= self.keystream[self.pos];
            self.pos += 1;
        }
    }
}

pub fn ige_encrypt(key: &[u8; 32], iv: &[u8; 32], data: &mut [u8]) {
    assert!(data.len() % 16 == 0, "ige: data not multiple of block");
    let enc = aes::Aes256::new_from_slice(key).unwrap();
    let mut c_prev: [u8; 16] = iv[..16].try_into().unwrap();
    let mut p_prev: [u8; 16] = iv[16..].try_into().unwrap();
    for chunk in data.chunks_exact_mut(16) {
        let mut block = aes::Block::clone_from_slice(chunk);
        for i in 0..16 {
            block[i] ^= c_prev[i];
        }
        enc.encrypt_block(&mut block);
        for i in 0..16 {
            block[i] ^= p_prev[i];
        }
        c_prev = block.into();
        p_prev.copy_from_slice(chunk);
        chunk.copy_from_slice(&c_prev);
    }
}

pub fn ige_decrypt(key: &[u8; 32], iv: &[u8; 32], data: &mut [u8]) {
    assert!(data.len() % 16 == 0, "ige: data not multiple of block");
    let dec = aes::Aes256::new_from_slice(key).unwrap();
    let mut c_prev: [u8; 16] = iv[..16].try_into().unwrap();
    let mut p_prev: [u8; 16] = iv[16..].try_into().unwrap();
    for chunk in data.chunks_exact_mut(16) {
        let cipher: [u8; 16] = chunk.try_into().unwrap();
        let mut block = aes::Block::clone_from_slice(&cipher);
        for i in 0..16 {
            block[i] ^= p_prev[i];
        }
        dec.decrypt_block(&mut block);
        for i in 0..16 {
            block[i] ^= c_prev[i];
        }
        c_prev = cipher;
        p_prev = block.into();
        chunk.copy_from_slice(&p_prev);
    }
}
