//! Kernel reference counting and freeing of unlinked inodes.
//!
//! A file removed from the namespace must stay readable while the kernel still knows its
//! inode (`lookups`) or an open handle exists (`opens`); it is freed afterwards.

use super::node::{Body, Ino, ROOT_INO};
use super::{FsError, FsResult, WorkTree};

impl WorkTree {
    pub(super) fn add_lookup(&mut self, ino: Ino) {
        if let Some(n) = self.nodes.get_mut(&ino) {
            n.lookups += 1;
        }
    }

    /// The kernel dropped `n` references to `ino`.
    pub fn forget(&mut self, ino: Ino, n: u64) {
        if let Some(node) = self.nodes.get_mut(&ino) {
            node.lookups = node.lookups.saturating_sub(n);
        }
        self.maybe_free(ino);
    }

    pub fn open(&mut self, ino: Ino) -> FsResult<()> {
        self.node_mut(ino)?.opens += 1;
        Ok(())
    }

    pub fn release(&mut self, ino: Ino) {
        if let Some(node) = self.nodes.get_mut(&ino) {
            node.opens = node.opens.saturating_sub(1);
        }
        self.maybe_free(ino);
    }

    /// Detach a node from the namespace; it is freed once nothing references it.
    pub(super) fn mark_unlinked(&mut self, ino: Ino) -> FsResult<()> {
        self.node_mut(ino)?.unlinked = true;
        self.maybe_free(ino);
        Ok(())
    }

    fn maybe_free(&mut self, ino: Ino) {
        if ino == ROOT_INO {
            return;
        }
        let free = self
            .nodes
            .get(&ino)
            .is_some_and(|n| n.unlinked && n.lookups == 0 && n.opens == 0);
        if !free {
            return;
        }
        if let Some(node) = self.nodes.remove(&ino) {
            if let Body::File(f) = &node.body {
                let held = f.list.as_ref().map_or(0, |l| l.mem_bytes());
                self.dirty_bytes = self.dirty_bytes.saturating_sub(held);
            }
        }
    }

    /// Number of inodes currently held in memory (for tests and diagnostics).
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

impl From<FsError> for std::io::Error {
    fn from(e: FsError) -> Self {
        std::io::Error::other(e.to_string())
    }
}
