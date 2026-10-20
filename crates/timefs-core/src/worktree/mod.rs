//! The mutable working tree: a writable, lazily loaded view over an immutable commit.
//!
//! Mutations touch only in-memory nodes. [`WorkTree::checkpoint`] persists the dirty paths
//! as new immutable objects and returns the new root tree hash (copy-on-write by path
//! copying). Nothing here knows about FUSE; see `docs/architecture.md`.

mod dir_ops;
mod error;
mod extents;
mod file_ops;
mod lifetime;
mod node;
mod persist;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::chunk::DEFAULT_BLOCK_SIZE;
use crate::hash::Hash;
use crate::object::{EntryKind, Tree, TreeEntry};
use crate::store::{ObjectStore, ObjectStoreExt};
use crate::timeutil::now_ns;

pub use error::{FsError, FsResult};
pub use node::{Attr, DirEntry, Ino, SetAttr, ROOT_INO};

use node::{Body, DirBody, FileBody, Meta, Node, SymlinkBody};

/// Source of timestamps (nanoseconds since the epoch); replaceable for deterministic tests.
pub type Clock = Box<dyn Fn() -> i64 + Send + Sync>;

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Block size used when persisting new data.
    pub block_size: usize,
    /// Owner reported for the root directory (it has no tree entry of its own).
    pub root_uid: u32,
    pub root_gid: u32,
    pub root_mtime_ns: i64,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            block_size: DEFAULT_BLOCK_SIZE,
            root_uid: 0,
            root_gid: 0,
            root_mtime_ns: 0,
        }
    }
}

pub struct WorkTree {
    store: Arc<dyn ObjectStore>,
    nodes: HashMap<Ino, Node>,
    next_ino: Ino,
    block_size: usize,
    /// Bytes of file data held in memory and not yet persisted.
    dirty_bytes: u64,
    clock: Clock,
}

impl WorkTree {
    /// Open a working tree over `root` (`None` starts an empty tree).
    pub fn new(store: Arc<dyn ObjectStore>, root: Option<Hash>, opts: Options) -> Self {
        let meta = Meta {
            mode: 0o755,
            uid: opts.root_uid,
            gid: opts.root_gid,
            mtime_ns: opts.root_mtime_ns,
        };
        let mut root_node = Node::new(
            ROOT_INO,
            meta,
            Body::Dir(DirBody {
                children: None,
                backing: root,
            }),
        );
        root_node.clean_hash = root;
        root_node.lookups = 1;
        let mut nodes = HashMap::new();
        nodes.insert(ROOT_INO, root_node);
        WorkTree {
            store,
            nodes,
            next_ino: ROOT_INO + 1,
            block_size: opts.block_size.max(1),
            dirty_bytes: 0,
            clock: Box::new(now_ns),
        }
    }

    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    pub fn dirty_bytes(&self) -> u64 {
        self.dirty_bytes
    }

    /// True if anything changed since the last checkpoint.
    pub fn is_dirty(&self) -> bool {
        self.nodes[&ROOT_INO].dirty
    }

    pub fn getattr(&self, ino: Ino) -> FsResult<Attr> {
        let n = self.node(ino)?;
        Ok(Attr {
            ino,
            kind: n.body.kind(),
            mode: n.meta.mode,
            uid: n.meta.uid,
            gid: n.meta.gid,
            size: n.size(),
            mtime_ns: n.meta.mtime_ns,
            nlink: if n.body.kind() == EntryKind::Dir {
                2
            } else {
                1
            },
        })
    }

    // ---- shared helpers for the operation modules ------------------------------------------

    fn now(&self) -> i64 {
        (self.clock)()
    }

    fn node(&self, ino: Ino) -> FsResult<&Node> {
        self.nodes.get(&ino).ok_or(FsError::NotFound)
    }

    fn node_mut(&mut self, ino: Ino) -> FsResult<&mut Node> {
        self.nodes.get_mut(&ino).ok_or(FsError::NotFound)
    }

    fn alloc_ino(&mut self) -> Ino {
        let ino = self.next_ino;
        self.next_ino += 1;
        ino
    }

    /// Mark `ino` and its ancestors dirty. Stops at the first already-dirty node, because
    /// a dirty node always has dirty ancestors.
    fn mark_dirty(&mut self, mut ino: Ino) {
        while let Some(node) = self.nodes.get_mut(&ino) {
            if node.dirty {
                return;
            }
            node.dirty = true;
            if ino == ROOT_INO {
                return;
            }
            ino = node.parent;
        }
    }

    /// Load a directory's entries from its tree object (once) and create child nodes.
    fn ensure_children(&mut self, ino: Ino) -> FsResult<()> {
        let backing = match &self.node(ino)?.body {
            Body::Dir(d) if d.children.is_some() => return Ok(()),
            Body::Dir(d) => d.backing,
            _ => return Err(FsError::NotDir),
        };
        let tree = match backing {
            Some(hash) => self.store.get_tree(&hash)?,
            None => Tree::new(),
        };
        let mut children = BTreeMap::new();
        for entry in tree.entries() {
            let child = self.alloc_ino();
            self.nodes.insert(child, node_from_entry(ino, entry));
            children.insert(entry.name.clone(), child);
        }
        if let Body::Dir(d) = &mut self.node_mut(ino)?.body {
            d.children = Some(children);
        }
        Ok(())
    }

    /// The loaded children of a directory. Call `ensure_children` first.
    fn children(&self, ino: Ino) -> FsResult<&BTreeMap<Vec<u8>, Ino>> {
        match &self.node(ino)?.body {
            Body::Dir(DirBody {
                children: Some(c), ..
            }) => Ok(c),
            Body::Dir(_) => unreachable!("ensure_children was not called"),
            _ => Err(FsError::NotDir),
        }
    }

    fn children_mut(&mut self, ino: Ino) -> FsResult<&mut BTreeMap<Vec<u8>, Ino>> {
        match &mut self.node_mut(ino)?.body {
            Body::Dir(DirBody {
                children: Some(c), ..
            }) => Ok(c),
            Body::Dir(_) => unreachable!("ensure_children was not called"),
            _ => Err(FsError::NotDir),
        }
    }
}

/// Build a lazily loaded node from a tree entry.
fn node_from_entry(parent: Ino, e: &TreeEntry) -> Node {
    let meta = Meta {
        mode: e.mode,
        uid: e.uid,
        gid: e.gid,
        mtime_ns: e.mtime_ns,
    };
    let body = match e.kind {
        EntryKind::Dir => Body::Dir(DirBody {
            children: None,
            backing: Some(e.hash),
        }),
        EntryKind::File => Body::File(FileBody {
            backing: Some(e.hash),
            list: None,
            stored_size: e.size,
        }),
        EntryKind::Symlink => Body::Symlink(SymlinkBody {
            backing: Some(e.hash),
            target: None,
            len: e.size,
        }),
    };
    let mut node = Node::new(parent, meta, body);
    node.clean_hash = Some(e.hash);
    node
}
