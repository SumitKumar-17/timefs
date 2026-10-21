//! TimeFS core: content-addressed, copy-on-write versioning engine.
//!
//! This crate has no FUSE dependency. See `docs/architecture.md` for the design.

#![forbid(unsafe_code)]

pub mod chunk;
pub mod codec;
pub mod diff;
pub mod error;
pub mod filedata;
pub mod fsck;
pub mod gc;
pub mod hash;
pub mod hostfs;
pub mod merge;
pub mod migrate;
pub mod object;
pub mod reach;
pub mod refs;
pub mod repo;
pub mod store;
pub mod timeutil;
pub mod treeops;
pub mod worktree;

pub use error::{Error, Result};
pub use hash::Hash;
pub use repo::Repository;
