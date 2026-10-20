//! The four immutable object kinds and their canonical encodings (see `docs/format.md`).

use crate::codec::{Reader, Writer};
use crate::error::{Error, Result};
use crate::hash::{Hash, HASH_LEN};

pub const MAX_NAME_LEN: usize = 255;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ObjectKind {
    Blob = 1,
    File = 2,
    Tree = 3,
    Commit = 4,
}

impl ObjectKind {
    pub fn from_u8(b: u8) -> Result<Self> {
        match b {
            1 => Ok(ObjectKind::Blob),
            2 => Ok(ObjectKind::File),
            3 => Ok(ObjectKind::Tree),
            4 => Ok(ObjectKind::Commit),
            _ => Err(Error::corrupt(format!("unknown object kind {b}"))),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ObjectKind::Blob => "blob",
            ObjectKind::File => "file",
            ObjectKind::Tree => "tree",
            ObjectKind::Commit => "commit",
        }
    }
}

/// A run of file data stored as one blob.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extent {
    pub hash: Hash,
    pub len: u64,
}

/// A regular file: the concatenation of its extents. An empty file has no extents.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileObj {
    pub extents: Vec<Extent>,
}

impl FileObj {
    pub fn size(&self) -> u64 {
        self.extents.iter().map(|e| e.len).sum()
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(self.extents.len() * (HASH_LEN + 3) + 4);
        w.varint(self.extents.len() as u64);
        for e in &self.extents {
            w.hash(&e.hash);
            w.varint(e.len);
        }
        w.finish()
    }

    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut r = Reader::new(payload);
        let n = r.varint()?;
        // each extent needs at least HASH_LEN + 1 bytes: bound the allocation by the input
        if n > (r.remaining() / (HASH_LEN + 1)) as u64 {
            return Err(Error::corrupt("extent count exceeds input"));
        }
        let mut extents = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let hash = r.hash()?;
            let len = r.varint()?;
            extents.push(Extent { hash, len });
        }
        r.finish()?;
        Ok(FileObj { extents })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum EntryKind {
    File = 0,
    Dir = 1,
    Symlink = 2,
}

impl EntryKind {
    fn from_u8(b: u8) -> Result<Self> {
        match b {
            0 => Ok(EntryKind::File),
            1 => Ok(EntryKind::Dir),
            2 => Ok(EntryKind::Symlink),
            _ => Err(Error::corrupt(format!("unknown entry kind {b}"))),
        }
    }
}

/// One directory entry. Metadata lives here (in the parent) so `stat` never has to load the
/// child object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeEntry {
    pub name: Vec<u8>,
    pub kind: EntryKind,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub mtime_ns: i64,
    pub hash: Hash,
}

impl TreeEntry {
    /// "Same thing" for merge purposes: kind, content hash, mode, owner. mtime is ignored.
    pub fn same_content(&self, other: &TreeEntry) -> bool {
        self.kind == other.kind
            && self.hash == other.hash
            && self.mode == other.mode
            && self.uid == other.uid
            && self.gid == other.gid
    }
}

pub fn validate_name(name: &[u8]) -> Result<()> {
    let bad = name.is_empty()
        || name.len() > MAX_NAME_LEN
        || name == b"."
        || name == b".."
        || name.contains(&b'/')
        || name.contains(&0);
    if bad {
        return Err(Error::InvalidName(
            String::from_utf8_lossy(name).into_owned(),
        ));
    }
    Ok(())
}

/// A directory: entries sorted bytewise by name, names unique.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tree {
    entries: Vec<TreeEntry>,
}

impl Tree {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build from arbitrary entries: validates names, sorts, rejects duplicates.
    pub fn from_entries(mut entries: Vec<TreeEntry>) -> Result<Self> {
        for e in &entries {
            validate_name(&e.name)?;
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        if entries.windows(2).any(|w| w[0].name == w[1].name) {
            return Err(Error::Invalid("duplicate name in tree".into()));
        }
        Ok(Tree { entries })
    }

    pub fn entries(&self) -> &[TreeEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, name: &[u8]) -> Option<&TreeEntry> {
        self.entries
            .binary_search_by(|e| e.name.as_slice().cmp(name))
            .ok()
            .map(|i| &self.entries[i])
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(self.entries.len() * 80 + 4);
        w.varint(self.entries.len() as u64);
        for e in &self.entries {
            w.bytes(&e.name);
            w.u8(e.kind as u8);
            w.u32(e.mode);
            w.u32(e.uid);
            w.u32(e.gid);
            w.u64(e.size);
            w.i64(e.mtime_ns);
            w.hash(&e.hash);
        }
        w.finish()
    }

    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut r = Reader::new(payload);
        let n = r.varint()?;
        // minimum encoded entry: 1 (name len) + 1 (name) + 1 + 4 + 4 + 4 + 8 + 8 + 32
        const MIN_ENTRY: usize = 1 + 1 + 1 + 4 + 4 + 4 + 8 + 8 + HASH_LEN;
        if n > (r.remaining() / MIN_ENTRY) as u64 {
            return Err(Error::corrupt("entry count exceeds input"));
        }
        let mut entries: Vec<TreeEntry> = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let name = r.bytes()?.to_vec();
            validate_name(&name).map_err(|_| Error::corrupt("invalid entry name in tree"))?;
            let kind = EntryKind::from_u8(r.u8()?)?;
            let entry = TreeEntry {
                name,
                kind,
                mode: r.u32()?,
                uid: r.u32()?,
                gid: r.u32()?,
                size: r.u64()?,
                mtime_ns: r.i64()?,
                hash: r.hash()?,
            };
            if let Some(prev) = entries.last() {
                if prev.name >= entry.name {
                    return Err(Error::corrupt("tree entries not strictly sorted"));
                }
            }
            entries.push(entry);
        }
        r.finish()?;
        Ok(Tree { entries })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub tree: Hash,
    pub parents: Vec<Hash>,
    pub time_ns: i64,
    pub author: String,
    pub message: String,
}

impl Commit {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.hash(&self.tree);
        w.varint(self.parents.len() as u64);
        for p in &self.parents {
            w.hash(p);
        }
        w.i64(self.time_ns);
        w.bytes(self.author.as_bytes());
        w.bytes(self.message.as_bytes());
        w.finish()
    }

    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut r = Reader::new(payload);
        let tree = r.hash()?;
        let np = r.varint()?;
        if np > (r.remaining() / HASH_LEN) as u64 || np > 2 {
            return Err(Error::corrupt("bad parent count"));
        }
        let mut parents = Vec::with_capacity(np as usize);
        for _ in 0..np {
            parents.push(r.hash()?);
        }
        let time_ns = r.i64()?;
        let author = utf8(r.bytes()?)?;
        let message = utf8(r.bytes()?)?;
        r.finish()?;
        Ok(Commit {
            tree,
            parents,
            time_ns,
            author,
            message,
        })
    }
}

fn utf8(b: &[u8]) -> Result<String> {
    String::from_utf8(b.to_vec()).map_err(|_| Error::corrupt("invalid UTF-8 in commit"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn h(n: u8) -> Hash {
        Hash([n; HASH_LEN])
    }

    fn entry(name: &str, kind: EntryKind) -> TreeEntry {
        TreeEntry {
            name: name.as_bytes().to_vec(),
            kind,
            mode: 0o644,
            uid: 1000,
            gid: 1000,
            size: 7,
            mtime_ns: 123,
            hash: h(9),
        }
    }

    #[test]
    fn tree_round_trip_and_sorting() {
        let t = Tree::from_entries(vec![
            entry("b", EntryKind::File),
            entry("a", EntryKind::Dir),
            entry("c", EntryKind::Symlink),
        ])
        .unwrap();
        let names: Vec<_> = t.entries().iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]);
        assert_eq!(Tree::decode(&t.encode()).unwrap(), t);
        assert_eq!(t.get(b"b").unwrap().kind, EntryKind::File);
        assert!(t.get(b"zz").is_none());
    }

    #[test]
    fn tree_rejects_duplicates_and_bad_names() {
        assert!(Tree::from_entries(vec![
            entry("a", EntryKind::File),
            entry("a", EntryKind::Dir)
        ])
        .is_err());
        for bad in ["", ".", "..", "a/b", "a\0b"] {
            assert!(validate_name(bad.as_bytes()).is_err(), "{bad:?}");
        }
        assert!(validate_name(&vec![b'x'; 256]).is_err());
        assert!(validate_name(&vec![b'x'; 255]).is_ok());
    }

    #[test]
    fn tree_decode_rejects_unsorted_input() {
        let mut t = Tree::from_entries(vec![
            entry("a", EntryKind::File),
            entry("b", EntryKind::File),
        ])
        .unwrap();
        t.entries.swap(0, 1);
        assert!(Tree::decode(&t.encode()).is_err());
    }

    #[test]
    fn file_round_trip() {
        let f = FileObj {
            extents: vec![
                Extent {
                    hash: h(1),
                    len: 65536,
                },
                Extent {
                    hash: h(2),
                    len: 17,
                },
            ],
        };
        assert_eq!(f.size(), 65553);
        assert_eq!(FileObj::decode(&f.encode()).unwrap(), f);
        assert_eq!(
            FileObj::decode(&FileObj::default().encode()).unwrap(),
            FileObj::default()
        );
    }

    #[test]
    fn commit_round_trip() {
        let c = Commit {
            tree: h(1),
            parents: vec![h(2), h(3)],
            time_ns: -5,
            author: "me".into(),
            message: "héllo ✓".into(),
        };
        assert_eq!(Commit::decode(&c.encode()).unwrap(), c);
    }

    #[test]
    fn commit_rejects_octopus() {
        let mut c = Commit {
            tree: h(1),
            parents: vec![h(2), h(3), h(4)],
            time_ns: 0,
            author: String::new(),
            message: String::new(),
        };
        assert!(Commit::decode(&c.encode()).is_err());
        c.parents.truncate(2);
        assert!(Commit::decode(&c.encode()).is_ok());
    }

    proptest! {
        #[test]
        fn decoders_never_panic(data in proptest::collection::vec(any::<u8>(), 0..200)) {
            let _ = FileObj::decode(&data);
            let _ = Tree::decode(&data);
            let _ = Commit::decode(&data);
        }

        #[test]
        fn file_extents_round_trip(lens in proptest::collection::vec(0u64..1_000_000, 0..20)) {
            let f = FileObj {
                extents: lens.iter().enumerate().map(|(i, &len)| Extent { hash: h(i as u8), len }).collect(),
            };
            prop_assert_eq!(FileObj::decode(&f.encode()).unwrap(), f);
        }
    }
}
