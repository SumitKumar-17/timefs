//! Shared benchmark environment: configuration, scratch space and repository helpers.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use timefs_core::chunk::Chunker;
use timefs_core::hostfs::import_dir;
use timefs_core::store::ObjectStore;
use timefs_core::worktree::{Options, WorkTree};
use timefs_core::{Hash, Repository};

#[derive(Clone, Debug)]
pub struct Config {
    /// Scratch directory (should live on the disk under test, not tmpfs).
    pub work: PathBuf,
    /// Repetitions per measurement.
    pub repeat: usize,
    /// File counts for the scaling scenarios.
    pub sizes: Vec<usize>,
    pub file_size: usize,
}

impl Config {
    /// A fresh, empty directory below the scratch area.
    pub fn scratch(&self, name: &str) -> io::Result<PathBuf> {
        let dir = self.work.join(name);
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        fs::create_dir_all(&dir)?;
        Ok(dir)
    }
}

/// Create a repository in a fresh scratch directory.
pub fn new_repo(cfg: &Config, name: &str) -> io::Result<Repository> {
    let dir = cfg.scratch(name)?;
    Repository::init(dir.join("repo")).map_err(io::Error::other)
}

/// Import `dir` and commit it on `main`; returns the commit.
pub fn import_commit(repo: &Repository, dir: &Path, chunker: &Chunker, msg: &str) -> Hash {
    let (tree, _) = import_dir(repo.store().as_ref(), dir, chunker).expect("import");
    repo.commit_to_branch("main", tree, "bench", msg)
        .expect("commit")
}

/// A working tree over `tree` with the default block size.
pub fn open_tree(repo: &Repository, tree: Hash) -> WorkTree {
    let store: Arc<dyn ObjectStore> = Arc::clone(repo.store());
    WorkTree::new(store, Some(tree), Options::default())
}

/// Run an external command, returning whether it succeeded.
pub fn run_ok(program: &str, args: &[&str], cwd: Option<&Path>) -> bool {
    let mut c = Command::new(program);
    c.args(args);
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    c.output().map(|o| o.status.success()).unwrap_or(false)
}

pub fn have(program: &str) -> bool {
    Command::new(program).arg("--version").output().is_ok()
}

/// Count inodes used on the filesystem holding `path` (from `df -i`).
pub fn inodes_used(path: &Path) -> Option<u64> {
    let out = Command::new("df")
        .args(["-P", "-i"])
        .arg(path)
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    text.lines().nth(1)?.split_whitespace().nth(2)?.parse().ok()
}

/// Bytes physically allocated below `path` (from `du -sk`).
pub fn disk_bytes(path: &Path) -> Option<u64> {
    let out = Command::new("du").args(["-sk"]).arg(path).output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    text.split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()
        .map(|k| k * 1024)
}
