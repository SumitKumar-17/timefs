//! Structural diff between two trees.
//!
//! Equal subtree hashes are skipped without being read, so the cost is proportional to
//! the number of changed paths, not the size of the trees.

use crate::error::Result;
use crate::hash::Hash;
use crate::object::{EntryKind, Tree, TreeEntry};
use crate::store::{ObjectStore, ObjectStoreExt};
use crate::treeops::join_path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Deleted,
    /// Same kind, different content.
    Modified,
    /// Same content, different mode or owner.
    MetaChanged,
}

#[derive(Clone, Debug)]
pub struct Change {
    pub path: String,
    pub kind: ChangeKind,
    pub old: Option<TreeEntry>,
    pub new: Option<TreeEntry>,
}

/// Diff `old` against `new`. `None` stands for an empty tree.
pub fn diff_trees(
    store: &dyn ObjectStore,
    old: Option<&Hash>,
    new: Option<&Hash>,
) -> Result<Vec<Change>> {
    let mut out = Vec::new();
    diff_dir(store, old, new, "", &mut out)?;
    Ok(out)
}

fn load(store: &dyn ObjectStore, tree: Option<&Hash>) -> Result<Tree> {
    match tree {
        Some(h) => store.get_tree(h),
        None => Ok(Tree::new()),
    }
}

fn diff_dir(
    store: &dyn ObjectStore,
    old: Option<&Hash>,
    new: Option<&Hash>,
    prefix: &str,
    out: &mut Vec<Change>,
) -> Result<()> {
    if old == new {
        return Ok(());
    }
    let (a, b) = (load(store, old)?, load(store, new)?);
    let (mut i, mut j) = (0, 0);
    let (ae, be) = (a.entries(), b.entries());
    while i < ae.len() || j < be.len() {
        let order = match (ae.get(i), be.get(j)) {
            (Some(x), Some(y)) => x.name.cmp(&y.name),
            (Some(_), None) => std::cmp::Ordering::Less,
            _ => std::cmp::Ordering::Greater,
        };
        match order {
            std::cmp::Ordering::Less => {
                emit_subtree(store, &ae[i], prefix, ChangeKind::Deleted, out)?;
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                emit_subtree(store, &be[j], prefix, ChangeKind::Added, out)?;
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                diff_entry(store, &ae[i], &be[j], prefix, out)?;
                i += 1;
                j += 1;
            }
        }
    }
    Ok(())
}

fn diff_entry(
    store: &dyn ObjectStore,
    old: &TreeEntry,
    new: &TreeEntry,
    prefix: &str,
    out: &mut Vec<Change>,
) -> Result<()> {
    let path = join_path(prefix, &old.name);
    if old.kind != new.kind {
        emit_subtree(store, old, prefix, ChangeKind::Deleted, out)?;
        return emit_subtree(store, new, prefix, ChangeKind::Added, out);
    }
    if old.hash != new.hash && old.kind != EntryKind::Dir {
        out.push(change(
            path.clone(),
            ChangeKind::Modified,
            Some(old),
            Some(new),
        ));
    } else if !old.same_content(new) {
        out.push(change(
            path.clone(),
            ChangeKind::MetaChanged,
            Some(old),
            Some(new),
        ));
    }
    if old.kind == EntryKind::Dir {
        diff_dir(store, Some(&old.hash), Some(&new.hash), &path, out)?;
    }
    Ok(())
}

/// Report an entry that exists on one side only, expanding directories.
fn emit_subtree(
    store: &dyn ObjectStore,
    entry: &TreeEntry,
    prefix: &str,
    kind: ChangeKind,
    out: &mut Vec<Change>,
) -> Result<()> {
    let path = join_path(prefix, &entry.name);
    let (old, new) = match kind {
        ChangeKind::Deleted => (Some(entry), None),
        _ => (None, Some(entry)),
    };
    out.push(change(path.clone(), kind, old, new));
    if entry.kind == EntryKind::Dir {
        for child in store.get_tree(&entry.hash)?.entries() {
            emit_subtree(store, child, &path, kind, out)?;
        }
    }
    Ok(())
}

fn change(
    path: String,
    kind: ChangeKind,
    old: Option<&TreeEntry>,
    new: Option<&TreeEntry>,
) -> Change {
    Change {
        path,
        kind,
        old: old.cloned(),
        new: new.cloned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Chunker;
    use crate::hostfs::import_dir;
    use crate::store::LooseStore;
    use std::fs;

    fn summary(changes: &[Change]) -> Vec<(String, ChangeKind)> {
        changes.iter().map(|c| (c.path.clone(), c.kind)).collect()
    }

    fn snapshot(s: &LooseStore, dir: &std::path::Path) -> Hash {
        import_dir(s, dir, &Chunker::default()).unwrap().0
    }

    #[test]
    fn detects_add_delete_modify_and_type_change() {
        let d = tempfile::tempdir().unwrap();
        let s = LooseStore::open(d.path().join("objects")).unwrap();
        let src = d.path().join("src");
        fs::create_dir_all(src.join("dir")).unwrap();
        fs::write(src.join("keep"), "same").unwrap();
        fs::write(src.join("gone"), "bye").unwrap();
        fs::write(src.join("edit"), "v1").unwrap();
        fs::write(src.join("dir/inner"), "x").unwrap();
        fs::write(src.join("swap"), "was a file").unwrap();
        let a = snapshot(&s, &src);

        fs::remove_file(src.join("gone")).unwrap();
        fs::write(src.join("edit"), "v2").unwrap();
        fs::write(src.join("new"), "n").unwrap();
        fs::remove_file(src.join("swap")).unwrap();
        fs::create_dir(src.join("swap")).unwrap();
        fs::write(src.join("swap/now"), "dir").unwrap();
        let b = snapshot(&s, &src);

        let got = summary(&diff_trees(&s, Some(&a), Some(&b)).unwrap());
        use ChangeKind::*;
        assert!(got.contains(&("gone".into(), Deleted)));
        assert!(got.contains(&("edit".into(), Modified)));
        assert!(got.contains(&("new".into(), Added)));
        assert!(got.contains(&("swap".into(), Deleted)));
        assert!(got.contains(&("swap".into(), Added)));
        assert!(got.contains(&("swap/now".into(), Added)));
        assert!(!got.iter().any(|(p, _)| p == "keep" || p.starts_with("dir")));
    }

    #[test]
    fn identical_trees_have_no_changes_and_none_means_empty() {
        let d = tempfile::tempdir().unwrap();
        let s = LooseStore::open(d.path().join("objects")).unwrap();
        let src = d.path().join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("f"), "1").unwrap();
        let a = snapshot(&s, &src);
        assert!(diff_trees(&s, Some(&a), Some(&a)).unwrap().is_empty());
        let all = diff_trees(&s, None, Some(&a)).unwrap();
        assert_eq!(summary(&all), [("f".to_string(), ChangeKind::Added)]);
    }

    #[test]
    fn mode_only_change_is_reported_as_meta() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let s = LooseStore::open(d.path().join("objects")).unwrap();
        let src = d.path().join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("f"), "1").unwrap();
        let a = snapshot(&s, &src);
        fs::set_permissions(src.join("f"), fs::Permissions::from_mode(0o700)).unwrap();
        let b = snapshot(&s, &src);
        let got = summary(&diff_trees(&s, Some(&a), Some(&b)).unwrap());
        assert_eq!(got, [("f".to_string(), ChangeKind::MetaChanged)]);
    }
}
