//! Data and attribute operations on files: read, write, truncate, setattr, readlink.

use std::sync::Arc;

use super::extents::ExtentList;
use super::node::{Attr, Body, Ino, SetAttr};
use super::{FsError, FsResult, WorkTree};
use crate::error::Result;
use crate::object::FileObj;
use crate::store::{ObjectStore, ObjectStoreExt};

const PERM_MASK: u32 = 0o7777;

/// Largest supported file (1 PiB). Holes cost one extent each and long extent lists are
/// paged, so the bound only keeps offset arithmetic far from overflow.
pub const MAX_FILE_SIZE: u64 = 1 << 50;

fn check_size(end: u64) -> FsResult<()> {
    if end > MAX_FILE_SIZE {
        return Err(FsError::FileTooBig);
    }
    Ok(())
}

impl WorkTree {
    pub fn read(&mut self, ino: Ino, off: u64, size: usize) -> FsResult<Vec<u8>> {
        self.ensure_file_list(ino)?;
        let Body::File(f) = &self.node(ino)?.body else {
            unreachable!("ensure_file_list guarantees a file");
        };
        let list = f.list.as_ref().expect("list loaded");
        Ok(list.read(self.store.as_ref(), off, size)?)
    }

    pub fn write(&mut self, ino: Ino, off: u64, data: &[u8]) -> FsResult<usize> {
        check_size(
            off.checked_add(data.len() as u64)
                .ok_or(FsError::FileTooBig)?,
        )?;
        let block = self.block_size;
        self.modify_file(ino, |list, store| list.write(store, off, data, block))?;
        Ok(data.len())
    }

    pub fn readlink(&mut self, ino: Ino) -> FsResult<Vec<u8>> {
        let (backing, cached) = match &self.node(ino)?.body {
            Body::Symlink(s) => (s.backing, s.target.clone()),
            Body::Dir(_) => return Err(FsError::IsDir),
            Body::File(_) => return Err(FsError::InvalidArgument("not a symlink")),
        };
        if let Some(target) = cached {
            return Ok(target);
        }
        let hash = backing.ok_or(FsError::NotFound)?;
        let target = self.store.get_blob(&hash)?;
        if let Body::Symlink(s) = &mut self.node_mut(ino)?.body {
            s.target = Some(target.clone());
        }
        Ok(target)
    }

    pub fn setattr(&mut self, ino: Ino, change: SetAttr) -> FsResult<Attr> {
        if let Some(size) = change.size {
            check_size(size)?;
            self.modify_file(ino, |list, store| list.truncate(store, size))?;
        }
        let node = self.node_mut(ino)?;
        if let Some(mode) = change.mode {
            node.meta.mode = mode & PERM_MASK;
        }
        if let Some(uid) = change.uid {
            node.meta.uid = uid;
        }
        if let Some(gid) = change.gid {
            node.meta.gid = gid;
        }
        if let Some(mtime) = change.mtime_ns {
            node.meta.mtime_ns = mtime;
        }
        self.mark_dirty(ino);
        self.getattr(ino)
    }

    // ---- helpers -------------------------------------------------------------------------

    /// Load a file's extent list from its backing object (once).
    fn ensure_file_list(&mut self, ino: Ino) -> FsResult<()> {
        let backing = match &self.node(ino)?.body {
            Body::File(f) if f.list.is_some() => return Ok(()),
            Body::File(f) => f.backing,
            Body::Dir(_) => return Err(FsError::IsDir),
            Body::Symlink(_) => return Err(FsError::InvalidArgument("not a regular file")),
        };
        let obj = match backing {
            Some(hash) => self.store.get_file(&hash)?,
            None => FileObj::default(),
        };
        if let Body::File(f) = &mut self.node_mut(ino)?.body {
            f.list = Some(ExtentList::from_file(&obj));
        }
        Ok(())
    }

    /// Apply a mutation to a file's data, then account memory, set mtime and mark dirty.
    fn modify_file(
        &mut self,
        ino: Ino,
        change: impl FnOnce(&mut ExtentList, &dyn ObjectStore) -> Result<()>,
    ) -> FsResult<()> {
        self.ensure_file_list(ino)?;
        let store = Arc::clone(&self.store);
        let now = self.now();
        let node = self.node_mut(ino)?;
        let Body::File(f) = &mut node.body else {
            unreachable!("ensure_file_list guarantees a file");
        };
        let list = f.list.as_mut().expect("list loaded");
        let before = list.mem_bytes();
        let result = change(list, store.as_ref());
        let after = list.mem_bytes();
        node.meta.mtime_ns = now;
        self.dirty_bytes = (self.dirty_bytes + after).saturating_sub(before);
        self.mark_dirty(ino);
        Ok(result?)
    }
}
