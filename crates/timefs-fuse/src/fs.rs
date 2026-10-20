//! The `fuser::Filesystem` implementation: a thin translation layer.
//!
//! Every handler locks the working tree, calls one `WorkTree` method and converts the
//! result. No versioning logic lives here.

use std::ffi::OsStr;
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use fuser::{
    Errno, FileHandle, Filesystem, FopenFlags, Generation, INodeNo, KernelConfig, OpenFlags,
    RenameFlags, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry,
    ReplyOpen, ReplyStatfs, ReplyWrite, Request, TimeOrNow,
};
use timefs_core::timeutil::system_time_to_ns;
use timefs_core::worktree::{Attr, FsResult, SetAttr};

use crate::attr::{file_type, to_file_attr};
use crate::errno::{errno_of, store_errno};
use crate::shared::{DirItem, Shared};
use crate::statfs::{host_stats, NAME_MAX};

const MAX_WRITE: u32 = 1 << 20;
const S_IFMT: u32 = 0o170000;
/// All writes go through this mount, so the kernel page cache can be kept across opens.
const OPEN_FLAGS: FopenFlags = FopenFlags::FOPEN_KEEP_CACHE;
const S_IFREG: u32 = 0o100000;

pub struct TimeFs {
    shared: Arc<Shared>,
    ttl: Duration,
}

impl TimeFs {
    pub fn new(shared: Arc<Shared>, ttl: Duration) -> Self {
        TimeFs { shared, ttl }
    }

    fn reply_entry(&self, result: FsResult<Attr>, reply: ReplyEntry) {
        match result {
            Ok(a) => reply.entry(&self.ttl, &to_file_attr(&a), Generation(0)),
            Err(e) => reply.error(errno_of(&e)),
        }
    }
}

fn reply_unit(result: FsResult<()>, reply: ReplyEmpty) {
    match result {
        Ok(()) => reply.ok(),
        Err(e) => reply.error(errno_of(&e)),
    }
}

fn apply_umask(mode: u32, umask: u32) -> u32 {
    mode & !umask & 0o7777
}

fn resolve_time(t: TimeOrNow) -> i64 {
    match t {
        TimeOrNow::SpecificTime(t) => system_time_to_ns(t),
        TimeOrNow::Now => system_time_to_ns(SystemTime::now()),
    }
}

impl Filesystem for TimeFs {
    fn init(&mut self, _req: &Request, config: &mut KernelConfig) -> io::Result<()> {
        // Larger transfers cut the number of kernel round trips; ignore a refusal.
        let _ = config.set_max_write(MAX_WRITE);
        Ok(())
    }

    fn destroy(&mut self) {
        self.shared.finish();
    }

    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        let result = self.shared.lock().lookup(parent.0, name.as_bytes());
        self.reply_entry(result, reply);
    }

    fn forget(&self, _req: &Request, ino: INodeNo, nlookup: u64) {
        self.shared.lock().forget(ino.0, nlookup);
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        match self.shared.lock().getattr(ino.0) {
            Ok(a) => reply.attr(&self.ttl, &to_file_attr(&a)),
            Err(e) => reply.error(errno_of(&e)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn setattr(
        &self,
        _req: &Request,
        ino: INodeNo,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<FileHandle>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<fuser::BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        let change = SetAttr {
            mode,
            uid,
            gid,
            size,
            mtime_ns: mtime.map(resolve_time),
        };
        match self.shared.lock().setattr(ino.0, change) {
            Ok(a) => reply.attr(&self.ttl, &to_file_attr(&a)),
            Err(e) => reply.error(errno_of(&e)),
        }
    }

    fn readlink(&self, _req: &Request, ino: INodeNo, reply: ReplyData) {
        match self.shared.lock().readlink(ino.0) {
            Ok(target) => reply.data(&target),
            Err(e) => reply.error(errno_of(&e)),
        }
    }

    fn mknod(
        &self,
        req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        umask: u32,
        _rdev: u32,
        reply: ReplyEntry,
    ) {
        if mode & S_IFMT != S_IFREG {
            return reply.error(Errno::EPERM); // only regular files, no device nodes or fifos
        }
        let mode = apply_umask(mode, umask);
        let result =
            self.shared
                .lock()
                .create(parent.0, name.as_bytes(), mode, req.uid(), req.gid());
        self.reply_entry(result, reply);
    }

    fn mkdir(
        &self,
        req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        umask: u32,
        reply: ReplyEntry,
    ) {
        let mode = apply_umask(mode, umask);
        let result =
            self.shared
                .lock()
                .mkdir(parent.0, name.as_bytes(), mode, req.uid(), req.gid());
        self.reply_entry(result, reply);
    }

    fn unlink(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        reply_unit(self.shared.lock().unlink(parent.0, name.as_bytes()), reply);
    }

    fn rmdir(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        reply_unit(self.shared.lock().rmdir(parent.0, name.as_bytes()), reply);
    }

    fn symlink(
        &self,
        req: &Request,
        parent: INodeNo,
        link_name: &OsStr,
        target: &Path,
        reply: ReplyEntry,
    ) {
        let result = self.shared.lock().symlink(
            parent.0,
            link_name.as_bytes(),
            target.as_os_str().as_bytes(),
            req.uid(),
            req.gid(),
        );
        self.reply_entry(result, reply);
    }

    fn rename(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        newparent: INodeNo,
        newname: &OsStr,
        flags: RenameFlags,
        reply: ReplyEmpty,
    ) {
        if flags.intersects(RenameFlags::RENAME_EXCHANGE | RenameFlags::RENAME_WHITEOUT) {
            return reply.error(Errno::EINVAL); // exchange and whiteout are not supported
        }
        let no_replace = flags.contains(RenameFlags::RENAME_NOREPLACE);
        let result = self.shared.lock().rename(
            parent.0,
            name.as_bytes(),
            newparent.0,
            newname.as_bytes(),
            no_replace,
        );
        reply_unit(result, reply);
    }

    fn open(&self, _req: &Request, ino: INodeNo, _flags: OpenFlags, reply: ReplyOpen) {
        let mut wt = self.shared.lock();
        match wt.getattr(ino.0) {
            Ok(a) if a.kind == timefs_core::object::EntryKind::Dir => reply.error(Errno::EISDIR),
            Ok(_) => match wt.open(ino.0) {
                Ok(()) => reply.opened(FileHandle(0), OPEN_FLAGS),
                Err(e) => reply.error(errno_of(&e)),
            },
            Err(e) => reply.error(errno_of(&e)),
        }
    }

    fn create(
        &self,
        req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        let mode = apply_umask(mode, umask);
        let mut wt = self.shared.lock();
        let created = wt
            .create(parent.0, name.as_bytes(), mode, req.uid(), req.gid())
            .and_then(|a| wt.open(a.ino).map(|()| a));
        match created {
            Ok(a) => reply.created(
                &self.ttl,
                &to_file_attr(&a),
                Generation(0),
                FileHandle(0),
                OPEN_FLAGS,
            ),
            Err(e) => reply.error(errno_of(&e)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn read(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<fuser::LockOwner>,
        reply: ReplyData,
    ) {
        match self.shared.lock().read(ino.0, offset, size as usize) {
            Ok(data) => reply.data(&data),
            Err(e) => reply.error(errno_of(&e)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn write(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        data: &[u8],
        _write_flags: fuser::WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<fuser::LockOwner>,
        reply: ReplyWrite,
    ) {
        let mut wt = self.shared.lock();
        match wt.write(ino.0, offset, data) {
            Ok(n) => {
                self.shared.flush_if_needed(&mut wt);
                reply.written(n as u32);
            }
            Err(e) => reply.error(errno_of(&e)),
        }
    }

    fn flush(
        &self,
        _req: &Request,
        _ino: INodeNo,
        _fh: FileHandle,
        _lock_owner: fuser::LockOwner,
        reply: ReplyEmpty,
    ) {
        reply.ok();
    }

    fn release(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<fuser::LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        self.shared.lock().release(ino.0);
        reply.ok();
    }

    fn fsync(
        &self,
        _req: &Request,
        _ino: INodeNo,
        _fh: FileHandle,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        self.reply_checkpoint(reply);
    }

    fn fsyncdir(
        &self,
        _req: &Request,
        _ino: INodeNo,
        _fh: FileHandle,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        self.reply_checkpoint(reply);
    }

    fn opendir(&self, _req: &Request, ino: INodeNo, _flags: OpenFlags, reply: ReplyOpen) {
        match self.list_dir(ino.0) {
            Ok(items) => {
                let fh = self.shared.open_dir_handle(items);
                reply.opened(FileHandle(fh), FopenFlags::empty());
            }
            Err(e) => reply.error(errno_of(&e)),
        }
    }

    fn readdir(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        let Some(items) = self.shared.dir_handle(fh.0) else {
            return reply.error(Errno::EBADF);
        };
        for (i, item) in items.iter().enumerate().skip(offset as usize) {
            if reply.add(INodeNo(item.ino), (i + 1) as u64, item.kind, &item.name) {
                break;
            }
        }
        reply.ok();
    }

    fn releasedir(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        reply: ReplyEmpty,
    ) {
        self.shared.close_dir_handle(fh.0);
        reply.ok();
    }

    fn statfs(&self, _req: &Request, _ino: INodeNo, reply: ReplyStatfs) {
        let s = host_stats(self.shared.stats_path());
        reply.statfs(
            s.blocks, s.bfree, s.bavail, s.files, s.ffree, s.bsize, NAME_MAX, s.frsize,
        );
    }
}

impl TimeFs {
    /// Directory listing including `.` and `..`, captured once at `opendir` so that offsets
    /// stay stable while the directory changes.
    fn list_dir(&self, ino: u64) -> FsResult<Vec<DirItem>> {
        let mut wt = self.shared.lock();
        let parent = wt.parent(ino)?;
        let entries = wt.readdir(ino)?;
        let mut items = vec![
            DirItem {
                ino,
                kind: fuser::FileType::Directory,
                name: ".".into(),
            },
            DirItem {
                ino: parent,
                kind: fuser::FileType::Directory,
                name: "..".into(),
            },
        ];
        items.extend(entries.into_iter().map(|e| DirItem {
            ino: e.ino,
            kind: file_type(e.kind),
            name: std::ffi::OsString::from_vec(e.name),
        }));
        Ok(items)
    }

    fn reply_checkpoint(&self, reply: ReplyEmpty) {
        match self.shared.checkpoint() {
            Ok(_) => reply.ok(),
            Err(e) => {
                eprintln!("timefs: fsync checkpoint failed: {e}");
                reply.error(store_errno(&e));
            }
        }
    }
}
