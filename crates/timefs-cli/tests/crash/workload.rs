//! The writer: a seeded random workload on the mount that records which tree states a
//! crash may legitimately recover to.
//!
//! TimeFS checkpoints the whole tree atomically under one lock, so after a crash the
//! recovered tree must be exactly one of the states the writer passed through since its
//! last acknowledged sync (fsync, fsyncdir, `timefs checkpoint`, `timefs snap`). States
//! before that sync are forbidden (fsynced data was lost); states that never existed are
//! forbidden too (corruption or a torn checkpoint). Every single syscall below becomes
//! one FUSE request, so each one moves the daemon from one modelled state to the next.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use super::daemon::timefs;
use super::model::{parent_of, Tree};
use super::rng::Rng;

/// Directory names the workload may create. Nested names make directory renames move
/// whole subtrees.
const DIRS: &[&str] = &["a", "b", "a/c", "b/c"];
const FILE_NAMES: &[&str] = &["f0", "f1", "f2"];
/// Kept below the 1 MiB FUSE max_write (and the kernel's per-request page limit), so
/// one `write` is one FUSE request and one atomic model step.
const MAX_WRITE_CHUNK: usize = 16 * 1024;

/// What the main thread learns once the writer stops.
pub struct Outcome {
    /// Allowed recovery states, oldest first; the first is the last synced state.
    pub candidates: Vec<Tree>,
    pub ops: usize,
    pub syncs: usize,
}

/// Flags shared between the writer thread and the thread that kills the daemon.
#[derive(Clone, Default)]
pub struct Signals {
    /// Set just before the SIGKILL. Errors seen while it is clear are real bugs.
    pub killed: Arc<AtomicBool>,
    /// Completed operations, so the killer can choose a crash point.
    pub ops: Arc<AtomicUsize>,
}

/// The writer stopped because the daemon died.
struct Stopped;

type Step = Result<(), Stopped>;

pub struct Writer {
    mnt: PathBuf,
    repo: PathBuf,
    rng: Rng,
    model: Tree,
    candidates: Vec<Tree>,
    signals: Signals,
    ops: usize,
    syncs: usize,
}

impl Writer {
    /// `base` is the tree the mount starts from; it is already durable.
    pub fn new(mnt: &Path, repo: &Path, seed: u64, base: Tree, signals: Signals) -> Self {
        Writer {
            mnt: mnt.to_path_buf(),
            repo: repo.to_path_buf(),
            rng: Rng::new(seed),
            candidates: vec![base.clone()],
            model: base,
            signals,
            ops: 0,
            syncs: 0,
        }
    }

    /// Run operations until the daemon is killed.
    pub fn run(mut self) -> Outcome {
        while self.one_op().is_ok() {
            self.ops += 1;
            self.signals.ops.store(self.ops, Ordering::SeqCst);
        }
        Outcome {
            candidates: self.candidates,
            ops: self.ops,
            syncs: self.syncs,
        }
    }

    fn one_op(&mut self) -> Step {
        loop {
            let done = match self.rng.below(100) {
                0..=19 => Some(self.write_whole_file()),
                20..=34 => self.append(),
                35..=44 => self.write_at(),
                45..=49 => self.truncate(),
                50..=57 => self.rename_file(),
                58..=64 => self.unlink(),
                65..=70 => self.mkdir(),
                71..=73 => self.rmdir(),
                74..=76 => self.rename_dir(),
                77..=84 => self.read_back(),
                85..=91 => self.fsync_file(),
                92..=94 => self.fsync_dir(),
                95..=97 => Some(self.cli_checkpoint()),
                _ => Some(self.cli_snap()),
            };
            // `None` means the chosen operation had no valid target in this state.
            if let Some(step) = done {
                return step;
            }
        }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.mnt.join(rel)
    }

    /// Turn an I/O result into a step. Failing while the daemon is alive is a bug in
    /// TimeFS (the workload only issues operations valid in the model).
    fn check<T>(&self, what: &str, result: io::Result<T>) -> Result<T, Stopped> {
        match result {
            Ok(v) => Ok(v),
            Err(_) if self.signals.killed.load(Ordering::SeqCst) => Err(Stopped),
            Err(e) => panic!("{what} failed while the daemon was alive: {e}"),
        }
    }

    /// Record `next` as a state the daemon may reach, then try to reach it. It stays a
    /// candidate even if the reply is lost: the daemon may have applied it before dying.
    fn step<T>(
        &mut self,
        what: &str,
        next: Tree,
        act: impl FnOnce() -> io::Result<T>,
    ) -> Result<T, Stopped> {
        self.candidates.push(next.clone());
        let value = self.check(what, act())?;
        self.model = next;
        Ok(value)
    }

    /// A sync succeeded: the current state is durable and older ones are no longer
    /// acceptable recovery results.
    fn synced(&mut self) {
        self.candidates = vec![self.model.clone()];
        self.syncs += 1;
    }

    fn random_data(&mut self) -> Vec<u8> {
        let len = match self.rng.below(10) {
            0 => 0,
            1 => self.rng.range(20_000, 60_000),
            _ => self.rng.range(1, 3_000),
        };
        self.rng.bytes(len)
    }

    fn pick_file(&mut self) -> Option<String> {
        let files = self.model.files();
        (!files.is_empty()).then(|| self.rng.pick(&files).clone())
    }

    /// Any file path from the pool whose parent directory exists.
    fn pick_file_slot(&mut self) -> String {
        let mut parents: Vec<&str> = vec![""];
        parents.extend(DIRS.iter().copied().filter(|d| self.model.is_dir(d)));
        let parent = *self.rng.pick(&parents);
        let name = *self.rng.pick(FILE_NAMES);
        if parent.is_empty() {
            name.to_string()
        } else {
            format!("{parent}/{name}")
        }
    }

    /// create or truncate, then write in chunks: each request is its own model state.
    fn write_whole_file(&mut self) -> Step {
        let rel = self.pick_file_slot();
        let data = self.random_data();
        let path = self.path(&rel);
        let next = self.model.with_file(&rel, Vec::new());
        let mut file = self.step("create", next, || File::create(&path))?;
        let mut written = 0;
        for chunk in data.chunks(MAX_WRITE_CHUNK) {
            written += chunk.len();
            let next = self.model.with_file(&rel, data[..written].to_vec());
            self.step("write", next, || write_once(&mut file, chunk))?;
        }
        Ok(())
    }

    fn append(&mut self) -> Option<Step> {
        let rel = self.pick_file()?;
        let len = self.rng.range(1, 4_000);
        let extra = self.rng.bytes(len);
        Some(self.append_to(&rel, &extra))
    }

    fn append_to(&mut self, rel: &str, extra: &[u8]) -> Step {
        let mut data = self.model.file(rel).unwrap().to_vec();
        data.extend_from_slice(extra);
        let open = OpenOptions::new().append(true).open(self.path(rel));
        let mut file = self.check("open append", open)?;
        let next = self.model.with_file(rel, data);
        self.step("append", next, || write_once(&mut file, extra))
    }

    /// A positioned write, possibly past the end so the model must zero-fill a hole.
    fn write_at(&mut self) -> Option<Step> {
        let rel = self.pick_file()?;
        let size = self.model.file(&rel).unwrap().len();
        let offset = self.rng.below(size + 5_000);
        let patch_len = self.rng.range(1, 4_000);
        let patch = self.rng.bytes(patch_len);
        Some(self.write_at_offset(&rel, offset, &patch))
    }

    fn write_at_offset(&mut self, rel: &str, offset: usize, patch: &[u8]) -> Step {
        let mut data = self.model.file(rel).unwrap().to_vec();
        if data.len() < offset + patch.len() {
            data.resize(offset + patch.len(), 0);
        }
        data[offset..offset + patch.len()].copy_from_slice(patch);
        let open = OpenOptions::new().write(true).open(self.path(rel));
        let file = self.check("open write", open)?;
        let next = self.model.with_file(rel, data);
        self.step("pwrite", next, || {
            expect_full(file.write_at(patch, offset as u64)?, patch.len())
        })
    }

    fn truncate(&mut self) -> Option<Step> {
        let rel = self.pick_file()?;
        let len = self.rng.below(self.model.file(&rel).unwrap().len() + 3_000);
        Some(self.truncate_to(&rel, len))
    }

    fn truncate_to(&mut self, rel: &str, len: usize) -> Step {
        let mut data = self.model.file(rel).unwrap().to_vec();
        data.resize(len, 0);
        let open = OpenOptions::new().write(true).open(self.path(rel));
        let file = self.check("open truncate", open)?;
        let next = self.model.with_file(rel, data);
        self.step("truncate", next, || file.set_len(len as u64))
    }

    /// Rename a file onto a free slot or over another file (an atomic replace).
    fn rename_file(&mut self) -> Option<Step> {
        let src = self.pick_file()?;
        let dst = self.pick_file_slot();
        if dst == src {
            return None;
        }
        let next = self.model.renamed(&src, &dst);
        let (from, to) = (self.path(&src), self.path(&dst));
        Some(self.step("rename file", next, || fs::rename(from, to)))
    }

    fn unlink(&mut self) -> Option<Step> {
        let rel = self.pick_file()?;
        let next = self.model.without(&rel);
        let path = self.path(&rel);
        Some(self.step("unlink", next, || fs::remove_file(path)))
    }

    fn mkdir(&mut self) -> Option<Step> {
        let rel = *self.rng.pick(DIRS);
        if self.model.exists(rel) || !self.model.is_dir(parent_of(rel)) {
            return None;
        }
        let next = self.model.with_dir(rel);
        let path = self.path(rel);
        Some(self.step("mkdir", next, || fs::create_dir(path)))
    }

    fn rmdir(&mut self) -> Option<Step> {
        let dirs = self.model.dirs();
        let empty: Vec<&String> = dirs.iter().filter(|d| self.model.is_empty_dir(d)).collect();
        if empty.is_empty() {
            return None;
        }
        let rel = self.rng.pick(&empty).to_string();
        let next = self.model.without(&rel);
        let path = self.path(&rel);
        Some(self.step("rmdir", next, || fs::remove_dir(path)))
    }

    /// Move a directory (with its subtree) to a free pool name outside itself.
    fn rename_dir(&mut self) -> Option<Step> {
        let dirs = self.model.dirs();
        if dirs.is_empty() {
            return None;
        }
        let src = self.rng.pick(&dirs).clone();
        let dst = *self.rng.pick(DIRS);
        let inside_src = dst == src || dst.starts_with(&format!("{src}/"));
        if inside_src || self.model.exists(dst) || !self.model.is_dir(parent_of(dst)) {
            return None;
        }
        let next = self.model.renamed(&src, dst);
        let (from, to) = (self.path(&src), self.path(dst));
        Some(self.step("rename dir", next, || fs::rename(from, to)))
    }

    /// Reads through the live mount must match the model too, not only after recovery.
    fn read_back(&mut self) -> Option<Step> {
        let rel = self.pick_file()?;
        Some(self.read_and_compare(&rel))
    }

    fn read_and_compare(&self, rel: &str) -> Step {
        let got = self.check("read", fs::read(self.path(rel)))?;
        let want = self.model.file(rel).unwrap();
        assert!(
            got == **want,
            "live read of {rel} returned {} bytes, model has {}",
            got.len(),
            want.len()
        );
        Ok(())
    }

    fn fsync_file(&mut self) -> Option<Step> {
        let rel = self.pick_file()?;
        // Opened for writing because that is how applications fsync their data.
        let open = OpenOptions::new().write(true).open(self.path(&rel));
        Some(self.sync_handle("fsync", open))
    }

    fn fsync_dir(&mut self) -> Option<Step> {
        let mut dirs = vec![String::new()];
        dirs.extend(self.model.dirs());
        let rel = self.rng.pick(&dirs).clone();
        Some(self.sync_handle("fsyncdir", File::open(self.path(&rel))))
    }

    fn sync_handle(&mut self, what: &str, open: io::Result<File>) -> Step {
        let file = self.check(what, open)?;
        self.check(what, file.sync_all())?;
        self.synced();
        Ok(())
    }

    /// `timefs checkpoint` fails when no daemon answers, so success means durable.
    fn cli_checkpoint(&mut self) -> Step {
        let out = timefs(&self.repo).arg("checkpoint").output();
        let result = out.and_then(|o| cli_status(&o, "checkpoint"));
        self.check("timefs checkpoint", result)?;
        self.synced();
        Ok(())
    }

    /// `timefs snap` falls back to promoting the saved checkpoint when the daemon is gone,
    /// which would not cover the newest changes, so a snap that ends after the kill is
    /// not counted as a sync.
    fn cli_snap(&mut self) -> Step {
        let out = timefs(&self.repo)
            .args(["snap", "-m", "crash test"])
            .output();
        let result = out.and_then(|o| cli_status(&o, "snap"));
        self.check("timefs snap", result)?;
        if self.signals.killed.load(Ordering::SeqCst) {
            return Err(Stopped);
        }
        self.synced();
        Ok(())
    }
}

fn write_once(file: &mut File, data: &[u8]) -> io::Result<()> {
    let n = file.write(data)?;
    expect_full(n, data.len())
}

/// A short write would split one modelled step into two unknown ones.
fn expect_full(n: usize, want: usize) -> io::Result<()> {
    if n == want {
        Ok(())
    } else {
        Err(io::Error::other(format!("short write: {n} of {want}")))
    }
}

fn cli_status(out: &std::process::Output, what: &str) -> io::Result<()> {
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "timefs {what}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}
