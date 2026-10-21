//! Executing one operation against one side (the mount or the oracle directory).
//!
//! Everything goes through plain `std::fs` so the kernel VFS and the FUSE protocol are
//! exercised exactly as an ordinary program would exercise them.

use std::fmt;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{symlink, DirBuilderExt, FileExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::support::ops::{Op, SLOTS};
use crate::support::rng::payload;
use crate::support::tree::{kind_of, mode_bits, summarize, Kind};

/// Mode for newly created files; the process umask applies on both sides alike.
const FILE_MODE: u32 = 0o644;
/// Upper bound for reading a held handle to its end.
const READ_ALL: usize = 16 << 20;

/// How an operation failed. OS errors compare by errno, anything else by kind.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ErrCode {
    Os(i32),
    Kind(io::ErrorKind),
}

impl From<io::Error> for ErrCode {
    fn from(e: io::Error) -> Self {
        e.raw_os_error()
            .map_or(ErrCode::Kind(e.kind()), ErrCode::Os)
    }
}

impl fmt::Debug for ErrCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrCode::Os(n) => write!(f, "Err({})", io::Error::from_raw_os_error(*n)),
            ErrCode::Kind(k) => write!(f, "Err({k:?})"),
        }
    }
}

/// What a successful operation observed.
#[derive(PartialEq, Eq)]
pub enum Output {
    Done,
    Data(Vec<u8>),
    Target(PathBuf),
    Names(Vec<String>),
    /// Directory sizes are host specific, so `size` is only recorded for non-directories.
    Meta {
        kind: Kind,
        size: Option<u64>,
        mode: Option<u32>,
    },
    NoHandle,
}

impl fmt::Debug for Output {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Output::Done => write!(f, "Ok"),
            Output::Data(d) => write!(f, "Data({})", summarize(d)),
            Output::Target(t) => write!(f, "Target({})", t.display()),
            Output::Names(n) => write!(f, "Names({n:?})"),
            Output::Meta { kind, size, mode } => {
                write!(
                    f,
                    "Meta({kind:?}, size {size:?}, mode {:?})",
                    mode.map(|m| format!("{m:o}"))
                )
            }
            Output::NoHandle => write!(f, "NoHandle"),
        }
    }
}

pub type Outcome = Result<Output, ErrCode>;

/// One directory tree plus the handles held open in it.
pub struct Side {
    root: PathBuf,
    held: [Option<File>; SLOTS],
}

impl Side {
    pub fn new(root: PathBuf) -> Self {
        Side {
            root,
            held: Default::default(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Close every held handle; required before unmounting.
    pub fn release_all(&mut self) {
        self.held = Default::default();
    }

    pub fn apply(&mut self, op: &Op) -> Outcome {
        self.run(op).map_err(ErrCode::from)
    }

    fn run(&mut self, op: &Op) -> io::Result<Output> {
        let p = |rel: &str| self.root.join(rel);
        match op {
            Op::Write {
                path,
                offset,
                len,
                fill,
            } => {
                let f = open_for_write(&p(path), false)?;
                f.write_all_at(&payload(*len, *fill), *offset)?;
            }
            Op::Read { path, offset, len } => {
                return read_range(&File::open(p(path))?, *offset, *len).map(Output::Data)
            }
            Op::Append { path, len, fill } => {
                open_for_write(&p(path), true)?.write_all(&payload(*len, *fill))?;
            }
            Op::Truncate { path } => {
                OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(p(path))?;
            }
            Op::SetLen { path, len } => OpenOptions::new()
                .write(true)
                .open(p(path))?
                .set_len(*len)?,
            Op::Mkdir { path, mode } => DirBuilder::new().mode(*mode).create(p(path))?,
            Op::Rmdir { path } => fs::remove_dir(p(path))?,
            Op::Unlink { path } => fs::remove_file(p(path))?,
            Op::Rename { from, to } => fs::rename(p(from), p(to))?,
            Op::Symlink { target, path } => symlink(target, p(path))?,
            Op::Readlink { path } => return fs::read_link(p(path)).map(Output::Target),
            Op::Chmod { path, mode } => {
                fs::set_permissions(p(path), fs::Permissions::from_mode(*mode))?
            }
            Op::Fsync { path } => File::open(p(path))?.sync_all()?,
            Op::Stat { path } => return stat(&p(path)),
            Op::ReadDir { path } => return list(&p(path)),
            Op::Hold { slot, path } => {
                // Clear first so a failed open leaves both sides with an empty slot.
                self.held[*slot] = None;
                self.held[*slot] = Some(OpenOptions::new().read(true).write(true).open(p(path))?);
            }
            Op::ReadHeld { slot } => {
                let Some(f) = &self.held[*slot] else {
                    return Ok(Output::NoHandle);
                };
                return read_range(f, 0, READ_ALL).map(Output::Data);
            }
            Op::WriteHeld {
                slot,
                offset,
                len,
                fill,
            } => {
                let Some(f) = &self.held[*slot] else {
                    return Ok(Output::NoHandle);
                };
                f.write_all_at(&payload(*len, *fill), *offset)?;
            }
            Op::Release { slot } => self.held[*slot] = None,
        }
        Ok(Output::Done)
    }
}

fn open_for_write(path: &Path, append: bool) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .append(append)
        .create(true)
        .mode(FILE_MODE)
        .open(path)
}

/// `pread` until `len` bytes or end of file, like a careful reader would.
fn read_range(f: &File, offset: u64, len: usize) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = vec![0u8; 128 * 1024];
    while out.len() < len {
        let want = buf.len().min(len - out.len());
        let n = f.read_at(&mut buf[..want], offset + out.len() as u64)?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
    }
    Ok(out)
}

fn stat(path: &Path) -> io::Result<Output> {
    let meta = fs::symlink_metadata(path)?;
    let kind = kind_of(meta.file_type());
    Ok(Output::Meta {
        kind,
        size: (kind != Kind::Dir).then_some(meta.len()),
        mode: (kind != Kind::Symlink).then_some(mode_bits(&meta)),
    })
}

fn list(path: &Path) -> io::Result<Output> {
    let mut names = fs::read_dir(path)?
        .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<io::Result<Vec<_>>>()?;
    names.sort();
    Ok(Output::Names(names))
}
