//! TL binary codec.
#![allow(dead_code)]

/// Little-endian TL binary writer.
pub struct TlWriter {
    buf: Vec<u8>,
}

impl Default for TlWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl TlWriter {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i32(&mut self, v: i32) {
        self.u32(v as u32);
    }

    pub fn i64(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn bytes(&mut self, p: &[u8]) {
        self.buf.extend_from_slice(p);
    }

    /// TL `bytes` field: length prefix (1 byte, or 0xfe + 3 LE bytes),
    /// payload, zero padding to a 4-byte boundary.
    pub fn blob(&mut self, p: &[u8]) {
        let len = p.len();
        if len < 254 {
            self.buf.push(len as u8);
            self.buf.extend_from_slice(p);
            let pad = (4 - ((1 + len) % 4)) % 4;
            self.buf.resize(self.buf.len() + pad, 0);
        } else {
            self.buf.push(0xfe);
            self.buf.push((len & 0xff) as u8);
            self.buf.push(((len >> 8) & 0xff) as u8);
            self.buf.push(((len >> 16) & 0xff) as u8);
            self.buf.extend_from_slice(p);
            let pad = (4 - ((4 + len) % 4)) % 4;
            self.buf.resize(self.buf.len() + pad, 0);
        }
    }

 /// Append pre-encoded bytes verbatim.
    pub fn raw(&mut self, s: &[u8]) {
        self.buf.extend_from_slice(s);
    }

    pub fn str(&mut self, s: &str) {
        let b = s.as_bytes();
        let len = b.len();
        if len < 254 {
            self.buf.push(len as u8);
            self.buf.extend_from_slice(b);
            let pad = (4 - ((1 + len) % 4)) % 4;
            self.buf.resize(self.buf.len() + pad, 0);
        } else {
            self.buf.push(0xfe);
            self.buf.push((len & 0xff) as u8);
            self.buf.push(((len >> 8) & 0xff) as u8);
            self.buf.push(((len >> 16) & 0xff) as u8);
            self.buf.extend_from_slice(b);
            let pad = (4 - ((4 + len) % 4)) % 4;
            self.buf.resize(self.buf.len() + pad, 0);
        }
    }

    pub fn vec_header(&mut self, count: u32) {
        self.u32(0x1cb5c415);
        self.u32(count);
    }

    pub fn out(&self) -> &[u8] {
        &self.buf
    }
}

/// Bounds-checked TL reader.
pub struct TlReader<'a> {
    s: &'a [u8],
    p: usize,
}

impl<'a> TlReader<'a> {
    pub fn new(s: &'a [u8]) -> Self {
        Self { s, p: 0 }
    }

    pub fn pos(&self) -> usize {
        self.p
    }

    pub fn remaining(&self) -> usize {
        if self.p <= self.s.len() {
            self.s.len() - self.p
        } else {
            0
        }
    }

    pub fn ok(&self) -> bool {
        self.p <= self.s.len()
    }

    pub fn done(&self) -> bool {
        self.p == self.s.len()
    }

    fn need(&self, n: usize) {
        assert!(self.p + n <= self.s.len(), "tl: out of data");
    }

    pub fn u32(&mut self) -> u32 {
        self.need(4);
        let v = u32::from_le_bytes(self.s[self.p..self.p + 4].try_into().unwrap());
        self.p += 4;
        v
    }

    pub fn i32(&mut self) -> i32 {
        self.u32() as i32
    }

    pub fn i64(&mut self) -> i64 {
        self.need(8);
        let v = i64::from_le_bytes(self.s[self.p..self.p + 8].try_into().unwrap());
        self.p += 8;
        v
    }

    pub fn f64(&mut self) -> f64 {
        f64::from_bits(self.i64() as u64)
    }

    pub fn blob(&mut self, n: usize) -> &'a [u8] {
        self.need(n);
        let v = &self.s[self.p..self.p + n];
        self.p += n;
        v
    }

    pub fn peek_u32(&self) -> u32 {
        if self.remaining() < 4 {
            return 0;
        }
        u32::from_le_bytes(self.s[self.p..self.p + 4].try_into().unwrap())
    }

    pub fn rest(&self) -> &'a [u8] {
        &self.s[self.p..]
    }

    /// Vector element count (ctor already consumed by the caller).
    pub fn vec_count(&mut self) -> i32 {
        self.i32()
    }

    pub fn bytes(&mut self) -> &'a [u8] {
        // TL `bytes` shares the string wire format: 1-byte length for <254,
        // else 0xFE + 3-byte little-endian length, all padded to 4 bytes.
        self.str()
    }

    pub fn str(&mut self) -> &'a [u8] {
        let l0 = self.blob(1)[0] as usize;
        let (len, hdr) = if l0 < 254 {
            (l0, 1)
        } else if l0 == 254 {
            let raw = self.blob(3);
            (raw[0] as usize | (raw[1] as usize) << 8 | (raw[2] as usize) << 16, 4)
        } else {
            panic!("tl: bad string prefix");
        };
        let v = self.blob(len);
        let pad = (4 - ((hdr + len) % 4)) % 4;
        let _ = self.blob(pad);
        v
    }
}
