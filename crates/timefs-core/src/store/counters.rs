//! Operation counters shared by the store backends.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::store::StoreStats;

#[derive(Default)]
pub(crate) struct Counters {
    puts: AtomicU64,
    new_objects: AtomicU64,
    new_bytes: AtomicU64,
    dedup_hits: AtomicU64,
    dedup_bytes: AtomicU64,
    gets: AtomicU64,
}

impl Counters {
    pub fn stored_new(&self, bytes: u64) {
        self.puts.fetch_add(1, Ordering::Relaxed);
        self.new_objects.fetch_add(1, Ordering::Relaxed);
        self.new_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn deduplicated(&self, bytes: u64) {
        self.puts.fetch_add(1, Ordering::Relaxed);
        self.dedup_hits.fetch_add(1, Ordering::Relaxed);
        self.dedup_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn read(&self) {
        self.gets.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> StoreStats {
        let load = |c: &AtomicU64| c.load(Ordering::Relaxed);
        StoreStats {
            puts: load(&self.puts),
            new_objects: load(&self.new_objects),
            new_bytes: load(&self.new_bytes),
            dedup_hits: load(&self.dedup_hits),
            dedup_bytes: load(&self.dedup_bytes),
            gets: load(&self.gets),
        }
    }
}
