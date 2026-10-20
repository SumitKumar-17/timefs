//! Three-way merge of directory trees.
//!
//! Equal hashes short-circuit whole subtrees, so the cost is proportional to the number of
//! changed paths. "Equal" for entries means same kind, content and owner/mode; mtime is
//! ignored, and when two sides are otherwise equal the newer mtime wins.

use std::collections::BTreeSet;

use super::conflict::{Conflict, ConflictKind, Resolve};
use crate::error::Result;
use crate::hash::Hash;
use crate::object::{EntryKind, Tree, TreeEntry};
use crate::store::{ObjectStore, ObjectStoreExt};
use crate::treeops::join_path;

pub struct TreeMerger<'a> {
    store: &'a dyn ObjectStore,
    resolve: Resolve,
    /// Conflicts met so far (resolved ones included, when a policy picked a side).
    pub conflicts: Vec<Conflict>,
}

impl<'a> TreeMerger<'a> {
    pub fn new(store: &'a dyn ObjectStore, resolve: Resolve) -> Self {
        TreeMerger {
            store,
            resolve,
            conflicts: Vec::new(),
        }
    }

    /// Merge `ours` and `theirs` relative to `base` (`None` means no common ancestor tree).
    /// With `Resolve::Abort` the returned tree is only meaningful if `conflicts` is empty.
    pub fn merge(&mut self, base: Option<&Hash>, ours: &Hash, theirs: &Hash) -> Result<Hash> {
        self.merge_dir(base, ours, theirs, "")
    }

    fn merge_dir(
        &mut self,
        base: Option<&Hash>,
        ours: &Hash,
        theirs: &Hash,
        path: &str,
    ) -> Result<Hash> {
        if ours == theirs || base == Some(theirs) {
            return Ok(*ours);
        }
        if base == Some(ours) {
            return Ok(*theirs);
        }
        let base_tree = match base {
            Some(h) => self.store.get_tree(h)?,
            None => Tree::new(),
        };
        let (our_tree, their_tree) = (self.store.get_tree(ours)?, self.store.get_tree(theirs)?);

        let names: BTreeSet<&[u8]> = [&base_tree, &our_tree, &their_tree]
            .iter()
            .flat_map(|t| t.entries().iter().map(|e| e.name.as_slice()))
            .collect();

        let mut merged = Vec::new();
        for name in names {
            let child_path = join_path(path, name);
            let entry = self.merge_entry(
                &child_path,
                base_tree.get(name),
                our_tree.get(name),
                their_tree.get(name),
            )?;
            merged.extend(entry);
        }
        self.store.put_tree(&Tree::from_entries(merged)?)
    }

    /// Merge one name; `None` means the entry is absent from the result.
    fn merge_entry(
        &mut self,
        path: &str,
        base: Option<&TreeEntry>,
        ours: Option<&TreeEntry>,
        theirs: Option<&TreeEntry>,
    ) -> Result<Option<TreeEntry>> {
        if same(ours, theirs) {
            return Ok(ours.zip(theirs).map(|(o, t)| newer(o, t).clone()));
        }
        if same(base, ours) {
            return Ok(theirs.cloned());
        }
        if same(base, theirs) {
            return Ok(ours.cloned());
        }
        // Both sides changed this name in different ways.
        if let (Some(o), Some(t)) = (ours, theirs) {
            if o.kind == EntryKind::Dir && t.kind == EntryKind::Dir {
                return self.merge_dirs(path, base, o, t).map(Some);
            }
        }
        Ok(self.conflict(path, base, ours, theirs))
    }

    fn merge_dirs(
        &mut self,
        path: &str,
        base: Option<&TreeEntry>,
        ours: &TreeEntry,
        theirs: &TreeEntry,
    ) -> Result<TreeEntry> {
        let base_hash = base.filter(|b| b.kind == EntryKind::Dir).map(|b| &b.hash);
        let hash = self.merge_dir(base_hash, &ours.hash, &theirs.hash, path)?;
        // Keep our metadata unless only they changed it relative to the base.
        let meta_from = match base {
            Some(b) if (ours.mode, ours.uid, ours.gid) == (b.mode, b.uid, b.gid) => theirs,
            _ => ours,
        };
        Ok(TreeEntry {
            hash,
            mode: meta_from.mode,
            uid: meta_from.uid,
            gid: meta_from.gid,
            mtime_ns: ours.mtime_ns.max(theirs.mtime_ns),
            ..ours.clone()
        })
    }

    /// Record a conflict and apply the resolution policy.
    fn conflict(
        &mut self,
        path: &str,
        base: Option<&TreeEntry>,
        ours: Option<&TreeEntry>,
        theirs: Option<&TreeEntry>,
    ) -> Option<TreeEntry> {
        let kind = match (base, ours, theirs) {
            (_, Some(o), Some(t)) if o.kind != t.kind => ConflictKind::TypeChange,
            (None, Some(_), Some(_)) => ConflictKind::AddAdd,
            (_, Some(_), Some(_)) => ConflictKind::ModifyModify,
            _ => ConflictKind::ModifyDelete,
        };
        self.conflicts.push(Conflict {
            path: path.to_string(),
            kind,
            ours: ours.cloned(),
            theirs: theirs.cloned(),
        });
        match self.resolve {
            Resolve::Theirs => theirs.cloned(),
            Resolve::Ours | Resolve::Abort => ours.cloned(),
        }
    }
}

/// Entries are the same thing: both absent, or equal in kind, content and ownership.
fn same(a: Option<&TreeEntry>, b: Option<&TreeEntry>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.same_content(b),
        _ => false,
    }
}

fn newer<'e>(a: &'e TreeEntry, b: &'e TreeEntry) -> &'e TreeEntry {
    if b.mtime_ns > a.mtime_ns {
        b
    } else {
        a
    }
}
