//! POSIX-style errors for working tree operations.
//!
//! These are deliberately errno-free: translating them to numbers is the job of the
//! FUSE adapter, so the core stays independent of the kernel interface.

use crate::error::Error;

pub type FsResult<T> = std::result::Result<T, FsError>;

#[derive(Debug, thiserror::Error)]
pub enum FsError {
    #[error("no such file or directory")]
    NotFound,
    #[error("file exists")]
    Exists,
    #[error("not a directory")]
    NotDir,
    #[error("is a directory")]
    IsDir,
    #[error("directory not empty")]
    NotEmpty,
    #[error("file name too long")]
    NameTooLong,
    #[error("invalid argument: {0}")]
    InvalidArgument(&'static str),
    #[error("operation not supported: {0}")]
    Unsupported(&'static str),
    #[error("storage error: {0}")]
    Store(#[from] Error),
}

impl FsError {
    /// True when both errors are the same variant (payloads ignored). Used by tests that
    /// compare against a reference model.
    pub fn same_kind(&self, other: &FsError) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}
