//! Mounting: wires the repository, working tree, checkpointer and control socket together.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fuser::{BackgroundSession, Config, MountOption, SessionACL};
use timefs_core::chunk::DEFAULT_BLOCK_SIZE;
use timefs_core::repo::RepoLock;
use timefs_core::Repository;

use crate::checkpointer::Checkpointer;
use crate::control::ControlServer;
use crate::engine::{Engine, Recovery};
use crate::fs::TimeFs;
use crate::shared::Shared;

#[derive(Debug, thiserror::Error)]
pub enum MountError {
    #[error(transparent)]
    Core(#[from] timefs_core::Error),
    #[error("mount failed: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug)]
pub struct MountConfig {
    /// Branch to mount; defaults to the repository's HEAD branch.
    pub branch: Option<String>,
    /// Periodic checkpoint interval; `None` disables the timer.
    pub checkpoint_interval: Option<Duration>,
    /// Checkpoint inline when this many unpersisted bytes accumulate.
    pub flush_threshold: u64,
    /// How long the kernel may cache attributes and entries.
    pub attr_ttl: Duration,
    pub block_size: usize,
    pub author: String,
    /// Let users other than the mounting user access the mount (needs `user_allow_other`).
    pub allow_other: bool,
    /// Checkpoint on shutdown. Tests disable this to simulate a crash.
    pub final_checkpoint: bool,
    pub threads: usize,
}

impl Default for MountConfig {
    fn default() -> Self {
        MountConfig {
            branch: None,
            checkpoint_interval: Some(Duration::from_secs(5)),
            flush_threshold: 256 * 1024 * 1024,
            attr_ttl: Duration::from_secs(1),
            block_size: DEFAULT_BLOCK_SIZE,
            author: std::env::var("USER").unwrap_or_else(|_| "timefs".into()),
            allow_other: false,
            final_checkpoint: true,
            threads: 1,
        }
    }
}

/// A live mount. Unmounting happens through [`Mounted::unmount`] or when the kernel
/// connection ends (for example `fusermount3 -u`); call [`Mounted::finish`] afterwards.
pub struct Mounted {
    session: BackgroundSession,
    shared: Arc<Shared>,
    checkpointer: Option<Checkpointer>,
    control: Option<ControlServer>,
    _lock: RepoLock,
    mountpoint: PathBuf,
    pub recovery: Recovery,
}

pub fn mount(
    repo: Repository,
    mountpoint: &Path,
    cfg: &MountConfig,
) -> Result<Mounted, MountError> {
    let lock = repo.lock_exclusive()?;
    let branch = match &cfg.branch {
        Some(b) => b.clone(),
        None => repo.head_branch()?,
    };
    let socket = repo.control_socket_path();
    let stats_path = repo.root().to_path_buf();
    let engine = Engine::new(Arc::new(repo), &branch, &cfg.author, cfg.block_size)?;
    let (tree, recovery) = engine.open_tree()?;
    let shared = Arc::new(Shared::new(
        tree,
        engine,
        cfg.flush_threshold,
        cfg.final_checkpoint,
        stats_path,
    ));

    let session = fuser::spawn_mount(
        TimeFs::new(Arc::clone(&shared), cfg.attr_ttl),
        mountpoint,
        &fuse_config(cfg),
    )?;
    let control = ControlServer::start(&socket, Arc::clone(&shared))?;
    let checkpointer = cfg
        .checkpoint_interval
        .map(|every| Checkpointer::spawn(Arc::clone(&shared), every));

    Ok(Mounted {
        session,
        shared,
        checkpointer,
        control: Some(control),
        _lock: lock,
        mountpoint: mountpoint.to_path_buf(),
        recovery,
    })
}

fn fuse_config(cfg: &MountConfig) -> Config {
    let mut c = Config::default();
    c.mount_options = vec![
        MountOption::FSName("timefs".into()),
        MountOption::Subtype("timefs".into()),
        MountOption::DefaultPermissions,
    ];
    c.acl = if cfg.allow_other {
        SessionACL::All
    } else {
        SessionACL::Owner
    };
    c.n_threads = Some(cfg.threads.max(1));
    c
}

impl Mounted {
    pub fn mountpoint(&self) -> &Path {
        &self.mountpoint
    }

    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    /// True once the kernel connection has ended (the filesystem was unmounted).
    pub fn is_finished(&self) -> bool {
        self.session.guard.is_finished()
    }

    /// Unmount and shut everything down, persisting remaining changes.
    pub fn unmount(self) -> std::io::Result<()> {
        let Mounted {
            session,
            shared,
            checkpointer,
            control,
            ..
        } = self;
        if let Some(c) = control {
            c.stop();
        }
        if let Some(c) = checkpointer {
            c.stop();
        }
        let result = session.umount_and_join();
        shared.finish();
        result
    }

    /// Shut down after the filesystem was unmounted externally.
    pub fn finish(self) {
        let _ = self.unmount();
    }
}
