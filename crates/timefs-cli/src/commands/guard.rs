//! Preconditions shared by commands that must not race a live working tree.

use std::error::Error;

use timefs_core::repo::RepoLock;
use timefs_core::{Error as CoreError, Repository};

/// Take the exclusive repository lock, explaining a failure in user terms.
pub fn exclusive(repo: &Repository) -> Result<RepoLock, Box<dyn Error>> {
    repo.lock_exclusive().map_err(|e| match e {
        CoreError::Locked => "the repository is mounted; unmount it first".into(),
        other => other.into(),
    })
}

/// Fail if `branch` has changes that were never snapshotted, because moving the branch
/// would orphan them.
pub fn require_snapshotted(repo: &Repository, branch: &str) -> Result<(), Box<dyn Error>> {
    if repo.has_checkpoint(branch)? {
        return Err(
            format!("branch {branch} has unsnapshotted changes; run `timefs snap` first").into(),
        );
    }
    Ok(())
}
