//! Reachability: which objects are referenced from a set of root commits.
//!
//! Shared by `fsck` (report problems) and `gc` (decide what is garbage). Marking is
//! tolerant: missing or undecodable objects are recorded instead of aborting the walk.

use std::collections::HashSet;

use crate::error::{Error, Result};
use crate::filedata;
use crate::hash::Hash;
use crate::object::{EntryKind, ObjectKind};
use crate::store::{ObjectStore, ObjectStoreExt};

#[derive(Debug, Default)]
pub struct Reachable {
    pub objects: HashSet<Hash>,
    /// Referenced but absent: `(hash, what referenced it)`.
    pub missing: Vec<(Hash, String)>,
    /// Present but undecodable: `(hash, reason)`.
    pub undecodable: Vec<(Hash, String)>,
}

pub fn mark_reachable(store: &dyn ObjectStore, roots: &[Hash]) -> Result<Reachable> {
    let mut out = Reachable::default();
    let mut stack: Vec<(Hash, ObjectKind, String)> = roots
        .iter()
        .map(|h| (*h, ObjectKind::Commit, "ref".to_string()))
        .collect();

    while let Some((hash, kind, referenced_by)) = stack.pop() {
        if !out.objects.insert(hash) {
            continue;
        }
        if !store.has(&hash)? {
            out.objects.remove(&hash);
            out.missing.push((hash, referenced_by));
            continue;
        }
        if kind == ObjectKind::Blob {
            continue;
        }
        if let Err(e) = expand(store, &hash, kind, &mut stack) {
            out.undecodable.push((hash, e.to_string()));
        }
    }
    Ok(out)
}

fn expand(
    store: &dyn ObjectStore,
    hash: &Hash,
    kind: ObjectKind,
    stack: &mut Vec<(Hash, ObjectKind, String)>,
) -> Result<()> {
    let here = format!("{} {}", kind.name(), hash.short());
    match kind {
        ObjectKind::Commit => {
            let commit = store.get_commit(hash)?;
            stack.push((commit.tree, ObjectKind::Tree, here.clone()));
            for p in commit.parents {
                stack.push((p, ObjectKind::Commit, here.clone()));
            }
        }
        ObjectKind::Tree => {
            for e in store.get_tree(hash)?.entries() {
                let child = match e.kind {
                    EntryKind::Dir => ObjectKind::Tree,
                    EntryKind::File => ObjectKind::File,
                    EntryKind::Symlink => ObjectKind::Blob,
                };
                stack.push((e.hash, child, here.clone()));
            }
        }
        // A tree entry names either a flat `File` or a paged `FileIndex`; `file_children`
        // looks at the actual kind and skips holes, which reference nothing.
        ObjectKind::File | ObjectKind::FileIndex => {
            for (child, child_kind) in filedata::file_children(store, hash)? {
                stack.push((child, child_kind, here.clone()));
            }
        }
        ObjectKind::Blob => {}
    }
    Ok(())
}

/// Convenience for callers that only want a hard failure on any problem.
pub fn require_complete(r: &Reachable) -> Result<()> {
    if let Some((h, by)) = r.missing.first() {
        return Err(Error::corrupt(format!("object {h} (from {by}) is missing")));
    }
    if let Some((h, why)) = r.undecodable.first() {
        return Err(Error::corrupt(format!("object {h}: {why}")));
    }
    Ok(())
}
