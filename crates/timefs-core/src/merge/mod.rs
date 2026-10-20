//! Merging of branches (ADR-0006): a tree-level 3-way merge with explicit conflicts.

mod conflict;
mod tree;

pub use conflict::{Conflict, ConflictKind, Resolve};
pub use tree::TreeMerger;

use crate::error::Result;
use crate::hash::Hash;
use crate::repo::Repository;

#[derive(Debug)]
pub enum MergeOutcome {
    /// `theirs` is already contained in `ours`.
    UpToDate,
    /// `ours` is an ancestor of `theirs`: just move the branch to `tip`.
    FastForward { tip: Hash },
    /// A merge commit was created (not yet attached to any branch).
    Merged {
        commit: Hash,
        /// Conflicts that the resolution policy settled.
        resolved: Vec<Conflict>,
    },
    /// Conflicts were found and the policy was `Abort`; nothing was created.
    Conflicts(Vec<Conflict>),
}

/// Merge commit `theirs` into commit `ours`. Does not move any ref; the caller decides.
pub fn merge_commits(
    repo: &Repository,
    ours: &Hash,
    theirs: &Hash,
    resolve: Resolve,
    author: &str,
    message: &str,
) -> Result<MergeOutcome> {
    let base = repo.merge_base(ours, theirs)?;
    if base == Some(*theirs) {
        return Ok(MergeOutcome::UpToDate);
    }
    if base == Some(*ours) {
        return Ok(MergeOutcome::FastForward { tip: *theirs });
    }
    let base_tree = match base {
        Some(b) => Some(repo.get_commit(&b)?.tree),
        None => None,
    };
    let our_tree = repo.get_commit(ours)?.tree;
    let their_tree = repo.get_commit(theirs)?.tree;

    let mut merger = TreeMerger::new(repo.store().as_ref(), resolve);
    let tree = merger.merge(base_tree.as_ref(), &our_tree, &their_tree)?;
    if !merger.conflicts.is_empty() && resolve == Resolve::Abort {
        return Ok(MergeOutcome::Conflicts(merger.conflicts));
    }
    let commit = repo.create_commit(tree, vec![*ours, *theirs], author, message)?;
    Ok(MergeOutcome::Merged {
        commit,
        resolved: merger.conflicts,
    })
}

#[cfg(test)]
mod tests;
