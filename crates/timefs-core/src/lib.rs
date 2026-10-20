//! TimeFS core: content-addressed, copy-on-write versioning engine.
//!
//! This crate has no FUSE dependency. See `docs/architecture.md` for the design.

#![forbid(unsafe_code)]

pub mod chunk;
pub mod codec;
pub mod error;
pub mod hash;
pub mod object;
pub mod store;

pub use error::{Error, Result};
pub use hash::Hash;
