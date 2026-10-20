//! Shared helpers for tests that mount a real filesystem.

use std::path::{Path, PathBuf};

use timefs_fuse::{MountConfig, Mounted};

/// A repository plus a mountpoint inside a temporary directory.
pub struct Env {
    pub dir: tempfile::TempDir,
    pub repo: PathBuf,
    pub mnt: PathBuf,
}

impl Env {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let mnt = dir.path().join("mnt");
        std::fs::create_dir(&mnt).unwrap();
        timefs_core::Repository::init(&repo).unwrap();
        Env { dir, repo, mnt }
    }

    pub fn open_repo(&self) -> timefs_core::Repository {
        timefs_core::Repository::open(&self.repo).unwrap()
    }

    pub fn socket(&self) -> PathBuf {
        self.repo.join("timefs.sock")
    }

    pub fn config() -> MountConfig {
        MountConfig {
            checkpoint_interval: None,
            attr_ttl: std::time::Duration::from_millis(1),
            ..MountConfig::default()
        }
    }

    /// Mount with the default test config. `None` means FUSE is unavailable here and the
    /// test should be skipped (unless `TIMEFS_REQUIRE_FUSE=1`, in which case it fails).
    pub fn mount(&self) -> Option<Mounted> {
        self.mount_with(&Self::config())
    }

    pub fn mount_with(&self, cfg: &MountConfig) -> Option<Mounted> {
        if !Path::new("/dev/fuse").exists() {
            return skip("/dev/fuse is missing");
        }
        match timefs_fuse::mount(self.open_repo(), &self.mnt, cfg) {
            Ok(m) => Some(m),
            Err(e) => skip(&format!("mount failed: {e}")),
        }
    }
}

fn skip(why: &str) -> Option<Mounted> {
    if std::env::var("TIMEFS_REQUIRE_FUSE").is_ok_and(|v| v == "1") {
        panic!("FUSE is required but unavailable: {why}");
    }
    eprintln!("SKIPPED: {why}");
    None
}

/// Mount or return from the calling test.
#[macro_export]
macro_rules! mount_or_skip {
    ($env:expr) => {
        match $env.mount() {
            Some(m) => m,
            None => return,
        }
    };
    ($env:expr, $cfg:expr) => {
        match $env.mount_with($cfg) {
            Some(m) => m,
            None => return,
        }
    };
}
