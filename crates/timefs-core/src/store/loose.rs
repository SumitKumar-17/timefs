//! Loose-object backend: one immutable file per object, `objects/ab/cdef...`.
//!
//! Writes go to `objects/tmp/` and are renamed into place, so readers never observe a
//! partially written object. Durability is batched: [`ObjectStore::sync`] fsyncs everything
//! written since the previous sync (and the directories holding it), which keeps `put`
//! cheap while still honouring the "objects durable before refs" invariant.

use std::collections::{BTreeSet, HashSet};
use std::fs::{self, File};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::hash::{Hash, HASH_LEN};
use crate::object::ObjectKind;
use crate::store::{sweep_by_deletion, ObjectInfo, ObjectStore, StoreStats, SweepReport};

const TMP_DIR: &str = "tmp";

#[derive(Default)]
struct Counters {
    puts: AtomicU64,
    new_objects: AtomicU64,
    new_bytes: AtomicU64,
    dedup_hits: AtomicU64,
    dedup_bytes: AtomicU64,
    gets: AtomicU64,
}

pub struct LooseStore {
    root: PathBuf,
    verify_reads: bool,
    /// Objects created or reused since the last `sync`. Reused objects are included because
    /// an object left by a crashed process may exist without ever having been made durable.
    unsynced: Mutex<HashSet<PathBuf>>,
    tmp_seq: AtomicU64,
    counters: Counters,
}

impl LooseStore {
    /// Open (creating if needed) a store rooted at `objects_dir`.
    pub fn open(objects_dir: impl Into<PathBuf>) -> Result<Self> {
        let root = objects_dir.into();
        fs::create_dir_all(root.join(TMP_DIR))?;
        Ok(LooseStore {
            root,
            verify_reads: true,
            unsynced: Mutex::new(HashSet::new()),
            tmp_seq: AtomicU64::new(0),
            counters: Counters::default(),
        })
    }

    /// Re-hash every object on read (default on). Disabling trades integrity for speed.
    pub fn with_verify_reads(mut self, verify: bool) -> Self {
        self.verify_reads = verify;
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn object_path(&self, hash: &Hash) -> PathBuf {
        let hex = hash.to_hex();
        self.root.join(&hex[..2]).join(&hex[2..])
    }

    fn tmp_path(&self) -> PathBuf {
        let seq = self.tmp_seq.fetch_add(1, Ordering::Relaxed);
        self.root
            .join(TMP_DIR)
            .join(format!("{}-{}", std::process::id(), seq))
    }

    fn write_new(&self, path: &Path, kind: ObjectKind, payload: &[u8]) -> Result<()> {
        let tmp = self.tmp_path();
        let written = (|| -> std::io::Result<()> {
            let mut f = File::create(&tmp)?;
            f.write_all(&[kind as u8])?;
            f.write_all(payload)
        })();
        if let Err(e) = written {
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::rename(&tmp, path)?;
        Ok(())
    }
}

impl ObjectStore for LooseStore {
    fn put(&self, kind: ObjectKind, payload: &[u8]) -> Result<Hash> {
        let hash = Hash::of_object(kind, payload);
        let stored_len = payload.len() as u64 + 1;
        self.counters.puts.fetch_add(1, Ordering::Relaxed);

        let path = self.object_path(&hash);
        if is_intact(&path, stored_len)? {
            self.counters.dedup_hits.fetch_add(1, Ordering::Relaxed);
            self.counters
                .dedup_bytes
                .fetch_add(stored_len, Ordering::Relaxed);
        } else {
            self.write_new(&path, kind, payload)?;
            self.counters.new_objects.fetch_add(1, Ordering::Relaxed);
            self.counters
                .new_bytes
                .fetch_add(stored_len, Ordering::Relaxed);
        }
        self.unsynced.lock().unwrap().insert(path);
        Ok(hash)
    }

    fn get(&self, hash: &Hash) -> Result<(ObjectKind, Vec<u8>)> {
        self.counters.gets.fetch_add(1, Ordering::Relaxed);
        let mut file = match File::open(self.object_path(hash)) {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::NotFound => return Err(Error::NotFound(*hash)),
            Err(e) => return Err(e.into()),
        };
        let mut tag = [0u8; 1];
        file.read_exact(&mut tag)
            .map_err(|_| Error::corrupt(format!("object {hash} is empty")))?;
        let kind = ObjectKind::from_u8(tag[0])?;
        let mut payload = Vec::new();
        file.read_to_end(&mut payload)?;
        if self.verify_reads {
            let actual = Hash::of_object(kind, &payload);
            if actual != *hash {
                return Err(Error::HashMismatch {
                    expected: *hash,
                    actual,
                });
            }
        }
        Ok((kind, payload))
    }

    fn has(&self, hash: &Hash) -> Result<bool> {
        Ok(self.object_path(hash).exists())
    }

    fn delete(&self, hash: &Hash) -> Result<()> {
        match fs::remove_file(self.object_path(hash)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn list(&self) -> Result<Vec<ObjectInfo>> {
        let mut out = Vec::new();
        for fan in fanout_dirs(&self.root)? {
            for entry in fs::read_dir(fan.path())? {
                let entry = entry?;
                let Some(hash) = hash_from_paths(&fan.file_name(), &entry.file_name()) else {
                    continue;
                };
                let meta = entry.metadata()?;
                out.push(ObjectInfo {
                    hash,
                    size: meta.len(),
                    modified: meta.modified()?,
                });
            }
        }
        Ok(out)
    }

    fn list_prefix(&self, hex_prefix: &str) -> Result<Vec<Hash>> {
        if hex_prefix.len() < 2 || !hex_prefix.is_ascii() {
            return Err(Error::Invalid(
                "hash prefix must be at least 2 hex characters".into(),
            ));
        }
        let dir = self.root.join(&hex_prefix[..2]);
        let mut out = Vec::new();
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let name = entry?.file_name();
            if let Some(h) = hash_from_paths(std::ffi::OsStr::new(&hex_prefix[..2]), &name) {
                if h.to_hex().starts_with(hex_prefix) {
                    out.push(h);
                }
            }
        }
        out.sort();
        Ok(out)
    }

    fn sync(&self) -> Result<()> {
        let pending: Vec<PathBuf> = std::mem::take(&mut *self.unsynced.lock().unwrap())
            .into_iter()
            .collect();
        if pending.is_empty() {
            return Ok(());
        }
        let result = sync_paths(&self.root, &pending);
        if result.is_err() {
            // Keep them pending: a later sync must not report success over these objects.
            self.unsynced.lock().unwrap().extend(pending);
        }
        result
    }

    fn sweep(&self, keep: &HashSet<Hash>, grace: Duration, dry_run: bool) -> Result<SweepReport> {
        let mut report = sweep_by_deletion(self, keep, grace, dry_run)?;
        if !dry_run {
            report.temp_files_removed = self.clean_temp(grace)?;
        }
        Ok(report)
    }

    fn stats(&self) -> StoreStats {
        let c = &self.counters;
        StoreStats {
            puts: c.puts.load(Ordering::Relaxed),
            new_objects: c.new_objects.load(Ordering::Relaxed),
            new_bytes: c.new_bytes.load(Ordering::Relaxed),
            dedup_hits: c.dedup_hits.load(Ordering::Relaxed),
            dedup_bytes: c.dedup_bytes.load(Ordering::Relaxed),
            gets: c.gets.load(Ordering::Relaxed),
        }
    }
}

impl LooseStore {
    /// Remove leftover temporary files of interrupted writes older than `older_than`.
    fn clean_temp(&self, older_than: Duration) -> Result<u64> {
        let mut removed = 0;
        for entry in fs::read_dir(self.root.join(TMP_DIR))? {
            let entry = entry?;
            let age = entry.metadata()?.modified()?.elapsed().unwrap_or_default();
            if age >= older_than {
                fs::remove_file(entry.path())?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

/// Two parallel rounds: file contents, then the directories naming them (and the store root,
/// which names new fan-out directories). Measured slightly faster than one combined round.
fn sync_paths(root: &Path, files: &[PathBuf]) -> Result<()> {
    fsync_all(files)?;
    let mut dirs: BTreeSet<PathBuf> = files
        .iter()
        .filter_map(|p| p.parent().map(Path::to_path_buf))
        .collect();
    dirs.insert(root.to_path_buf());
    fsync_all(&dirs.into_iter().collect::<Vec<_>>())
}

/// Whether a complete object already exists at `path`. A wrong length means a write was
/// torn by a crash, so the caller writes the object again.
fn is_intact(path: &Path, expected_len: u64) -> Result<bool> {
    match fs::metadata(path) {
        Ok(meta) => Ok(meta.len() == expected_len),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Below this many paths, spawning threads costs more than it saves.
const PARALLEL_SYNC_MIN: usize = 2;
const MAX_SYNC_THREADS: usize = 16;

/// fsync every path, concurrently. Concurrent fsyncs on one filesystem are merged into
/// shared journal commits, so this is much faster than syncing one file at a time.
fn fsync_all(paths: &[PathBuf]) -> Result<()> {
    if paths.len() < PARALLEL_SYNC_MIN {
        return paths.iter().try_for_each(|p| fsync_path(p));
    }
    let threads = paths.len().min(MAX_SYNC_THREADS);
    let per_thread = paths.len().div_ceil(threads);
    std::thread::scope(|scope| {
        let workers: Vec<_> = paths
            .chunks(per_thread)
            .map(|chunk| scope.spawn(move || chunk.iter().try_for_each(|p| fsync_path(p))))
            .collect();
        workers
            .into_iter()
            .try_for_each(|w| w.join().expect("fsync worker panicked"))
    })
}

fn fsync_path(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

/// The two-hex-digit fan-out directories (everything except `tmp`).
fn fanout_dirs(root: &Path) -> Result<Vec<fs::DirEntry>> {
    let mut dirs = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        if name.len() == 2 && entry.file_type()?.is_dir() {
            dirs.push(entry);
        }
    }
    Ok(dirs)
}

/// Rebuild a hash from `ab` + `cdef...`; `None` for anything that is not an object name.
fn hash_from_paths(fan: &std::ffi::OsStr, name: &std::ffi::OsStr) -> Option<Hash> {
    let (fan, name) = (fan.to_str()?, name.to_str()?);
    if fan.len() != 2 || name.len() != HASH_LEN * 2 - 2 {
        return None;
    }
    Hash::from_hex(&format!("{fan}{name}")).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ObjectStoreExt;
    use std::sync::Arc;

    fn store() -> (tempfile::TempDir, LooseStore) {
        let dir = tempfile::tempdir().unwrap();
        let s = LooseStore::open(dir.path().join("objects")).unwrap();
        (dir, s)
    }

    #[test]
    fn put_get_round_trip() {
        let (_d, s) = store();
        let h = s.put_blob(b"hello").unwrap();
        assert_eq!(s.get_blob(&h).unwrap(), b"hello");
        assert!(s.has(&h).unwrap());
    }

    #[test]
    fn identical_puts_are_deduplicated() {
        let (_d, s) = store();
        let a = s.put_blob(b"same").unwrap();
        let b = s.put_blob(b"same").unwrap();
        assert_eq!(a, b);
        let st = s.stats();
        assert_eq!((st.puts, st.new_objects, st.dedup_hits), (2, 1, 1));
        assert_eq!(s.list().unwrap().len(), 1);
    }

    #[test]
    fn missing_object_is_not_found() {
        let (_d, s) = store();
        let h = Hash([7; 32]);
        assert!(matches!(s.get(&h), Err(Error::NotFound(_))));
        assert!(!s.has(&h).unwrap());
        s.delete(&h).unwrap();
    }

    #[test]
    fn corruption_is_detected_on_read() {
        let (_d, s) = store();
        let h = s.put_blob(b"precious data").unwrap();
        let path = s.object_path(&h);
        let mut bytes = fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 0xff;
        fs::write(&path, bytes).unwrap();
        assert!(matches!(s.get(&h), Err(Error::HashMismatch { .. })));
    }

    #[test]
    fn wrong_kind_is_reported() {
        let (_d, s) = store();
        let h = s.put_blob(b"x").unwrap();
        assert!(matches!(s.get_tree(&h), Err(Error::WrongKind { .. })));
    }

    #[test]
    fn list_and_prefix_lookup() {
        let (_d, s) = store();
        let hashes: Vec<Hash> = (0..20u8).map(|i| s.put_blob(&[i]).unwrap()).collect();
        assert_eq!(s.list().unwrap().len(), 20);
        let target = hashes[3];
        let found = s.list_prefix(&target.to_hex()[..8]).unwrap();
        assert_eq!(found, vec![target]);
        assert!(s.list_prefix("z").is_err());
    }

    #[test]
    fn sync_clears_pending_and_leaves_no_tmp_files() {
        let (_d, s) = store();
        for i in 0..5u8 {
            s.put_blob(&[i]).unwrap();
        }
        s.sync().unwrap();
        assert!(s.unsynced.lock().unwrap().is_empty());
        assert_eq!(fs::read_dir(s.root().join(TMP_DIR)).unwrap().count(), 0);
    }

    #[test]
    fn a_torn_object_is_rewritten_instead_of_trusted() {
        let (_d, s) = store();
        let h = s.put_blob(b"important bytes").unwrap();
        // simulate a crash that left the object file empty (ext4 delayed allocation)
        fs::write(s.object_path(&h), b"").unwrap();
        assert_eq!(s.put_blob(b"important bytes").unwrap(), h);
        assert_eq!(s.get_blob(&h).unwrap(), b"important bytes");
        assert_eq!(
            s.stats().new_objects,
            2,
            "the torn object must be written again"
        );
    }

    #[test]
    fn reused_objects_are_synced_before_publication() {
        let (_d, s) = store();
        let h = s.put_blob(b"left by another process").unwrap();
        s.sync().unwrap();
        s.put_blob(b"left by another process").unwrap();
        assert!(s.unsynced.lock().unwrap().contains(&s.object_path(&h)));
        s.sync().unwrap();
        assert!(s.unsynced.lock().unwrap().is_empty());
    }

    #[test]
    fn a_failed_sync_keeps_objects_pending() {
        let (_d, s) = store();
        let h = s.put_blob(b"will vanish").unwrap();
        s.put_blob(b"stays").unwrap();
        fs::remove_file(s.object_path(&h)).unwrap(); // makes the fsync fail
        assert!(s.sync().is_err());
        assert_eq!(
            s.unsynced.lock().unwrap().len(),
            2,
            "nothing may be dropped on failure"
        );
    }

    #[test]
    fn concurrent_identical_puts_are_safe() {
        let (_d, s) = store();
        let s = Arc::new(s);
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let s = Arc::clone(&s);
                std::thread::spawn(move || s.put_blob(&vec![42u8; 10_000]).unwrap())
            })
            .collect();
        let hashes: Vec<Hash> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(hashes.windows(2).all(|w| w[0] == w[1]));
        assert_eq!(s.get_blob(&hashes[0]).unwrap(), vec![42u8; 10_000]);
        assert_eq!(s.list().unwrap().len(), 1);
    }
}
