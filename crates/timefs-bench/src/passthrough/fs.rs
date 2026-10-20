//! FUSE handlers: each one forwards to the same operation on the host directory.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use fuser::{
    Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags, Generation, INodeNo, OpenFlags,
    RenameFlags, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry,
    ReplyOpen, ReplyStatfs, ReplyWrite, Request, TimeOrNow,
};

use super::inodes::Inodes;

const TTL: Duration = Duration::from_secs(1);

/// A snapshot of one directory taken at `opendir`: inode, type and name of each entry.
type DirListing = Vec<(u64, FileType, OsString)>;

pub struct Passthrough {
    inodes: Mutex<Inodes>,
    files: Mutex<HashMap<u64, Arc<File>>>,
    dirs: Mutex<HashMap<u64, Arc<DirListing>>>,
    next_fh: AtomicU64,
    root: PathBuf,
}

impl Passthrough {
    pub fn new(root: PathBuf) -> Self {
        Passthrough {
            inodes: Mutex::new(Inodes::new(root.clone())),
            files: Mutex::new(HashMap::new()),
            dirs: Mutex::new(HashMap::new()),
            next_fh: AtomicU64::new(1),
            root,
        }
    }

    fn path(&self, ino: INodeNo) -> Result<PathBuf, Errno> {
        self.inodes
            .lock()
            .unwrap()
            .path(ino.0)
            .map(Path::to_path_buf)
            .ok_or(Errno::ENOENT)
    }

    fn child_path(&self, parent: INodeNo, name: &OsStr) -> Result<PathBuf, Errno> {
        Ok(self.path(parent)?.join(name))
    }

    fn entry(&self, path: &Path) -> io::Result<FileAttr> {
        let meta = fs::symlink_metadata(path)?;
        let ino = self.inodes.lock().unwrap().ino_for(path.to_path_buf());
        Ok(attr_of(ino, &meta))
    }

    fn new_fh(&self) -> u64 {
        self.next_fh.fetch_add(1, Ordering::Relaxed)
    }

    fn keep_file(&self, file: File) -> u64 {
        let fh = self.new_fh();
        self.files.lock().unwrap().insert(fh, Arc::new(file));
        fh
    }

    fn file(&self, fh: FileHandle) -> Result<Arc<File>, Errno> {
        self.files
            .lock()
            .unwrap()
            .get(&fh.0)
            .cloned()
            .ok_or(Errno::EBADF)
    }
}

fn attr_of(ino: u64, m: &fs::Metadata) -> FileAttr {
    let kind = match m.file_type() {
        t if t.is_dir() => FileType::Directory,
        t if t.is_symlink() => FileType::Symlink,
        _ => FileType::RegularFile,
    };
    let time = |t: io::Result<SystemTime>| t.unwrap_or(SystemTime::UNIX_EPOCH);
    FileAttr {
        ino: INodeNo(ino),
        size: m.len(),
        blocks: m.len().div_ceil(512),
        atime: time(m.accessed()),
        mtime: time(m.modified()),
        ctime: time(m.modified()),
        crtime: time(m.modified()),
        kind,
        perm: (m.mode() & 0o7777) as u16,
        nlink: m.nlink() as u32,
        uid: m.uid(),
        gid: m.gid(),
        rdev: 0,
        blksize: 4096,
        flags: 0,
    }
}

fn errno(e: &io::Error) -> Errno {
    e.raw_os_error().map_or(Errno::EIO, Errno::from_i32)
}

fn reply_entry(result: io::Result<FileAttr>, reply: ReplyEntry) {
    match result {
        Ok(a) => reply.entry(&TTL, &a, Generation(0)),
        Err(e) => reply.error(errno(&e)),
    }
}

fn reply_unit(result: io::Result<()>, reply: ReplyEmpty) {
    match result {
        Ok(()) => reply.ok(),
        Err(e) => reply.error(errno(&e)),
    }
}

impl Filesystem for Passthrough {
    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        match self.child_path(parent, name) {
            Ok(p) => reply_entry(self.entry(&p), reply),
            Err(e) => reply.error(e),
        }
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        match self.path(ino).map(|p| self.entry(&p)) {
            Ok(Ok(a)) => reply.attr(&TTL, &a),
            Ok(Err(e)) => reply.error(errno(&e)),
            Err(e) => reply.error(e),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn setattr(
        &self,
        _req: &Request,
        ino: INodeNo,
        mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
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
        let path = match self.path(ino) {
            Ok(p) => p,
            Err(e) => return reply.error(e),
        };
        let apply = || -> io::Result<FileAttr> {
            if let Some(m) = mode {
                fs::set_permissions(&path, fs::Permissions::from_mode(m & 0o7777))?;
            }
            if let Some(s) = size {
                OpenOptions::new().write(true).open(&path)?.set_len(s)?;
            }
            if let Some(t) = mtime {
                let when = match t {
                    TimeOrNow::SpecificTime(t) => t,
                    TimeOrNow::Now => SystemTime::now(),
                };
                File::open(&path)?.set_modified(when)?;
            }
            self.entry(&path)
        };
        match apply() {
            Ok(a) => reply.attr(&TTL, &a),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn readlink(&self, _req: &Request, ino: INodeNo, reply: ReplyData) {
        match self.path(ino).map(fs::read_link) {
            Ok(Ok(t)) => reply.data(t.as_os_str().as_bytes()),
            Ok(Err(e)) => reply.error(errno(&e)),
            Err(e) => reply.error(e),
        }
    }

    fn mkdir(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        umask: u32,
        reply: ReplyEntry,
    ) {
        let result = self
            .child_path(parent, name)
            .map_err(|e| io::Error::from_raw_os_error(e.code()))
            .and_then(|p| {
                fs::create_dir(&p)?;
                fs::set_permissions(&p, fs::Permissions::from_mode(mode & !umask & 0o7777))?;
                self.entry(&p)
            });
        reply_entry(result, reply);
    }

    fn unlink(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let result = self
            .child_path(parent, name)
            .map_err(|e| io::Error::from_raw_os_error(e.code()))
            .and_then(|p| {
                fs::remove_file(&p)?;
                self.inodes.lock().unwrap().forget_path(&p);
                Ok(())
            });
        reply_unit(result, reply);
    }

    fn rmdir(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let result = self
            .child_path(parent, name)
            .map_err(|e| io::Error::from_raw_os_error(e.code()))
            .and_then(|p| {
                fs::remove_dir(&p)?;
                self.inodes.lock().unwrap().forget_path(&p);
                Ok(())
            });
        reply_unit(result, reply);
    }

    fn symlink(
        &self,
        _req: &Request,
        parent: INodeNo,
        link_name: &OsStr,
        target: &Path,
        reply: ReplyEntry,
    ) {
        let result = self
            .child_path(parent, link_name)
            .map_err(|e| io::Error::from_raw_os_error(e.code()))
            .and_then(|p| {
                std::os::unix::fs::symlink(target, &p)?;
                self.entry(&p)
            });
        reply_entry(result, reply);
    }

    fn rename(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        newparent: INodeNo,
        newname: &OsStr,
        _flags: RenameFlags,
        reply: ReplyEmpty,
    ) {
        let paths = self
            .child_path(parent, name)
            .and_then(|a| Ok((a, self.child_path(newparent, newname)?)));
        let result = paths
            .map_err(|e| io::Error::from_raw_os_error(e.code()))
            .and_then(|(from, to)| {
                fs::rename(&from, &to)?;
                self.inodes.lock().unwrap().rename(&from, &to);
                Ok(())
            });
        reply_unit(result, reply);
    }

    fn open(&self, _req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        let result = self
            .path(ino)
            .map_err(|e| io::Error::from_raw_os_error(e.code()))
            .and_then(|p| {
                let acc = flags.0 & libc::O_ACCMODE;
                OpenOptions::new()
                    .read(acc == libc::O_RDONLY || acc == libc::O_RDWR)
                    .write(acc == libc::O_WRONLY || acc == libc::O_RDWR)
                    .custom_flags(flags.0 & !libc::O_ACCMODE & !libc::O_TRUNC)
                    .open(p)
            });
        match result {
            Ok(f) => reply.opened(FileHandle(self.keep_file(f)), FopenFlags::empty()),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn create(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        let result = self
            .child_path(parent, name)
            .map_err(|e| io::Error::from_raw_os_error(e.code()))
            .and_then(|p| {
                let f = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .mode(mode & !umask & 0o7777)
                    .open(&p)?;
                Ok((f, self.entry(&p)?))
            });
        match result {
            Ok((f, a)) => reply.created(
                &TTL,
                &a,
                Generation(0),
                FileHandle(self.keep_file(f)),
                FopenFlags::empty(),
            ),
            Err(e) => reply.error(errno(&e)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn read(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<fuser::LockOwner>,
        reply: ReplyData,
    ) {
        match self.file(fh) {
            Ok(f) => {
                let mut buf = vec![0u8; size as usize];
                match f.read_at(&mut buf, offset) {
                    Ok(n) => reply.data(&buf[..n]),
                    Err(e) => reply.error(errno(&e)),
                }
            }
            Err(e) => reply.error(e),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn write(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        data: &[u8],
        _write_flags: fuser::WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<fuser::LockOwner>,
        reply: ReplyWrite,
    ) {
        match self.file(fh).map(|f| f.write_all_at(data, offset)) {
            Ok(Ok(())) => reply.written(data.len() as u32),
            Ok(Err(e)) => reply.error(errno(&e)),
            Err(e) => reply.error(e),
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
        _ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<fuser::LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        self.files.lock().unwrap().remove(&fh.0);
        reply.ok();
    }

    fn fsync(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        datasync: bool,
        reply: ReplyEmpty,
    ) {
        match self.file(fh) {
            Ok(f) => reply_unit(
                if datasync {
                    f.sync_data()
                } else {
                    f.sync_all()
                },
                reply,
            ),
            Err(e) => reply.error(e),
        }
    }

    fn opendir(&self, _req: &Request, ino: INodeNo, _flags: OpenFlags, reply: ReplyOpen) {
        let result = self
            .path(ino)
            .map_err(|e| io::Error::from_raw_os_error(e.code()))
            .and_then(|dir| {
                let parent = dir.parent().unwrap_or(&self.root).to_path_buf();
                let mut items = vec![
                    (ino.0, FileType::Directory, OsString::from(".")),
                    (
                        self.inode_of(&parent),
                        FileType::Directory,
                        OsString::from(".."),
                    ),
                ];
                for entry in fs::read_dir(&dir)? {
                    let entry = entry?;
                    let kind = match entry.file_type()? {
                        t if t.is_dir() => FileType::Directory,
                        t if t.is_symlink() => FileType::Symlink,
                        _ => FileType::RegularFile,
                    };
                    items.push((self.inode_of(&entry.path()), kind, entry.file_name()));
                }
                Ok(items)
            });
        match result {
            Ok(items) => {
                let fh = self.new_fh();
                self.dirs.lock().unwrap().insert(fh, Arc::new(items));
                reply.opened(FileHandle(fh), FopenFlags::empty());
            }
            Err(e) => reply.error(errno(&e)),
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
        let Some(items) = self.dirs.lock().unwrap().get(&fh.0).cloned() else {
            return reply.error(Errno::EBADF);
        };
        for (i, (ino, kind, name)) in items.iter().enumerate().skip(offset as usize) {
            if reply.add(INodeNo(*ino), (i + 1) as u64, *kind, name) {
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
        self.dirs.lock().unwrap().remove(&fh.0);
        reply.ok();
    }

    fn statfs(&self, _req: &Request, _ino: INodeNo, reply: ReplyStatfs) {
        reply.statfs(1 << 30, 1 << 30, 1 << 30, 1 << 30, 1 << 30, 4096, 255, 4096);
    }
}

impl Passthrough {
    fn inode_of(&self, path: &Path) -> u64 {
        self.inodes.lock().unwrap().ino_for(path.to_path_buf())
    }
}
