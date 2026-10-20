//! Conflict types and resolution policy for merges.

use crate::object::TreeEntry;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictKind {
    /// Both sides added the same path with different content.
    AddAdd,
    /// Both sides changed the same file differently.
    ModifyModify,
    /// One side changed a path the other deleted.
    ModifyDelete,
    /// The path is a file on one side and a directory (or symlink) on the other.
    TypeChange,
}

#[derive(Clone, Debug)]
pub struct Conflict {
    pub path: String,
    pub kind: ConflictKind,
    pub ours: Option<TreeEntry>,
    pub theirs: Option<TreeEntry>,
}

/// What to do when both sides changed the same path (ADR-0006).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Resolve {
    /// Report conflicts and produce no merge (the default).
    #[default]
    Abort,
    Ours,
    Theirs,
}
