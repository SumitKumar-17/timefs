//! State shared by the FUSE handlers, the checkpointer and the control socket.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use fuser::FileType;
use timefs_core::worktree::WorkTree;
use timefs_core::{Hash, Result};

use crate::engine::{Engine, SnapshotOutcome};

/// One entry of an open directory listing.
pub struct DirItem {
    pub ino: u64,
    pub kind: FileType,
    pub name: OsString,
}

pub struct Shared {
    tree: Mutex<WorkTree>,
    engine: Engine,
    /// Checkpoint inline once this many dirty bytes are held in memory.
    flush_threshold: u64,
    /// Whether to checkpoint when the filesystem shuts down.
    final_checkpoint: bool,
    stats_path: PathBuf,
    dir_handles: Mutex<HashMap<u64, Arc<Vec<DirItem>>>>,
    next_fh: AtomicU64,
}

impl Shared {
    pub fn new(
        tree: WorkTree,
        engine: Engine,
        flush_threshold: u64,
        final_checkpoint: bool,
        stats_path: PathBuf,
    ) -> Self {
        Shared {
            tree: Mutex::new(tree),
            engine,
            flush_threshold,
            final_checkpoint,
            stats_path,
            dir_handles: Mutex::new(HashMap::new()),
            next_fh: AtomicU64::new(1),
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, WorkTree> {
        self.tree.lock().expect("working tree lock poisoned")
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn stats_path(&self) -> &std::path::Path {
        &self.stats_path
    }

    pub fn checkpoint(&self) -> Result<Option<Hash>> {
        let mut wt = self.lock();
        self.engine.checkpoint(&mut wt)
    }

    pub fn snapshot(&self, message: &str) -> Result<SnapshotOutcome> {
        let mut wt = self.lock();
        self.engine.snapshot(&mut wt, message)
    }

    /// Called after writes: bound the amount of unpersisted data held in memory.
    pub fn flush_if_needed(&self, wt: &mut WorkTree) {
        if wt.dirty_bytes() >= self.flush_threshold {
            if let Err(e) = self.engine.checkpoint(wt) {
                eprintln!("timefs: checkpoint failed: {e}");
            }
        }
    }

    /// Persist remaining changes at shutdown, logging a failure. Used by the FUSE
    /// `destroy` callback, which has no way to report an error; the mount owner calls
    /// [`Shared::finish_checked`] afterwards to learn the outcome.
    pub fn finish(&self) {
        if let Err(e) = self.finish_checked() {
            eprintln!("timefs: final checkpoint failed: {e}");
        }
    }

    /// Persist remaining changes at shutdown and report whether that worked. Idempotent
    /// and cheap when nothing changed since the last checkpoint, so it is safe to call
    /// after `finish` already ran; if that attempt failed, this one retries it. Does
    /// nothing when the final checkpoint is disabled.
    pub fn finish_checked(&self) -> Result<()> {
        if !self.final_checkpoint {
            return Ok(());
        }
        self.checkpoint().map(|_| ())
    }

    pub fn open_dir_handle(&self, items: Vec<DirItem>) -> u64 {
        let fh = self.next_fh.fetch_add(1, Ordering::Relaxed);
        self.dir_handles.lock().unwrap().insert(fh, Arc::new(items));
        fh
    }

    pub fn dir_handle(&self, fh: u64) -> Option<Arc<Vec<DirItem>>> {
        self.dir_handles.lock().unwrap().get(&fh).cloned()
    }

    pub fn close_dir_handle(&self, fh: u64) {
        self.dir_handles.lock().unwrap().remove(&fh);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use timefs_core::repo::StoreKind;
    use timefs_core::worktree::ROOT_INO;
    use timefs_core::Repository;

    use super::*;

    fn shared(final_checkpoint: bool) -> (tempfile::TempDir, Shared) {
        shared_in(StoreKind::Pack, final_checkpoint)
    }

    fn shared_in(kind: StoreKind, final_checkpoint: bool) -> (tempfile::TempDir, Shared) {
        let d = tempfile::tempdir().unwrap();
        let repo = Arc::new(Repository::init_with(d.path().join("repo"), kind).unwrap());
        let engine = Engine::new(repo, "main", "tester", 64 * 1024).unwrap();
        let (tree, _) = engine.open_tree().unwrap();
        let stats = d.path().join("repo");
        (
            d,
            Shared::new(tree, engine, u64::MAX, final_checkpoint, stats),
        )
    }

    fn write_file(s: &Shared, name: &str) {
        let mut wt = s.lock();
        let a = wt.create(ROOT_INO, name.as_bytes(), 0o644, 0, 0).unwrap();
        wt.write(a.ino, 0, b"data").unwrap();
    }

    fn has_checkpoint(s: &Shared) -> bool {
        s.engine().repo().has_checkpoint("main").unwrap()
    }

    /// Make ref updates fail by denying writes to the refs directory. Returns false when
    /// the permission change has no effect (for example when running as root).
    fn break_refs(s: &Shared, broken: bool) -> bool {
        let refs = s.engine().repo().root().join("refs");
        let mode = if broken { 0o555 } else { 0o755 };
        fs::set_permissions(&refs, fs::Permissions::from_mode(mode)).unwrap();
        !broken || fs::File::create(refs.join("probe")).is_err()
    }

    #[test]
    fn finish_checked_persists_and_is_idempotent() {
        let (_d, s) = shared(true);
        write_file(&s, "a");
        s.finish_checked().unwrap();
        assert!(has_checkpoint(&s));
        s.finish_checked().unwrap();
    }

    #[test]
    fn finish_checked_does_nothing_when_disabled() {
        let (_d, s) = shared(false);
        write_file(&s, "a");
        s.finish_checked().unwrap();
        assert!(!has_checkpoint(&s));
    }

    #[test]
    fn a_failed_final_checkpoint_is_reported_and_retried() {
        // Failure is injected by making the loose store's refs directory read-only.
        let (_d, s) = shared_in(StoreKind::Loose, true);
        write_file(&s, "a");
        if !break_refs(&s, true) {
            eprintln!("skipping: permissions are not enforced for this user");
            return;
        }
        s.finish(); // logs only, as FUSE destroy does
        assert!(s.finish_checked().is_err(), "the failure must surface");
        break_refs(&s, false);
        s.finish_checked().unwrap();
        assert!(has_checkpoint(&s), "a later attempt persists the changes");
    }
}
