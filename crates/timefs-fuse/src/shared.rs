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

    /// Persist remaining changes at shutdown (idempotent).
    pub fn finish(&self) {
        if !self.final_checkpoint {
            return;
        }
        if let Err(e) = self.checkpoint() {
            eprintln!("timefs: final checkpoint failed: {e}");
        }
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
