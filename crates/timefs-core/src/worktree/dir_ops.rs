//! Namespace operations: lookup, readdir, create, mkdir, symlink, unlink, rmdir, rename.

use std::collections::BTreeMap;

use super::extents::ExtentList;
use super::node::{Attr, Body, DirBody, DirEntry, FileBody, Ino, Meta, Node, SymlinkBody};
use super::{FsError, FsResult, WorkTree};
use crate::object::{validate_name, EntryKind, MAX_NAME_LEN};

const PERM_MASK: u32 = 0o7777;

impl WorkTree {
    pub fn lookup(&mut self, parent: Ino, name: &[u8]) -> FsResult<Attr> {
        if validate_name(name).is_err() {
            return Err(FsError::NotFound);
        }
        let ino = self.child(parent, name)?.ok_or(FsError::NotFound)?;
        self.add_lookup(ino);
        self.getattr(ino)
    }

    /// The parent directory of `ino` (the root is its own parent).
    pub fn parent(&self, ino: Ino) -> FsResult<Ino> {
        if ino == super::ROOT_INO {
            return Ok(ino);
        }
        Ok(self.node(ino)?.parent)
    }

    /// All entries of a directory in name order (without `.` and `..`).
    pub fn readdir(&mut self, ino: Ino) -> FsResult<Vec<DirEntry>> {
        self.ensure_children(ino)?;
        self.children(ino)?
            .iter()
            .map(|(name, &child)| {
                Ok(DirEntry {
                    ino: child,
                    name: name.clone(),
                    kind: self.node(child)?.body.kind(),
                })
            })
            .collect()
    }

    pub fn mkdir(
        &mut self,
        parent: Ino,
        name: &[u8],
        mode: u32,
        uid: u32,
        gid: u32,
    ) -> FsResult<Attr> {
        let body = Body::Dir(DirBody {
            children: Some(BTreeMap::new()),
            backing: None,
        });
        self.create_node(parent, name, mode, uid, gid, body)
    }

    /// Create an empty regular file.
    pub fn create(
        &mut self,
        parent: Ino,
        name: &[u8],
        mode: u32,
        uid: u32,
        gid: u32,
    ) -> FsResult<Attr> {
        let body = Body::File(FileBody {
            backing: None,
            list: Some(ExtentList::default()),
            stored_size: 0,
        });
        self.create_node(parent, name, mode, uid, gid, body)
    }

    pub fn symlink(
        &mut self,
        parent: Ino,
        name: &[u8],
        target: &[u8],
        uid: u32,
        gid: u32,
    ) -> FsResult<Attr> {
        let body = Body::Symlink(SymlinkBody {
            backing: None,
            target: Some(target.to_vec()),
            len: target.len() as u64,
        });
        self.create_node(parent, name, 0o777, uid, gid, body)
    }

    pub fn unlink(&mut self, parent: Ino, name: &[u8]) -> FsResult<()> {
        let ino = self.existing_child(parent, name)?;
        if self.node(ino)?.body.kind() == EntryKind::Dir {
            return Err(FsError::IsDir);
        }
        self.remove_entry(parent, name, ino)
    }

    pub fn rmdir(&mut self, parent: Ino, name: &[u8]) -> FsResult<()> {
        let ino = self.existing_child(parent, name)?;
        if self.node(ino)?.body.kind() != EntryKind::Dir {
            return Err(FsError::NotDir);
        }
        self.ensure_children(ino)?;
        if !self.children(ino)?.is_empty() {
            return Err(FsError::NotEmpty);
        }
        self.remove_entry(parent, name, ino)
    }

    /// Rename `parent/name` to `new_parent/new_name`, replacing a compatible target unless
    /// `no_replace` is set.
    pub fn rename(
        &mut self,
        parent: Ino,
        name: &[u8],
        new_parent: Ino,
        new_name: &[u8],
        no_replace: bool,
    ) -> FsResult<()> {
        check_new_name(new_name)?;
        let src = self.existing_child(parent, name)?;
        let dst = self.child(new_parent, new_name)?;
        if dst == Some(src) {
            return Ok(());
        }
        if no_replace && dst.is_some() {
            return Err(FsError::Exists);
        }
        let src_is_dir = self.node(src)?.body.kind() == EntryKind::Dir;
        if src_is_dir && self.is_same_or_ancestor(src, new_parent)? {
            return Err(FsError::InvalidArgument(
                "cannot move a directory into itself",
            ));
        }
        if let Some(dst) = dst {
            self.check_replaceable(src_is_dir, dst)?;
            self.children_mut(new_parent)?.remove(new_name);
            self.mark_unlinked(dst)?;
        }
        self.children_mut(parent)?.remove(name);
        self.children_mut(new_parent)?
            .insert(new_name.to_vec(), src);
        self.node_mut(src)?.parent = new_parent;
        self.touch_dir(parent)?;
        self.touch_dir(new_parent)
    }

    // ---- helpers -------------------------------------------------------------------------

    /// Child inode by name, loading the directory if needed.
    fn child(&mut self, parent: Ino, name: &[u8]) -> FsResult<Option<Ino>> {
        self.ensure_children(parent)?;
        Ok(self.children(parent)?.get(name).copied())
    }

    fn existing_child(&mut self, parent: Ino, name: &[u8]) -> FsResult<Ino> {
        if validate_name(name).is_err() {
            return Err(FsError::NotFound);
        }
        self.child(parent, name)?.ok_or(FsError::NotFound)
    }

    fn create_node(
        &mut self,
        parent: Ino,
        name: &[u8],
        mode: u32,
        uid: u32,
        gid: u32,
        body: Body,
    ) -> FsResult<Attr> {
        check_new_name(name)?;
        if self.child(parent, name)?.is_some() {
            return Err(FsError::Exists);
        }
        let meta = Meta {
            mode: mode & PERM_MASK,
            uid,
            gid,
            mtime_ns: self.now(),
        };
        let ino = self.alloc_ino();
        let mut node = Node::new(parent, meta, body);
        node.dirty = true;
        node.lookups = 1;
        self.nodes.insert(ino, node);
        self.children_mut(parent)?.insert(name.to_vec(), ino);
        self.touch_dir(parent)?;
        self.getattr(ino)
    }

    fn remove_entry(&mut self, parent: Ino, name: &[u8], ino: Ino) -> FsResult<()> {
        self.children_mut(parent)?.remove(name);
        self.touch_dir(parent)?;
        self.mark_unlinked(ino)
    }

    /// A directory's contents changed: bump its mtime and mark it (and ancestors) dirty.
    fn touch_dir(&mut self, dir: Ino) -> FsResult<()> {
        let now = self.now();
        self.node_mut(dir)?.meta.mtime_ns = now;
        self.mark_dirty(dir);
        Ok(())
    }

    /// Whether `maybe_descendant` is `ancestor` or lies below it.
    fn is_same_or_ancestor(&self, ancestor: Ino, maybe_descendant: Ino) -> FsResult<bool> {
        let mut cur = maybe_descendant;
        loop {
            if cur == ancestor {
                return Ok(true);
            }
            if cur == super::ROOT_INO {
                return Ok(false);
            }
            cur = self.node(cur)?.parent;
        }
    }

    /// POSIX rules for renaming over an existing target.
    fn check_replaceable(&mut self, src_is_dir: bool, dst: Ino) -> FsResult<()> {
        let dst_is_dir = self.node(dst)?.body.kind() == EntryKind::Dir;
        match (src_is_dir, dst_is_dir) {
            (true, false) => Err(FsError::NotDir),
            (false, true) => Err(FsError::IsDir),
            (true, true) => {
                self.ensure_children(dst)?;
                if self.children(dst)?.is_empty() {
                    Ok(())
                } else {
                    Err(FsError::NotEmpty)
                }
            }
            (false, false) => Ok(()),
        }
    }
}

fn check_new_name(name: &[u8]) -> FsResult<()> {
    if name.len() > MAX_NAME_LEN {
        return Err(FsError::NameTooLong);
    }
    validate_name(name).map_err(|_| FsError::InvalidArgument("invalid name"))
}
