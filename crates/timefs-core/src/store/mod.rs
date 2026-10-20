//! Content-addressed object storage.
//!
//! [`ObjectStore`] is the backend boundary: the rest of the engine only sees this trait, so
//! alternative backends (for example a packed store, see ADR-0003) stay a local change.

mod cached;
mod loose;
mod typed;

use std::time::SystemTime;

use crate::error::Result;
use crate::hash::Hash;
use crate::object::ObjectKind;

pub use cached::{CacheStats, CachedStore};
pub use loose::LooseStore;
pub use typed::ObjectStoreExt;

/// Metadata about one stored object, produced by [`ObjectStore::list`].
#[derive(Clone, Copy, Debug)]
pub struct ObjectInfo {
    pub hash: Hash,
    /// Stored size in bytes (includes the one-byte kind tag).
    pub size: u64,
    pub modified: SystemTime,
}

/// Cumulative counters since the store was opened. Used by stats and benchmarks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoreStats {
    pub puts: u64,
    pub new_objects: u64,
    pub new_bytes: u64,
    pub dedup_hits: u64,
    pub dedup_bytes: u64,
    pub gets: u64,
}

pub trait ObjectStore: Send + Sync {
    /// Store an object and return its hash. Idempotent: storing existing data is a no-op.
    fn put(&self, kind: ObjectKind, payload: &[u8]) -> Result<Hash>;

    /// Fetch an object. Implementations may verify the content against its hash.
    fn get(&self, hash: &Hash) -> Result<(ObjectKind, Vec<u8>)>;

    fn has(&self, hash: &Hash) -> Result<bool>;

    fn delete(&self, hash: &Hash) -> Result<()>;

    /// Every stored object (order unspecified).
    fn list(&self) -> Result<Vec<ObjectInfo>>;

    /// Hashes whose hex form starts with `hex_prefix` (at least two characters).
    fn list_prefix(&self, hex_prefix: &str) -> Result<Vec<Hash>>;

    /// Make every object stored so far durable. Must be called before a ref is moved to
    /// point at them (the write ordering invariant).
    fn sync(&self) -> Result<()>;

    /// Remove leftover temporary files from interrupted writes older than `older_than`.
    /// Returns how many were removed. Backends without temporary files do nothing.
    fn clean_temp(&self, _older_than: std::time::Duration) -> Result<u64> {
        Ok(0)
    }

    fn stats(&self) -> StoreStats;
}
