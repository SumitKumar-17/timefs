//! A read cache in front of any object store.
//!
//! Objects are immutable and named by their content hash, so a cached copy can never go
//! stale; the only invalidation needed is when an object is deleted. The cache is a
//! two-generation approximation of LRU: lookups hit the young generation first, then the
//! old one (promoting on a hit); when the young generation fills half the budget it becomes
//! the old one and the previous old generation is dropped. All operations are O(1).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::error::Result;
use crate::hash::Hash;
use crate::object::ObjectKind;
use crate::store::{ObjectInfo, ObjectStore, StoreStats, SweepReport};

type Entry = (ObjectKind, Arc<Vec<u8>>);

#[derive(Default)]
struct Generations {
    young: HashMap<Hash, Entry>,
    old: HashMap<Hash, Entry>,
    young_bytes: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
}

pub struct CachedStore {
    inner: Arc<dyn ObjectStore>,
    capacity: usize,
    gens: Mutex<Generations>,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl CachedStore {
    /// Cache up to roughly `capacity` bytes of object data in front of `inner`.
    pub fn new(inner: Arc<dyn ObjectStore>, capacity: usize) -> Self {
        CachedStore {
            inner,
            capacity,
            gens: Mutex::new(Generations::default()),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    pub fn cache_stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
        }
    }

    fn lookup(&self, hash: &Hash) -> Option<Entry> {
        let mut g = self.gens.lock().unwrap();
        if let Some(e) = g.young.get(hash) {
            return Some(e.clone());
        }
        let promoted = g.old.remove(hash)?;
        g.young_bytes += promoted.1.len();
        g.young.insert(*hash, promoted.clone());
        Some(promoted)
    }

    fn insert(&self, hash: Hash, entry: Entry) {
        // An object that would take a large share of the budget is not worth caching.
        if entry.1.len() > self.capacity / 4 {
            return;
        }
        let mut g = self.gens.lock().unwrap();
        g.young_bytes += entry.1.len();
        g.young.insert(hash, entry);
        if g.young_bytes > self.capacity / 2 {
            g.old = std::mem::take(&mut g.young);
            g.young_bytes = 0;
        }
    }

    fn evict(&self, hash: &Hash) {
        let mut g = self.gens.lock().unwrap();
        if let Some(e) = g.young.remove(hash) {
            g.young_bytes = g.young_bytes.saturating_sub(e.1.len());
        }
        g.old.remove(hash);
    }
}

impl ObjectStore for CachedStore {
    fn put(&self, kind: ObjectKind, payload: &[u8]) -> Result<Hash> {
        self.inner.put(kind, payload)
    }

    fn get(&self, hash: &Hash) -> Result<(ObjectKind, Vec<u8>)> {
        if let Some((kind, data)) = self.lookup(hash) {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return Ok((kind, data.as_ref().clone()));
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        let (kind, data) = self.inner.get(hash)?;
        self.insert(*hash, (kind, Arc::new(data.clone())));
        Ok((kind, data))
    }

    fn has(&self, hash: &Hash) -> Result<bool> {
        self.inner.has(hash)
    }

    fn delete(&self, hash: &Hash) -> Result<()> {
        self.evict(hash);
        self.inner.delete(hash)
    }

    fn list(&self) -> Result<Vec<ObjectInfo>> {
        self.inner.list()
    }

    fn list_prefix(&self, hex_prefix: &str) -> Result<Vec<Hash>> {
        self.inner.list_prefix(hex_prefix)
    }

    fn sync(&self) -> Result<()> {
        self.inner.sync()
    }

    fn sweep(
        &self,
        keep: &HashSet<Hash>,
        grace: std::time::Duration,
        dry_run: bool,
    ) -> Result<SweepReport> {
        let report = self.inner.sweep(keep, grace, dry_run)?;
        if !dry_run {
            // Deleted objects must not be served from memory afterwards.
            *self.gens.lock().unwrap() = Generations::default();
        }
        Ok(report)
    }

    fn stats(&self) -> StoreStats {
        self.inner.stats()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{LooseStore, ObjectStoreExt};

    fn setup(capacity: usize) -> (tempfile::TempDir, Arc<LooseStore>, CachedStore) {
        let d = tempfile::tempdir().unwrap();
        let disk = Arc::new(LooseStore::open(d.path().join("objects")).unwrap());
        let cached = CachedStore::new(disk.clone(), capacity);
        (d, disk, cached)
    }

    #[test]
    fn repeated_reads_are_served_from_memory() {
        let (_d, disk, cached) = setup(1 << 20);
        let h = cached.put_blob(b"hello").unwrap();
        assert_eq!(cached.get_blob(&h).unwrap(), b"hello");
        assert_eq!(cached.get_blob(&h).unwrap(), b"hello");
        assert_eq!(cached.get_blob(&h).unwrap(), b"hello");
        assert_eq!(
            disk.stats().gets,
            1,
            "only the first read may touch the disk"
        );
        assert_eq!(cached.cache_stats(), CacheStats { hits: 2, misses: 1 });
    }

    #[test]
    fn kind_is_preserved_for_cached_objects() {
        let (_d, _disk, cached) = setup(1 << 20);
        let h = cached.put_blob(b"x").unwrap();
        cached.get(&h).unwrap();
        assert_eq!(cached.get(&h).unwrap().0, ObjectKind::Blob);
        assert!(
            cached.get_tree(&h).is_err(),
            "kind checks still apply to cached data"
        );
    }

    #[test]
    fn delete_evicts() {
        let (_d, _disk, cached) = setup(1 << 20);
        let h = cached.put_blob(b"gone soon").unwrap();
        cached.get(&h).unwrap();
        cached.delete(&h).unwrap();
        assert!(cached.get(&h).is_err());
    }

    #[test]
    fn memory_stays_within_the_budget() {
        let (_d, disk, cached) = setup(64 * 1024);
        let hashes: Vec<Hash> = (0..200u8)
            .map(|i| cached.put_blob(&[i; 1000]).unwrap())
            .collect();
        for h in &hashes {
            cached.get(h).unwrap();
        }
        let g = cached.gens.lock().unwrap();
        let held: usize = g
            .young
            .values()
            .chain(g.old.values())
            .map(|e| e.1.len())
            .sum();
        assert!(held <= 64 * 1024, "held {held} bytes");
        drop(g);
        let before = disk.stats().gets;
        cached.get(hashes.last().unwrap()).unwrap();
        assert_eq!(
            disk.stats().gets,
            before,
            "the most recent object must still be cached"
        );
    }

    #[test]
    fn oversized_objects_are_not_cached() {
        let (_d, disk, cached) = setup(1000);
        let h = cached.put_blob(&[1u8; 600]).unwrap();
        cached.get(&h).unwrap();
        cached.get(&h).unwrap();
        assert_eq!(disk.stats().gets, 2);
    }
}
