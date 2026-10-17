//! Minimal big unsigned integers for the DH/RSA math of the handshake.
//! u32 limbs, little-endian order; zero = empty limb vector.

#![allow(dead_code)]
use std::cmp::Ordering;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Big {
    limbs: Vec<u32>,
}

impl Big {
    pub fn zero() -> Big {
        Big { limbs: vec![] }
    }

    pub fn is_zero(&self) -> bool {
        self.limbs.is_empty()
    }

    pub fn from_limbs(mut limbs: Vec<u32>) -> Big {
        while limbs.last() == Some(&0) {
            limbs.pop();
        }
        Big { limbs }
    }

    pub fn from_bytes_be(bytes: &[u8]) -> Big {
        let mut limbs = Vec::with_capacity(bytes.len() / 4 + 1);
        let mut acc = 0u32;
        let mut shift = 0;
        for b in bytes.iter().rev() {
            acc |= (*b as u32) << shift;
            shift += 8;
            if shift == 32 {
                limbs.push(acc);
                acc = 0;
                shift = 0;
            }
        }
        if shift > 0 {
            limbs.push(acc);
        }
        Big::from_limbs(limbs)
    }

    /// Exactly `out_len` bytes big-endian (left-padded with zeros).
    pub fn to_bytes_be(&self, out_len: usize) -> Vec<u8> {
        let mut out = vec![0u8; out_len];
        for (i, l) in self.limbs.iter().enumerate() {
            for (j, b) in l.to_le_bytes().iter().enumerate() {
                let idx = i * 4 + j;
                if idx < out_len {
                    out[out_len - 1 - idx] = *b;
                }
            }
        }
        out
    }

    pub fn cmp_to(&self, other: &Big) -> Ordering {
        if self.limbs.len() != other.limbs.len() {
            return self.limbs.len().cmp(&other.limbs.len());
        }
        for i in (0..self.limbs.len()).rev() {
            if self.limbs[i] != other.limbs[i] {
                return self.limbs[i].cmp(&other.limbs[i]);
            }
        }
        Ordering::Equal
    }

    pub fn num_bits(&self) -> usize {
        match self.limbs.last() {
            None => 0,
            Some(top) => self.limbs.len() * 32 - (top.leading_zeros() as usize),
        }
    }

    fn bit(&self, i: usize) -> bool {
        let limb = i / 32;
        if limb >= self.limbs.len() {
            return false;
        }
        (self.limbs[limb] >> (i % 32)) & 1 == 1
    }

    /// r <- (r << 1) + bit; then reduce mod m (at most a few subtractions
    /// because r < m before the operation and the added value is < m).
    fn shift_add_reduce(r: &Big, bit: u32, add: &Big, m: &Big) -> Big {
        // r2 = r * 2 + bit * add
        let mut out = vec![0u32; r.limbs.len() + 2];
        let mut carry = 0u64;
        for (i, limb) in r.limbs.iter().enumerate() {
            let v = (*limb as u64) << 1 | carry;
            out[i] = v as u32;
            carry = v >> 32;
        }
        if carry != 0 {
            out[r.limbs.len()] = carry as u32;
        }
        if bit == 1 {
            let mut acc = 0u64;
            for i in 0..add.limbs.len().max(out.len()) {
                let a = *out.get(i).unwrap_or(&0) as u64;
                let b = *add.limbs.get(i).unwrap_or(&0) as u64;
                let v = a + b + acc;
                if i < out.len() {
                    out[i] = v as u32;
                } else {
                    out.push(v as u32);
                }
                acc = v >> 32;
            }
            if acc != 0 {
                match out.last_mut() {
                    Some(t) if *t == 0 => *t = acc as u32,
                    _ => out.push(acc as u32),
                }
            }
        }

        let mut val = Big::from_limbs(out);
        while val.cmp_to(m) != Ordering::Less {
            val = subtract(&val, m);
        }
        val
    }

    /// (a * b) mod m via bit-by-bit multiply-add-reduce.
    pub fn mulmod(a: &Big, b: &Big, m: &Big) -> Big {
        let mut r = Big::zero();
        let bits = b.num_bits();
        if bits == 0 {
            return Big::zero();
        }
        for i in (0..bits).rev() {
            let bit = if b.bit(i) { 1u32 } else { 0u32 };
            r = Big::shift_add_reduce(&r, bit, a, m);
        }
        r
    }

    /// base^exp mod m, square-and-multiply MSB-first.
    pub fn mod_pow(base: &Big, exp: &Big, m: &Big) -> Big {
        if m.is_zero() {
            return Big::zero();
        }
        let bits = exp.num_bits();
        let mut r = Big::from_limbs(vec![1]);
        if bits == 0 {
            return Big::mod_pow_once(r, m);
        }
        for i in (0..bits).rev() {
            r = Big::mulmod(&r, &r, m);
            if exp.bit(i) {
                r = Big::mulmod(&r, base, m);
            }
        }
        r
    }

    fn mod_pow_once(v: Big, m: &Big) -> Big {
        let mut val = v;
        while val.cmp_to(m) != Ordering::Less {
            val = subtract(&val, m);
        }
        val
    }
}

fn subtract(a: &Big, b: &Big) -> Big {
    let mut out = Vec::with_capacity(a.limbs.len());
    let mut borrow = 0i64;
    for i in 0..a.limbs.len() {
        let mut v = a.limbs[i] as i64 - borrow;
        if i < b.limbs.len() {
            v -= b.limbs[i] as i64;
        }
        if v < 0 {
            v += 1i64 << 32;
            borrow = 1;
        } else {
            borrow = 0;
        }
        out.push(v as u32);
    }
    Big::from_limbs(out)
}

/// Pollard's rho (Brent variant) over u64: factors the composite pq (<= 2^64)
/// returning (p, q) with p <= q, or (0, 0) on failure.
pub fn factorize_u64(n: u64) -> (u64, u64) {
    if n % 2 == 0 {
        return (2, n / 2);
    }
    if n < 3 {
        return (0, 0);
    }
    for c in 1u64..32 {
        // Brent's parameters
        let mut y: u64 = 2;
        let m: u64 = 128;
        let mut g: u64 = 1;
        let mut r: u64 = 1;
        let mut q: u64 = 1;
        let mut iter: u64 = 0;
        while g == 1 && iter < 1_000_000 {
            let x = y;
            let mut i = 0u64;
            while i < r && iter < 1_000_000 {
                y = rho_step(y, c, n);
                iter += 1;
                i += 1;
            }
            let mut k = 0u64;
            while k < r && g == 1 && iter < 1_000_000 {
                let ys = y;
                let limit = (r - k).min(m);
                let mut j = 0u64;
                while j < limit {
                    y = rho_step(y, c, n);
                    let diff = x.abs_diff(y);
                    if diff == 0 {
                        q = 0;
                    } else {
                        q = mulmod64(q, diff, n);
                    }
                    iter += 1;
                    j += 1;
                    if q == 0 || iter >= 1_000_000 {
                        break;
                    }
                }
                if q != 0 {
                    g = gcd64(q, n);
                } else {
                    let diff = x.abs_diff(ys);
                    g = gcd64(diff, n);
                }
                k += limit;
            }
            r <<= 1;
        }
        if g != n && g != 1 && g != 0 {
            let p = g;
            let q = n / g;
            return if p <= q { (p, q) } else { (q, p) };
        }
    }
    (0, 0)
}

fn gcd64(a: u64, b: u64) -> u64 {
    let mut a = a;
    let mut b = b;
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

fn mulmod64(a: u64, b: u64, m: u64) -> u64 {
    (((a as u128) * (b as u128)) % (m as u128)) as u64
}

/// f(x) = (x*x + c) mod n (n odd composite).
fn rho_step(x: u64, c: u64, n: u64) -> u64 {
    let sq = mulmod64(x, x, n);
    let c = c % n;
    if sq >= n - c {
        sq - (n - c)
    } else {
        sq + c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn big(v: u64) -> Big {
        Big::from_bytes_be(&v.to_be_bytes())
    }

    #[test]
    fn bytes_roundtrip() {
        let bytes: Vec<u8> = (0u8..32).collect();
        let b = Big::from_bytes_be(&bytes);
        assert_eq!(b.to_bytes_be(32), bytes);
    }

    #[test]
    fn mod_pow_known_values() {
        // 2^10 mod 1000 = 24
        assert_eq!(Big::mod_pow(&big(2), &big(10), &big(1000)), big(24));
        // (3^3)^2 = 729 mod 998; chain check via mulmod identity
        assert_eq!(Big::mulmod(&big(3), &big(3), &big(998)), big(9));
        assert_eq!(Big::mod_pow(&big(3), &big(1), &big(998)), big(3));
        // Fermat: a^(p-1) ≡ 1 (mod p)
        assert_eq!(Big::mod_pow(&big(7), &big(10), &big(11)), big(1));
        assert_eq!(Big::mod_pow(&big(3), &big(10), &big(1000)), big(49));
    }

    #[test]
    fn mod_pow_vs_bruteforce() {
        for (b, e, m) in [(7u64, 33u64, 1009u64), (2, 128, 65521), (1234567, 9, 999983)] {
            let mut acc = 1u128;
            for _ in 0..e {
                acc = acc * (b as u128) % (m as u128);
            }
            assert_eq!(Big::mod_pow(&big(b), &big(e), &big(m)), big(acc as u64), "{}^{} mod {}", b, e, m);
        }
    }

    #[test]
    fn mulmod_large_limbs() {
        let m = Big::from_bytes_be(&[0xffu8; 256]);
        let a = Big::from_bytes_be(&[0xfeu8; 256]);
        let r = Big::mulmod(&a, &a, &m);
        assert!(r.cmp_to(&m) == Ordering::Less);
    }

    #[test]
    fn mod_pow_dh_sized() {
        // 2048-bit base^2048-bit exp mod the real DH prime: result must be
        // a full-viewport (< prime, > 2^2040) number of the right bit width.
        let prime = Big::from_bytes_be(&[
            0xc7, 0x1c, 0xae, 0xb9, 0xc6, 0xb1, 0xc9, 0x04, 0x8e, 0x6c, 0x52, 0x2f, 0x70, 0xf1,
            0x3f, 0x73, 0x98, 0x0d, 0x40, 0x23, 0x8e, 0x3e, 0x21, 0xc1, 0x49, 0x34, 0xd0, 0x37,
            0x56, 0x3d, 0x93, 0x0f, 0x48, 0x19, 0x8a, 0x0a, 0xa7, 0xc1, 0x40, 0x58, 0x22, 0x94,
            0x93, 0xd2, 0x25, 0x30, 0xf4, 0xdb, 0xfa, 0x33, 0x6f, 0x6e, 0x0a, 0xc9, 0x25, 0x13,
            0x95, 0x43, 0xae, 0xd4, 0x4c, 0xce, 0x7c, 0x37, 0x20, 0xfd, 0x51, 0xf6, 0x94, 0x58,
            0x70, 0x5a, 0xc6, 0x8c, 0xd4, 0xfe, 0x6b, 0x6b, 0x13, 0xab, 0xdc, 0x97, 0x46, 0x51,
            0x29, 0x69, 0x32, 0x84, 0x54, 0xf1, 0x8f, 0xaf, 0x8c, 0x59, 0x5f, 0x64, 0x24, 0x77,
            0xfe, 0x96, 0xbb, 0x2a, 0x94, 0x1d, 0x5b, 0xcd, 0x1d, 0x4a, 0xc8, 0xcc, 0x49, 0x88,
            0x07, 0x08, 0xfa, 0x9b, 0x37, 0x8e, 0x3c, 0x4f, 0x3a, 0x90, 0x60, 0xbe, 0xe6, 0x7c,
            0xf9, 0xa4, 0xa4, 0xa6, 0x95, 0x81, 0x10, 0x51, 0x90, 0x7e, 0x16, 0x27, 0x53, 0xb5,
            0x6b, 0x0f, 0x6b, 0x41, 0x0d, 0xba, 0x74, 0xd8, 0xa8, 0x4b, 0x2a, 0x14, 0xb3, 0x14,
            0x4e, 0x0e, 0xf1, 0x28, 0x47, 0x54, 0xfd, 0x17, 0xed, 0x95, 0x0d, 0x59, 0x65, 0xb4,
            0xb9, 0xdd, 0x46, 0x58, 0x2d, 0xb1, 0x17, 0x8d, 0x16, 0x9c, 0x6b, 0xc4, 0x65, 0xb0,
            0xd6, 0xff, 0x9c, 0xa3, 0x92, 0x8f, 0xef, 0x5b, 0x9a, 0xe4, 0xe4, 0x18, 0xfc, 0x15,
            0xe8, 0x3e, 0xbe, 0xa0, 0xf8, 0x7f, 0xa9, 0xff, 0x5e, 0xed, 0x70, 0x05, 0x0d, 0xed,
            0x28, 0x49, 0xf4, 0x7b, 0xf9, 0x59, 0xd9, 0x56, 0x85, 0x0c, 0xe9, 0x29, 0x85, 0x1f,
            0x0d, 0x81, 0x15, 0xf6, 0x35, 0xb1, 0x05, 0xee, 0x2e, 0x4e, 0x15, 0xd0, 0x4b, 0x24,
            0x54, 0xbf, 0x6f, 0x4f, 0xad, 0xf0, 0x34, 0xb1, 0x04, 0x03, 0x11, 0x9c, 0xd8, 0xe3,
            0xb9, 0x2f, 0xcc, 0x5b,
        ]);
        let g = Big::from_bytes_be(&[3u8]);
        let b = Big::from_bytes_be(&[
            0x3d, 0x5a, 0x14, 0xc7, 0xa9, 0x62, 0x0b, 0x51, 0x7c, 0x33, 0xe8, 0xd1, 0x4f, 0xa2,
            0x66, 0x9b, 0x1d, 0x78, 0xc4, 0x2e, 0x53, 0xa7, 0x8b, 0x16, 0x2c, 0xe9, 0x41, 0x5d,
            0x70, 0xf3, 0x24, 0x97, 0x6a, 0x18, 0xdc, 0x5b, 0x83, 0x2f, 0x9e, 0x64, 0x71, 0xa5,
            0x0c, 0xd8, 0x36, 0xef, 0x72, 0x4b, 0x89, 0x15, 0x2d, 0x60, 0xfa, 0x93, 0x37, 0xc1,
            0x58, 0xad, 0x0e, 0x62, 0xb4, 0x7d, 0x21, 0x8f, 0x43, 0x56, 0xce, 0x90, 0x1b, 0x74,
            0xe5, 0x28, 0xa3, 0x4f, 0x97, 0x10, 0x63, 0xda, 0x2c, 0x55, 0xb8, 0x0d, 0x69, 0xf2,
            0x3e, 0x81, 0x44, 0x7a, 0x1f, 0x95, 0x60, 0x2b, 0xd3, 0x76, 0xeb, 0x12, 0x59, 0xa4,
            0x87, 0x30, 0x09, 0xf6, 0x4d, 0x22, 0xbb, 0x5e, 0x19, 0xa0, 0x6d, 0x34, 0x7f, 0xc5,
            0x28, 0x91, 0x52, 0xdb, 0x05, 0x68, 0xea, 0x3b, 0x94, 0x27, 0x7a, 0x1c, 0x83, 0xf0,
            0x45, 0x6b, 0xd9, 0x2e, 0x13, 0x8c, 0x5f, 0x40, 0x96, 0x21, 0xac, 0x57, 0x3d, 0x08,
            0x79, 0x16, 0xca, 0x3f, 0xa2, 0x5b, 0x04, 0x75, 0xe8, 0x29, 0x90, 0x43, 0x76, 0xdf,
            0x1a, 0x93, 0x26, 0x69, 0xfc, 0x37, 0x80, 0x4b, 0x9e, 0x17, 0x60, 0xc9, 0x32, 0x85,
            0xde, 0x07, 0x48, 0xb1, 0x2a, 0x93, 0x1c, 0x6f, 0xf8, 0x31, 0x82, 0x25, 0x54, 0xa7,
            0x38, 0x09, 0x8a, 0x13, 0x46, 0xaf, 0x20, 0x71, 0xdc, 0x39, 0xa2, 0x0b, 0x74, 0xed,
            0x26, 0x97, 0x30, 0x89, 0xf2, 0x43, 0x5c, 0xb5, 0x0e, 0x87, 0xd0, 0x10, 0x2a, 0x93, 0x24,
            0x6d, 0xd6, 0x0f, 0x98, 0x21, 0x4a, 0xb3, 0x2c, 0x75, 0x0e, 0xa7, 0x30, 0xb9, 0x42,
        ]);
        let g_b = Big::mod_pow(&g, &b, &prime);
        assert!(g_b.cmp_to(&prime) == Ordering::Less);
        assert!(g_b.num_bits() >= 2040, "g_b too small: {} bits", g_b.num_bits());
        let g_ab = Big::mod_pow(&Big::from_bytes_be(&[2u8]), &b, &prime);
        assert!(g_ab.num_bits() >= 2000, "g_ab too small: {} bits", g_ab.num_bits());
    }

    #[test]
    fn factorize_classics() {
        assert_eq!(factorize_u64(21), (3, 7));
        // two ~32-bit primes (neither divisible by 3)
        let (p, q) = factorize_u64(4294967291u64 * 4294967279);
        assert_eq!((p, q), (4294967279, 4294967291));
        // 63-bit composite
        let (p, q) = factorize_u64(1_129_989_338_887_446_267);
        assert!(p > 1 && q > 1 && p.checked_mul(q) == Some(1_129_989_338_887_446_267));
        // even numbers split instantly
        assert_eq!(factorize_u64(2 * 97), (2, 97));
    }
}
