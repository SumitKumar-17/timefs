//! File data as a list of segments: the engine behind file reads, writes and truncation.
//!
//! A segment is stored (a blob in the repository, loaded on demand), in memory (modified
//! bytes), or a hole (zeros). Writes only touch the segments they overlap, so random writes
//! never rewrite a whole file.

use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::hash::Hash;
use crate::object::{Extent, FileObj};
use crate::store::{ObjectStore, ObjectStoreExt};

#[derive(Clone, Debug)]
pub enum Segment {
    Stored { hash: Hash, len: u64 },
    Mem(Vec<u8>),
    Hole(u64),
}

impl Segment {
    fn len(&self) -> u64 {
        match self {
            Segment::Stored { len, .. } | Segment::Hole(len) => *len,
            Segment::Mem(b) => b.len() as u64,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ExtentList {
    segs: Vec<Segment>,
    size: u64,
    mem_bytes: u64,
}

impl ExtentList {
    pub fn from_file(file: &FileObj) -> Self {
        ExtentList {
            segs: file
                .extents
                .iter()
                .map(|e| Segment::Stored {
                    hash: e.hash,
                    len: e.len,
                })
                .collect(),
            size: file.size(),
            mem_bytes: 0,
        }
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    /// Bytes held in memory that are not yet persisted.
    pub fn mem_bytes(&self) -> u64 {
        self.mem_bytes
    }

    /// Read up to `len` bytes at `off` (short at end of file).
    pub fn read(&self, store: &dyn ObjectStore, off: u64, len: usize) -> Result<Vec<u8>> {
        let end = off.saturating_add(len as u64).min(self.size);
        if off >= end {
            return Ok(Vec::new());
        }
        let mut out = Vec::with_capacity((end - off) as usize);
        let mut pos = 0u64;
        for seg in &self.segs {
            let seg_end = pos + seg.len();
            if seg_end > off && pos < end {
                let lo = (off.max(pos) - pos) as usize;
                let hi = (end.min(seg_end) - pos) as usize;
                match seg {
                    Segment::Stored { hash, len } => {
                        out.extend_from_slice(&load_blob(store, hash, *len)?[lo..hi])
                    }
                    Segment::Mem(buf) => out.extend_from_slice(&buf[lo..hi]),
                    Segment::Hole(_) => out.resize(out.len() + (hi - lo), 0),
                }
            }
            pos = seg_end;
            if pos >= end {
                break;
            }
        }
        Ok(out)
    }

    /// Write `data` at `off`, zero-filling any gap past the current end.
    pub fn write(
        &mut self,
        store: &dyn ObjectStore,
        off: u64,
        data: &[u8],
        block: usize,
    ) -> Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        if off > self.size {
            self.push_hole(off - self.size);
        }
        let overlay_end = (off + data.len() as u64).min(self.size);
        let split = (overlay_end - off) as usize;
        if split > 0 {
            self.overlay(store, off, &data[..split])?;
        }
        if split < data.len() {
            self.append(&data[split..], block);
        }
        Ok(())
    }

    /// Resize to exactly `new_size`; growing adds a hole of zeros.
    pub fn truncate(&mut self, store: &dyn ObjectStore, new_size: u64) -> Result<()> {
        if new_size >= self.size {
            self.push_hole(new_size - self.size);
            return Ok(());
        }
        let mut pos = 0u64;
        let mut keep = self.segs.len();
        for i in 0..self.segs.len() {
            let seg_end = pos + self.segs[i].len();
            if seg_end > new_size {
                if pos < new_size {
                    self.cut_segment(store, i, (new_size - pos) as usize)?;
                    keep = i + 1;
                } else {
                    keep = i;
                }
                break;
            }
            pos = seg_end;
        }
        self.segs.truncate(keep);
        self.size = new_size;
        self.mem_bytes = self.recount_mem();
        Ok(())
    }

    /// Store every in-memory byte and hole as blobs and return the file object. Afterwards
    /// every segment is `Stored` and no memory is held.
    pub fn persist(&mut self, store: &dyn ObjectStore, block: usize) -> Result<FileObj> {
        let mut extents = Vec::with_capacity(self.segs.len());
        let mut zero_blobs: HashMap<usize, Hash> = HashMap::new();
        for seg in &self.segs {
            match seg {
                Segment::Stored { hash, len } => extents.push(Extent {
                    hash: *hash,
                    len: *len,
                }),
                Segment::Mem(buf) => {
                    for chunk in buf.chunks(block) {
                        extents.push(Extent {
                            hash: store.put_blob(chunk)?,
                            len: chunk.len() as u64,
                        });
                    }
                }
                Segment::Hole(len) => {
                    let mut left = *len;
                    while left > 0 {
                        let n = left.min(block as u64) as usize;
                        let hash = match zero_blobs.get(&n) {
                            Some(h) => *h,
                            None => {
                                let h = store.put_blob(&vec![0u8; n])?;
                                zero_blobs.insert(n, h);
                                h
                            }
                        };
                        extents.push(Extent {
                            hash,
                            len: n as u64,
                        });
                        left -= n as u64;
                    }
                }
            }
        }
        self.segs = extents
            .iter()
            .map(|e| Segment::Stored {
                hash: e.hash,
                len: e.len,
            })
            .collect();
        self.mem_bytes = 0;
        Ok(FileObj { extents })
    }

    // ---- internals -----------------------------------------------------------------------

    fn push_hole(&mut self, n: u64) {
        if n == 0 {
            return;
        }
        match self.segs.last_mut() {
            Some(Segment::Hole(len)) => *len += n,
            _ => self.segs.push(Segment::Hole(n)),
        }
        self.size += n;
    }

    /// Append bytes, topping up a trailing in-memory segment before starting new blocks.
    fn append(&mut self, data: &[u8], block: usize) {
        let mut rest = data;
        if let Some(Segment::Mem(last)) = self.segs.last_mut() {
            let take = block.saturating_sub(last.len()).min(rest.len());
            last.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
        }
        for chunk in rest.chunks(block) {
            self.segs.push(Segment::Mem(chunk.to_vec()));
        }
        self.size += data.len() as u64;
        self.mem_bytes += data.len() as u64;
    }

    /// Overwrite `[off, off + data.len())`, which lies entirely inside the file.
    fn overlay(&mut self, store: &dyn ObjectStore, off: u64, data: &[u8]) -> Result<()> {
        let end = off + data.len() as u64;
        let mut i = 0;
        let mut pos = 0u64;
        while i < self.segs.len() && pos < end {
            let seg_len = self.segs[i].len();
            let seg_end = pos + seg_len;
            if seg_end > off {
                let lo = (off.max(pos) - pos) as usize;
                let hi = (end.min(seg_end) - pos) as usize;
                let src = &data[(off.max(pos) - off) as usize..][..hi - lo];
                i += self.overlay_segment(store, i, lo, hi, src)?;
            } else {
                i += 1;
            }
            pos = seg_end;
        }
        Ok(())
    }

    /// Overwrite bytes `lo..hi` of segment `i`; returns how many segments now stand in its place.
    fn overlay_segment(
        &mut self,
        store: &dyn ObjectStore,
        i: usize,
        lo: usize,
        hi: usize,
        src: &[u8],
    ) -> Result<usize> {
        match &mut self.segs[i] {
            Segment::Mem(buf) => {
                buf[lo..hi].copy_from_slice(src);
                Ok(1)
            }
            Segment::Stored { hash, len } => {
                let mut buf = load_blob(store, hash, *len)?;
                buf[lo..hi].copy_from_slice(src);
                self.mem_bytes += buf.len() as u64;
                self.segs[i] = Segment::Mem(buf);
                Ok(1)
            }
            Segment::Hole(len) => {
                // Only the overwritten part becomes real data; the rest stays a hole.
                let seg_len = *len as usize;
                let mut parts = Vec::with_capacity(3);
                if lo > 0 {
                    parts.push(Segment::Hole(lo as u64));
                }
                parts.push(Segment::Mem(src.to_vec()));
                if hi < seg_len {
                    parts.push(Segment::Hole((seg_len - hi) as u64));
                }
                self.mem_bytes += src.len() as u64;
                let n = parts.len();
                self.segs.splice(i..=i, parts);
                Ok(n)
            }
        }
    }

    /// Shorten segment `i` to `keep` bytes (`0 < keep < len`).
    fn cut_segment(&mut self, store: &dyn ObjectStore, i: usize, keep: usize) -> Result<()> {
        match &mut self.segs[i] {
            Segment::Mem(buf) => buf.truncate(keep),
            Segment::Hole(len) => *len = keep as u64,
            Segment::Stored { hash, len } => {
                let mut buf = load_blob(store, hash, *len)?;
                buf.truncate(keep);
                self.segs[i] = Segment::Mem(buf);
            }
        }
        Ok(())
    }

    fn recount_mem(&self) -> u64 {
        self.segs
            .iter()
            .map(|s| match s {
                Segment::Mem(b) => b.len() as u64,
                _ => 0,
            })
            .sum()
    }

    #[cfg(test)]
    fn check_invariants(&self) {
        let total: u64 = self.segs.iter().map(Segment::len).sum();
        assert_eq!(total, self.size, "size mismatch");
        assert_eq!(self.recount_mem(), self.mem_bytes, "mem_bytes drifted");
        assert!(self.segs.iter().all(|s| s.len() > 0), "empty segment");
    }
}

fn load_blob(store: &dyn ObjectStore, hash: &Hash, expected_len: u64) -> Result<Vec<u8>> {
    let blob = store.get_blob(hash)?;
    if blob.len() as u64 != expected_len {
        return Err(Error::corrupt(format!(
            "blob {hash} has length {} but the file records {expected_len}",
            blob.len()
        )));
    }
    Ok(blob)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::LooseStore;
    use proptest::prelude::*;

    const BLOCK: usize = 8;

    fn store() -> (tempfile::TempDir, LooseStore) {
        let d = tempfile::tempdir().unwrap();
        let s = LooseStore::open(d.path().join("objects")).unwrap();
        (d, s)
    }

    /// Reference implementation: a flat byte vector.
    fn model_write(model: &mut Vec<u8>, off: usize, data: &[u8]) {
        if model.len() < off + data.len() {
            model.resize(off + data.len(), 0);
        }
        model[off..off + data.len()].copy_from_slice(data);
    }

    #[test]
    fn write_read_across_blocks_and_gaps() {
        let (_d, s) = store();
        let mut l = ExtentList::default();
        l.write(&s, 0, b"hello world, long enough", BLOCK).unwrap();
        l.write(&s, 40, b"tail", BLOCK).unwrap(); // leaves a hole
        l.check_invariants();
        assert_eq!(l.size(), 44);
        let all = l.read(&s, 0, 100).unwrap();
        assert_eq!(&all[..24], b"hello world, long enough");
        assert!(all[24..40].iter().all(|&b| b == 0));
        assert_eq!(&all[40..], b"tail");
        assert_eq!(l.read(&s, 6, 5).unwrap(), b"world");
        assert!(l.read(&s, 500, 5).unwrap().is_empty());
    }

    #[test]
    fn persist_then_partial_overwrite_loads_only_what_it_touches() {
        let (_d, s) = store();
        let mut l = ExtentList::default();
        l.write(&s, 0, &[1u8; 64], BLOCK).unwrap();
        let obj = l.persist(&s, BLOCK).unwrap();
        assert_eq!(obj.extents.len(), 8);
        assert_eq!(l.mem_bytes(), 0);
        let mut l2 = ExtentList::from_file(&obj);
        l2.write(&s, 10, &[9u8; 3], BLOCK).unwrap();
        l2.check_invariants();
        assert_eq!(
            l2.mem_bytes(),
            8,
            "only one block should be loaded into memory"
        );
        let got = l2.read(&s, 8, 8).unwrap();
        assert_eq!(got, [1, 1, 9, 9, 9, 1, 1, 1]);
    }

    #[test]
    fn holes_persist_as_shared_zero_blobs() {
        let (_d, s) = store();
        let mut l = ExtentList::default();
        l.truncate(&s, 100).unwrap();
        let obj = l.persist(&s, BLOCK).unwrap();
        assert_eq!(obj.size(), 100);
        // 12 full zero blocks + a 4 byte tail: two distinct blobs
        let distinct: std::collections::HashSet<_> = obj.extents.iter().map(|e| e.hash).collect();
        assert_eq!(distinct.len(), 2);
        let back = ExtentList::from_file(&obj).read(&s, 0, 200).unwrap();
        assert_eq!(back, vec![0u8; 100]);
    }

    #[test]
    fn truncate_shrinks_inside_each_segment_kind() {
        let (_d, s) = store();
        let mut l = ExtentList::default();
        l.write(&s, 0, &[7u8; 20], BLOCK).unwrap();
        let obj = l.persist(&s, BLOCK).unwrap();
        let mut stored = ExtentList::from_file(&obj);
        stored.truncate(&s, 10).unwrap();
        stored.check_invariants();
        assert_eq!(stored.read(&s, 0, 99).unwrap(), vec![7u8; 10]);
        let mut holey = ExtentList::default();
        holey.truncate(&s, 50).unwrap();
        holey.truncate(&s, 5).unwrap();
        holey.check_invariants();
        assert_eq!(holey.size(), 5);
        holey.truncate(&s, 0).unwrap();
        assert_eq!(holey.size(), 0);
    }

    #[derive(Clone, Debug)]
    enum Op {
        Write(usize, Vec<u8>),
        Truncate(usize),
        Persist,
    }

    fn op_strategy() -> impl Strategy<Value = Op> {
        prop_oneof![
            4 => (0usize..120, proptest::collection::vec(any::<u8>(), 0..40)).prop_map(|(o, d)| Op::Write(o, d)),
            2 => (0usize..140).prop_map(Op::Truncate),
            1 => Just(Op::Persist),
        ]
    }

    proptest! {
        #[test]
        fn behaves_like_a_flat_byte_vector(ops in proptest::collection::vec(op_strategy(), 1..40)) {
            let (_d, s) = store();
            let mut list = ExtentList::default();
            let mut model: Vec<u8> = Vec::new();
            for op in ops {
                match op {
                    Op::Write(off, data) => {
                        list.write(&s, off as u64, &data, BLOCK).unwrap();
                        if !data.is_empty() { model_write(&mut model, off, &data); }
                    }
                    Op::Truncate(n) => {
                        list.truncate(&s, n as u64).unwrap();
                        model.resize(n, 0);
                    }
                    Op::Persist => {
                        let obj = list.persist(&s, BLOCK).unwrap();
                        list = ExtentList::from_file(&obj);
                    }
                }
                list.check_invariants();
                prop_assert_eq!(list.size() as usize, model.len());
                prop_assert_eq!(list.read(&s, 0, 1000).unwrap(), model.clone());
            }
        }
    }
}
