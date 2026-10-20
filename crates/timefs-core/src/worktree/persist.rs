//! Checkpointing: turn the dirty part of the working tree into immutable objects.

use super::node::{Body, Ino, ROOT_INO};
use super::WorkTree;
use crate::error::{Error, Result};
use crate::hash::Hash;
use crate::object::{EntryKind, Tree, TreeEntry};
use crate::store::ObjectStoreExt;

impl WorkTree {
    /// Persist everything that changed and return the new root tree hash. Only dirty paths
    /// are rewritten; clean subtrees are referenced by their existing hashes.
    ///
    /// The caller must make the objects durable (`ObjectStore::sync`) before publishing
    /// the returned hash through a ref.
    pub fn checkpoint(&mut self) -> Result<Hash> {
        // A fresh tree that was never persisted has no hash yet, even though it is "clean".
        if let Some(root) = self.nodes.get_mut(&ROOT_INO) {
            if root.clean_hash.is_none() {
                root.dirty = true;
            }
        }
        self.persist_node(ROOT_INO)
    }

    fn persist_node(&mut self, ino: Ino) -> Result<Hash> {
        let node = self.nodes.get(&ino).ok_or_else(|| missing(ino))?;
        if !node.dirty {
            return node.clean_hash.ok_or_else(|| missing(ino));
        }
        let hash = match &node.body {
            Body::File(_) => self.persist_file(ino)?,
            Body::Symlink(_) => self.persist_symlink(ino)?,
            Body::Dir(_) => self.persist_dir(ino)?,
        };
        let node = self.nodes.get_mut(&ino).ok_or_else(|| missing(ino))?;
        node.dirty = false;
        node.clean_hash = Some(hash);
        match &mut node.body {
            Body::File(f) => f.backing = Some(hash),
            Body::Dir(d) => d.backing = Some(hash),
            Body::Symlink(s) => s.backing = Some(hash),
        }
        Ok(hash)
    }

    fn persist_file(&mut self, ino: Ino) -> Result<Hash> {
        let store = self.store.clone();
        let block = self.block_size;
        let node = self.nodes.get_mut(&ino).ok_or_else(|| missing(ino))?;
        let Body::File(f) = &mut node.body else {
            unreachable!("caller matched a file");
        };
        let list = f
            .list
            .as_mut()
            .ok_or_else(|| Error::corrupt("dirty file not loaded"))?;
        let held = list.mem_bytes();
        let obj = list.persist(store.as_ref(), block)?;
        self.dirty_bytes = self.dirty_bytes.saturating_sub(held);
        store.put_file(&obj)
    }

    fn persist_symlink(&mut self, ino: Ino) -> Result<Hash> {
        let node = self.nodes.get(&ino).ok_or_else(|| missing(ino))?;
        let Body::Symlink(s) = &node.body else {
            unreachable!("caller matched a symlink");
        };
        let target = s
            .target
            .as_ref()
            .ok_or_else(|| Error::corrupt("dirty symlink without target"))?;
        self.store.put_blob(target)
    }

    fn persist_dir(&mut self, ino: Ino) -> Result<Hash> {
        let node = self.nodes.get(&ino).ok_or_else(|| missing(ino))?;
        let Body::Dir(d) = &node.body else {
            unreachable!("caller matched a directory");
        };
        let children: Vec<(Vec<u8>, Ino)> = match (&d.children, d.backing) {
            (Some(loaded), _) => loaded
                .iter()
                .map(|(name, &child)| (name.clone(), child))
                .collect(),
            // never loaded and never modified: the existing tree is still correct
            (None, Some(existing)) => return Ok(existing),
            // a fresh, empty tree that was never persisted
            (None, None) => Vec::new(),
        };
        let mut entries = Vec::with_capacity(children.len());
        for (name, child) in children {
            let hash = self.persist_node(child)?;
            let c = self.nodes.get(&child).ok_or_else(|| missing(child))?;
            entries.push(TreeEntry {
                name,
                kind: c.body.kind(),
                mode: c.meta.mode,
                uid: c.meta.uid,
                gid: c.meta.gid,
                size: if c.body.kind() == EntryKind::Dir {
                    0
                } else {
                    c.size()
                },
                mtime_ns: c.meta.mtime_ns,
                hash,
            });
        }
        self.store.put_tree(&Tree::from_entries(entries)?)
    }
}

fn missing(ino: Ino) -> Error {
    Error::corrupt(format!("working tree inode {ino} is missing"))
}
