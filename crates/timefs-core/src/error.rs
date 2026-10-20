//! Error type for the versioning engine.

use std::io;

use crate::hash::Hash;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("object {0} not found")]
    NotFound(Hash),

    #[error("object {expected} is corrupt (content hashes to {actual})")]
    HashMismatch { expected: Hash, actual: Hash },

    #[error("corrupt data: {0}")]
    Corrupt(String),

    #[error("object {hash} has kind {actual}, expected {expected}")]
    WrongKind {
        hash: Hash,
        expected: &'static str,
        actual: &'static str,
    },

    #[error("invalid name {0:?}")]
    InvalidName(String),

    #[error("invalid ref name {0:?}")]
    InvalidRefName(String),

    #[error("ref {0:?} not found")]
    RefNotFound(String),

    #[error("ref {0:?} already exists")]
    RefExists(String),

    #[error("cannot resolve revision {0:?}")]
    BadRevision(String),

    #[error("revision prefix {0:?} is ambiguous")]
    AmbiguousRevision(String),

    #[error("not a TimeFS repository: {0}")]
    NotARepository(String),

    #[error("unsupported repository format version {0}")]
    UnsupportedFormat(String),

    #[error("repository is locked by another process (is it mounted?)")]
    Locked,

    #[error("{0}")]
    Invalid(String),
}

impl Error {
    pub fn corrupt(msg: impl Into<String>) -> Self {
        Error::Corrupt(msg.into())
    }
}
