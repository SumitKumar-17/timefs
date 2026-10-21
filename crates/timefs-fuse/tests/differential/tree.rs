//! Whole-tree snapshots and their comparison.
//!
//! Only properties TimeFS promises to preserve are captured: names, file types, permission
//! bits, file contents (and therefore sizes) and symlink targets. Timestamps, inode numbers,
//! link counts and directory sizes are deliberately left out (ctime equals mtime and
//! directory nlink is fixed at 2 by design), and the root directory itself is not compared
//! because its metadata is not persisted.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    Other,
}

pub fn kind_of(ft: fs::FileType) -> Kind {
    if ft.is_symlink() {
        Kind::Symlink
    } else if ft.is_dir() {
        Kind::Dir
    } else if ft.is_file() {
        Kind::File
    } else {
        Kind::Other
    }
}

/// Permission bits as compared; symlink modes are meaningless on Linux.
pub fn mode_bits(meta: &fs::Metadata) -> u32 {
    meta.permissions().mode() & 0o7777
}

#[derive(PartialEq, Eq)]
pub enum Node {
    Dir { mode: u32 },
    File { mode: u32, data: Vec<u8> },
    Symlink { target: PathBuf },
    Other,
}

impl fmt::Debug for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Node::Dir { mode } => write!(f, "dir {mode:o}"),
            Node::File { mode, data } => write!(f, "file {mode:o} {}", summarize(data)),
            Node::Symlink { target } => write!(f, "symlink -> {}", target.display()),
            Node::Other => write!(f, "special file"),
        }
    }
}

/// Large buffers are summarised by length and a cheap checksum so that messages stay short.
pub fn summarize(data: &[u8]) -> String {
    let fnv = data.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    });
    format!("len={} fnv={fnv:016x}", data.len())
}

pub type Tree = BTreeMap<PathBuf, Node>;

pub fn snapshot(root: &Path) -> io::Result<Tree> {
    let mut tree = Tree::new();
    walk(root, Path::new(""), &mut tree)?;
    Ok(tree)
}

fn walk(root: &Path, rel: &Path, tree: &mut Tree) -> io::Result<()> {
    for entry in fs::read_dir(root.join(rel))? {
        let entry = entry?;
        let rel = rel.join(entry.file_name());
        let path = root.join(&rel);
        let meta = fs::symlink_metadata(&path)?;
        let node = match kind_of(meta.file_type()) {
            Kind::Dir => {
                walk(root, &rel, tree)?;
                Node::Dir {
                    mode: mode_bits(&meta),
                }
            }
            Kind::File => Node::File {
                mode: mode_bits(&meta),
                data: fs::read(&path)?,
            },
            Kind::Symlink => Node::Symlink {
                target: fs::read_link(&path)?,
            },
            Kind::Other => Node::Other,
        };
        tree.insert(rel, node);
    }
    Ok(())
}

/// Human-readable differences, at most `limit` lines.
pub fn differences(timefs: &Tree, oracle: &Tree, limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    for (path, want) in oracle {
        match timefs.get(path) {
            None => out.push(format!(
                "{}: missing on TimeFS (host has {want:?})",
                path.display()
            )),
            Some(got) if got != want => out.push(format!(
                "{}: TimeFS has {got:?}, host has {want:?}",
                path.display()
            )),
            Some(_) => {}
        }
    }
    for (path, got) in timefs {
        if !oracle.contains_key(path) {
            out.push(format!("{}: only on TimeFS ({got:?})", path.display()));
        }
    }
    out.truncate(limit);
    out
}

/// Give the owner full access everywhere so the temporary directory can be deleted even
/// after the run removed write permission from some directories.
pub fn make_removable(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o700));
            make_removable(&path);
        }
    }
}
