//! Inode data structures of the working tree.

use std::collections::BTreeMap;

use super::extents::ExtentList;
use crate::hash::Hash;
use crate::object::EntryKind;

pub type Ino = u64;
pub const ROOT_INO: Ino = 1;

/// Persistent per-node metadata (what a tree entry records).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Meta {
    /// Permission bits only (`mode & 0o7777`).
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub mtime_ns: i64,
}

/// Directory contents. `children` is `None` until first needed (lazy loading).
#[derive(Debug, Default)]
pub struct DirBody {
    pub children: Option<BTreeMap<Vec<u8>, Ino>>,
    /// Tree object to load `children` from; `None` for a directory created in memory.
    pub backing: Option<Hash>,
}

/// File contents. `list` is `None` until the file is first read or written.
#[derive(Debug, Default)]
pub struct FileBody {
    pub backing: Option<Hash>,
    pub list: Option<ExtentList>,
    /// Size recorded in the parent entry while `list` is not loaded.
    pub stored_size: u64,
}

impl FileBody {
    pub fn size(&self) -> u64 {
        self.list
            .as_ref()
            .map_or(self.stored_size, ExtentList::size)
    }
}

/// Symlink target. `target` is `None` until first read (it lives in a blob).
#[derive(Debug, Default)]
pub struct SymlinkBody {
    pub backing: Option<Hash>,
    pub target: Option<Vec<u8>>,
    pub len: u64,
}

#[derive(Debug)]
pub enum Body {
    Dir(DirBody),
    File(FileBody),
    Symlink(SymlinkBody),
}

impl Body {
    pub fn kind(&self) -> EntryKind {
        match self {
            Body::Dir(_) => EntryKind::Dir,
            Body::File(_) => EntryKind::File,
            Body::Symlink(_) => EntryKind::Symlink,
        }
    }
}

#[derive(Debug)]
pub struct Node {
    pub parent: Ino,
    pub meta: Meta,
    pub body: Body,
    /// The node, or something below it, differs from `clean_hash`.
    /// Invariant: a dirty node's ancestors are dirty too.
    pub dirty: bool,
    /// Hash of the persisted object matching the current state (valid while `!dirty`).
    pub clean_hash: Option<Hash>,
    /// Kernel references (`lookup` replies minus `forget`s).
    pub lookups: u64,
    /// Open file handles.
    pub opens: u64,
    /// Removed from the namespace but still referenced by the kernel or an open handle.
    pub unlinked: bool,
}

impl Node {
    pub fn new(parent: Ino, meta: Meta, body: Body) -> Self {
        Node {
            parent,
            meta,
            body,
            dirty: false,
            clean_hash: None,
            lookups: 0,
            opens: 0,
            unlinked: false,
        }
    }

    pub fn size(&self) -> u64 {
        match &self.body {
            Body::Dir(_) => 0,
            Body::File(f) => f.size(),
            Body::Symlink(s) => s.len,
        }
    }
}

/// What `getattr`-like operations return.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attr {
    pub ino: Ino,
    pub kind: EntryKind,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub mtime_ns: i64,
    pub nlink: u32,
}

/// Fields to change in `setattr`; `None` leaves a field untouched.
#[derive(Clone, Copy, Debug, Default)]
pub struct SetAttr {
    pub mode: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub size: Option<u64>,
    pub mtime_ns: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub ino: Ino,
    pub name: Vec<u8>,
    pub kind: EntryKind,
}
