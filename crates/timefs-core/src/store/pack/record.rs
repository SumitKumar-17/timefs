//! Record framing of the pack log, and scanning a segment for its valid prefix.
//!
//! ```text
//! record = magic:u8 | type:u8 | len:u32 | payload[len] | check:[u8; 8]
//! check  = first 8 bytes of BLAKE3(type | len | payload)
//! object payload  = hash[32] | kind:u8 | data
//! ref-set payload = hash[32] | name (UTF-8)
//! ref-del payload = name (UTF-8)
//! ```
//!
//! A crash can tear only the end of the active segment. Scanning stops at the first record
//! that is incomplete or fails its check, so everything after a torn record (including a
//! ref record that might name a lost object) is ignored as a unit.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};

use crate::hash::{Hash, HASH_LEN};
use crate::object::ObjectKind;

pub const MAGIC: u8 = 0xA5;
pub const HEADER_LEN: u64 = 6;
pub const CHECK_LEN: u64 = 8;
/// Bytes of an object payload before its data: the hash and the kind.
pub const OBJECT_PREFIX: u64 = HASH_LEN as u64 + 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum RecordType {
    Object = 1,
    RefSet = 2,
    RefDelete = 3,
}

impl RecordType {
    fn from_u8(b: u8) -> Option<Self> {
        match b {
            1 => Some(RecordType::Object),
            2 => Some(RecordType::RefSet),
            3 => Some(RecordType::RefDelete),
            _ => None,
        }
    }
}

pub fn object(hash: &Hash, kind: ObjectKind, data: &[u8]) -> Vec<u8> {
    frame(RecordType::Object, &[&hash.0, &[kind as u8], data])
}

pub fn ref_set(name: &str, hash: &Hash) -> Vec<u8> {
    frame(RecordType::RefSet, &[&hash.0, name.as_bytes()])
}

pub fn ref_delete(name: &str) -> Vec<u8> {
    frame(RecordType::RefDelete, &[name.as_bytes()])
}

fn frame(ty: RecordType, parts: &[&[u8]]) -> Vec<u8> {
    let len: usize = parts.iter().map(|p| p.len()).sum();
    let mut out = Vec::with_capacity(HEADER_LEN as usize + len + CHECK_LEN as usize);
    out.push(MAGIC);
    out.push(ty as u8);
    out.extend_from_slice(&(len as u32).to_le_bytes());
    for p in parts {
        out.extend_from_slice(p);
    }
    let check = checksum(&out[1..HEADER_LEN as usize], &out[HEADER_LEN as usize..]);
    out.extend_from_slice(&check);
    out
}

fn checksum(type_and_len: &[u8], payload: &[u8]) -> [u8; 8] {
    let mut h = blake3::Hasher::new();
    h.update(type_and_len);
    h.update(payload);
    let mut out = [0u8; 8];
    out.copy_from_slice(&h.finalize().as_bytes()[..8]);
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    /// `data_offset` is the absolute file offset of the object's data.
    Object {
        hash: Hash,
        kind: ObjectKind,
        data_offset: u64,
        data_len: u32,
    },
    RefSet {
        name: String,
        hash: Hash,
    },
    RefDelete {
        name: String,
    },
}

pub struct Scan {
    pub entries: Vec<Entry>,
    /// Length of the valid prefix; equals the file length when nothing is torn.
    pub valid_len: u64,
}

/// Parse records from the start of `file`. With `verify_objects`, object records are
/// checksummed too (needed for the active segment, which a crash may have torn); without
/// it their data is skipped, because a segment is fsynced before the next one starts.
/// Ref records are always verified; they are small.
pub fn scan(file: &File, verify_objects: bool) -> io::Result<Scan> {
    let file_len = file.metadata()?.len();
    let mut reader = BufReader::with_capacity(1 << 20, file);
    // Always parse from the start, wherever the handle's cursor happens to be.
    reader.seek(SeekFrom::Start(0))?;
    let mut entries = Vec::new();
    let mut off = 0u64;
    while off + HEADER_LEN + CHECK_LEN <= file_len {
        let mut header = [0u8; HEADER_LEN as usize];
        reader.read_exact(&mut header)?;
        let Some(ty) = RecordType::from_u8(header[1]).filter(|_| header[0] == MAGIC) else {
            break;
        };
        let len = u64::from(u32::from_le_bytes(header[2..6].try_into().unwrap()));
        if off + HEADER_LEN + len + CHECK_LEN > file_len {
            break;
        }
        let entry = if ty == RecordType::Object && !verify_objects {
            read_object_unverified(&mut reader, off, len)?
        } else {
            read_verified(&mut reader, ty, &header[1..], off, len)?
        };
        let Some(entry) = entry else { break };
        entries.push(entry);
        off += HEADER_LEN + len + CHECK_LEN;
    }
    Ok(Scan {
        entries,
        valid_len: off,
    })
}

/// Read the hash and kind of an object record and skip its data and check.
fn read_object_unverified<R: Read + Seek>(
    reader: &mut BufReader<R>,
    off: u64,
    len: u64,
) -> io::Result<Option<Entry>> {
    if len < OBJECT_PREFIX {
        return Ok(None);
    }
    let mut prefix = [0u8; OBJECT_PREFIX as usize];
    reader.read_exact(&mut prefix)?;
    reader.seek_relative((len - OBJECT_PREFIX + CHECK_LEN) as i64)?;
    Ok(object_entry(&prefix, off, len))
}

fn read_verified<R: Read>(
    reader: &mut R,
    ty: RecordType,
    type_and_len: &[u8],
    off: u64,
    len: u64,
) -> io::Result<Option<Entry>> {
    let mut payload = vec![0u8; len as usize];
    reader.read_exact(&mut payload)?;
    let mut check = [0u8; CHECK_LEN as usize];
    reader.read_exact(&mut check)?;
    if check != checksum(type_and_len, &payload) {
        return Ok(None);
    }
    Ok(match ty {
        RecordType::Object => object_entry(&payload, off, len),
        RecordType::RefSet if payload.len() >= HASH_LEN => {
            let mut hash = [0u8; HASH_LEN];
            hash.copy_from_slice(&payload[..HASH_LEN]);
            String::from_utf8(payload[HASH_LEN..].to_vec())
                .ok()
                .map(|name| Entry::RefSet {
                    name,
                    hash: Hash(hash),
                })
        }
        RecordType::RefSet => None,
        RecordType::RefDelete => String::from_utf8(payload)
            .ok()
            .map(|name| Entry::RefDelete { name }),
    })
}

fn object_entry(payload_prefix: &[u8], off: u64, len: u64) -> Option<Entry> {
    if (payload_prefix.len() as u64) < OBJECT_PREFIX {
        return None;
    }
    let mut hash = [0u8; HASH_LEN];
    hash.copy_from_slice(&payload_prefix[..HASH_LEN]);
    let kind = ObjectKind::from_u8(payload_prefix[HASH_LEN]).ok()?;
    Some(Entry::Object {
        hash: Hash(hash),
        kind,
        data_offset: off + HEADER_LEN + OBJECT_PREFIX,
        data_len: (len - OBJECT_PREFIX) as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn file_with(records: &[Vec<u8>]) -> (tempfile::NamedTempFile, u64) {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for r in records {
            f.write_all(r).unwrap();
        }
        let len = f.as_file().metadata().unwrap().len();
        (f, len)
    }

    fn sample() -> Vec<Vec<u8>> {
        let h = Hash::of_object(ObjectKind::Blob, b"hello");
        vec![
            object(&h, ObjectKind::Blob, b"hello"),
            ref_set("heads/main", &h),
            ref_delete("wip/main"),
        ]
    }

    #[test]
    fn round_trips_every_record_type() {
        let (f, len) = file_with(&sample());
        for verify in [true, false] {
            let scan = scan(f.as_file(), verify).unwrap();
            assert_eq!(scan.valid_len, len);
            assert_eq!(scan.entries.len(), 3);
            let Entry::Object {
                kind,
                data_offset,
                data_len,
                ..
            } = &scan.entries[0]
            else {
                panic!("first entry is an object");
            };
            assert_eq!(
                (*kind, *data_offset, *data_len),
                (ObjectKind::Blob, HEADER_LEN + OBJECT_PREFIX, 5)
            );
            assert!(matches!(&scan.entries[1], Entry::RefSet { name, .. } if name == "heads/main"));
            assert!(matches!(&scan.entries[2], Entry::RefDelete { name } if name == "wip/main"));
        }
    }

    #[test]
    fn stops_at_a_torn_tail_at_every_cut_point() {
        let records = sample();
        let whole: Vec<u8> = records.concat();
        let first_two = (records[0].len() + records[1].len()) as u64;
        for cut in first_two as usize..whole.len() {
            let (f, _) = file_with(&[whole[..cut].to_vec()]);
            let scan = scan(f.as_file(), true).unwrap();
            assert_eq!(scan.valid_len, first_two, "cut at {cut}");
            assert_eq!(scan.entries.len(), 2);
        }
    }

    #[test]
    fn a_flipped_byte_invalidates_the_record_and_everything_after() {
        let records = sample();
        let mut whole: Vec<u8> = records.concat();
        whole[HEADER_LEN as usize + 40] ^= 1; // inside the object data
        let (f, _) = file_with(&[whole]);
        let verified = scan(f.as_file(), true).unwrap();
        assert_eq!((verified.valid_len, verified.entries.len()), (0, 0));
    }

    #[test]
    fn garbage_and_empty_files_are_handled() {
        let (f, _) = file_with(&[vec![0u8; 100]]);
        assert_eq!(scan(f.as_file(), true).unwrap().valid_len, 0);
        let (f, _) = file_with(&[]);
        assert!(scan(f.as_file(), false).unwrap().entries.is_empty());
    }
}
