//! FUSE adapter for TimeFS (libfuse3 via `fuser`).
//!
//! `fs` is a thin translation layer over `timefs_core::worktree::WorkTree`; the persistence
//! policy (checkpoints, snapshots, recovery) lives in `engine`.

mod attr;
mod checkpointer;
pub mod control;
pub mod engine;
mod errno;
mod fs;
pub mod mount;
mod shared;
mod statfs;

pub use mount::{mount, MountConfig, MountError, Mounted};
