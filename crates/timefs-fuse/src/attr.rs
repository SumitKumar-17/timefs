//! Conversion of working tree attributes to the FUSE representation.

use std::time::SystemTime;

use fuser::{FileAttr, FileType, INodeNo};
use timefs_core::object::EntryKind;
use timefs_core::timeutil::ns_to_system_time;
use timefs_core::worktree::Attr;

const BLKSIZE: u32 = 4096;

pub fn file_type(kind: EntryKind) -> FileType {
    match kind {
        EntryKind::File => FileType::RegularFile,
        EntryKind::Dir => FileType::Directory,
        EntryKind::Symlink => FileType::Symlink,
    }
}

pub fn to_file_attr(a: &Attr) -> FileAttr {
    // Only mtime is persisted; atime, ctime and crtime follow it.
    let mtime: SystemTime = ns_to_system_time(a.mtime_ns);
    FileAttr {
        ino: INodeNo(a.ino),
        size: a.size,
        blocks: a.size.div_ceil(512),
        atime: mtime,
        mtime,
        ctime: mtime,
        crtime: mtime,
        kind: file_type(a.kind),
        perm: (a.mode & 0o7777) as u16,
        nlink: a.nlink,
        uid: a.uid,
        gid: a.gid,
        rdev: 0,
        blksize: BLKSIZE,
        flags: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_fields() {
        let a = Attr {
            ino: 7,
            kind: EntryKind::File,
            mode: 0o640,
            uid: 1,
            gid: 2,
            size: 1025,
            mtime_ns: 5_000_000_000,
            nlink: 1,
        };
        let f = to_file_attr(&a);
        assert_eq!(
            (f.ino.0, f.perm, f.blocks, f.kind),
            (7, 0o640, 3, FileType::RegularFile)
        );
        assert_eq!(f.mtime, ns_to_system_time(5_000_000_000));
    }
}
