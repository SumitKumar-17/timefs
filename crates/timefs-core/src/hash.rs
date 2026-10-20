//! Object identity: BLAKE3-256 over `kind || payload`.

use std::fmt;
use std::str::FromStr;

use crate::error::Error;
use crate::object::ObjectKind;

pub const HASH_LEN: usize = 32;

/// A 256-bit content hash. Ordering is bytewise (used for deterministic tie-breaks).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hash(pub [u8; HASH_LEN]);

impl Hash {
    /// Hash of an object: `BLAKE3(kind || payload)`.
    pub fn of_object(kind: ObjectKind, payload: &[u8]) -> Hash {
        let mut h = blake3::Hasher::new();
        h.update(&[kind as u8]);
        h.update(payload);
        Hash(*h.finalize().as_bytes())
    }

    pub fn to_hex(&self) -> String {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut s = String::with_capacity(HASH_LEN * 2);
        for b in self.0 {
            s.push(DIGITS[(b >> 4) as usize] as char);
            s.push(DIGITS[(b & 15) as usize] as char);
        }
        s
    }

    /// First 12 hex characters, for display.
    pub fn short(&self) -> String {
        self.to_hex()[..12].to_string()
    }

    pub fn from_hex(s: &str) -> Result<Hash, Error> {
        let bytes = s.as_bytes();
        if bytes.len() != HASH_LEN * 2 {
            return Err(Error::corrupt(format!("bad hash length {}", bytes.len())));
        }
        let mut out = [0u8; HASH_LEN];
        for (i, pair) in bytes.chunks_exact(2).enumerate() {
            out[i] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
        }
        Ok(Hash(out))
    }
}

fn nibble(c: u8) -> Result<u8, Error> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        _ => Err(Error::corrupt("bad hex digit (lowercase hex required)")),
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash({})", self.short())
    }
}

impl FromStr for Hash {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        Hash::from_hex(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let h = Hash::of_object(ObjectKind::Blob, b"hello");
        assert_eq!(Hash::from_hex(&h.to_hex()).unwrap(), h);
        assert_eq!(h.to_hex().len(), 64);
    }

    #[test]
    fn kind_is_part_of_identity() {
        assert_ne!(
            Hash::of_object(ObjectKind::Blob, b"x"),
            Hash::of_object(ObjectKind::File, b"x")
        );
    }

    #[test]
    fn rejects_bad_hex() {
        assert!(Hash::from_hex("zz").is_err());
        assert!(Hash::from_hex(&"A".repeat(64)).is_err());
        assert!(Hash::from_hex(&"a".repeat(63)).is_err());
    }
}
