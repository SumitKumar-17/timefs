//! Canonical binary encoding primitives (see `docs/format.md`).
//!
//! The encoding is what gets hashed, so it must be canonical: every value has exactly one
//! encoding and decoders reject anything else (non-minimal varints, trailing bytes).

use crate::error::{Error, Result};
use crate::hash::{Hash, HASH_LEN};

#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(n: usize) -> Self {
        Writer {
            buf: Vec::with_capacity(n),
        }
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i64(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    /// Unsigned LEB128, always minimal.
    pub fn varint(&mut self, mut v: u64) {
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.buf.push(byte);
                return;
            }
            self.buf.push(byte | 0x80);
        }
    }

    pub fn bytes(&mut self, b: &[u8]) {
        self.varint(b.len() as u64);
        self.buf.extend_from_slice(b);
    }

    pub fn hash(&mut self, h: &Hash) {
        self.buf.extend_from_slice(&h.0);
    }

    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(Error::corrupt("unexpected end of data"));
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    pub fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    /// Unsigned LEB128; rejects non-minimal encodings and overflow.
    pub fn varint(&mut self) -> Result<u64> {
        let mut value: u64 = 0;
        for i in 0..10 {
            let byte = self.u8()?;
            let low = u64::from(byte & 0x7f);
            if i == 9 && low > 1 {
                return Err(Error::corrupt("varint overflows u64"));
            }
            value |= low << (7 * i);
            if byte & 0x80 == 0 {
                if i > 0 && byte == 0 {
                    return Err(Error::corrupt("non-minimal varint"));
                }
                return Ok(value);
            }
        }
        Err(Error::corrupt("varint too long"))
    }

    /// A length-prefixed byte string. The length is checked against the remaining input
    /// before anything is allocated.
    pub fn bytes(&mut self) -> Result<&'a [u8]> {
        let n = self.varint()?;
        if n > self.remaining() as u64 {
            return Err(Error::corrupt("length prefix exceeds input"));
        }
        self.take(n as usize)
    }

    pub fn hash(&mut self) -> Result<Hash> {
        let mut h = [0u8; HASH_LEN];
        h.copy_from_slice(self.take(HASH_LEN)?);
        Ok(Hash(h))
    }

    /// Fail if input remains (canonical encodings have no trailing bytes).
    pub fn finish(self) -> Result<()> {
        if self.remaining() != 0 {
            return Err(Error::corrupt("trailing bytes after object"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn varint_round_trip(v in any::<u64>()) {
            let mut w = Writer::new();
            w.varint(v);
            let bytes = w.finish();
            let mut r = Reader::new(&bytes);
            prop_assert_eq!(r.varint().unwrap(), v);
            r.finish().unwrap();
        }

        #[test]
        fn bytes_round_trip(b in proptest::collection::vec(any::<u8>(), 0..300)) {
            let mut w = Writer::new();
            w.bytes(&b);
            let enc = w.finish();
            let mut r = Reader::new(&enc);
            prop_assert_eq!(r.bytes().unwrap(), &b[..]);
            r.finish().unwrap();
        }

        #[test]
        fn decoder_never_panics(garbage in proptest::collection::vec(any::<u8>(), 0..64)) {
            let mut r = Reader::new(&garbage);
            let _ = r.varint();
            let _ = r.bytes();
            let _ = r.hash();
        }
    }

    #[test]
    fn rejects_non_minimal_varint() {
        // 1 encoded as 0x81 0x00 (two bytes) is non-canonical.
        assert!(Reader::new(&[0x81, 0x00]).varint().is_err());
        assert_eq!(Reader::new(&[0x00]).varint().unwrap(), 0);
    }

    #[test]
    fn rejects_overflowing_varint() {
        let mut v = vec![0xff; 9];
        v.push(0x02);
        assert!(Reader::new(&v).varint().is_err());
        let mut ok = vec![0xff; 9];
        ok.push(0x01);
        assert_eq!(Reader::new(&ok).varint().unwrap(), u64::MAX);
    }

    #[test]
    fn rejects_trailing_bytes_and_huge_length() {
        let mut r = Reader::new(&[1, 2]);
        r.u8().unwrap();
        assert!(r.finish().is_err());
        // length prefix claims 1 GiB but nothing follows
        let mut w = Writer::new();
        w.varint(1 << 30);
        let enc = w.finish();
        assert!(Reader::new(&enc).bytes().is_err());
    }
}
