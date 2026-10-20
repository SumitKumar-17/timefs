//! Read-only navigation over committed trees.

use crate::error::{Error, Result};
use crate::hash::Hash;
use crate::object::{EntryKind, Tree, TreeEntry};
use crate::store::{ObjectStore, ObjectStoreExt};

/// Look up `path` (slash separated, relative to `root`). `""` or `"/"` is the root itself.
pub fn lookup_path(store: &dyn ObjectStore, root: &Hash, path: &str) -> Result<Option<TreeEntry>> {
    let mut current = root_entry(*root);
    for part in path.split('/').filter(|p| !p.is_empty()) {
        if current.kind != EntryKind::Dir {
            return Ok(None);
        }
        let tree = store.get_tree(&current.hash)?;
        match tree.get(part.as_bytes()) {
            Some(entry) => current = entry.clone(),
            None => return Ok(None),
        }
    }
    Ok(Some(current))
}

/// A synthetic entry standing for the root directory of a tree.
pub fn root_entry(tree: Hash) -> TreeEntry {
    TreeEntry {
        name: Vec::new(),
        kind: EntryKind::Dir,
        mode: 0o755,
        uid: 0,
        gid: 0,
        size: 0,
        mtime_ns: 0,
        hash: tree,
    }
}

/// Concatenate a file's extents.
pub fn read_file(store: &dyn ObjectStore, file: &Hash) -> Result<Vec<u8>> {
    let obj = store.get_file(file)?;
    let mut out = Vec::with_capacity(obj.size() as usize);
    for extent in &obj.extents {
        let blob = store.get_blob(&extent.hash)?;
        if blob.len() as u64 != extent.len {
            return Err(Error::corrupt(format!(
                "extent {} has length {} but is recorded as {}",
                extent.hash,
                blob.len(),
                extent.len
            )));
        }
        out.extend_from_slice(&blob);
    }
    Ok(out)
}

/// Visit every entry below `tree` in sorted pre-order, passing the full relative path.
pub fn walk(
    store: &dyn ObjectStore,
    tree: &Hash,
    visit: &mut dyn FnMut(&str, &TreeEntry) -> Result<()>,
) -> Result<()> {
    walk_inner(store, tree, "", visit)
}

fn walk_inner(
    store: &dyn ObjectStore,
    tree: &Hash,
    prefix: &str,
    visit: &mut dyn FnMut(&str, &TreeEntry) -> Result<()>,
) -> Result<()> {
    let dir: Tree = store.get_tree(tree)?;
    for entry in dir.entries() {
        let path = join_path(prefix, &entry.name);
        visit(&path, entry)?;
        if entry.kind == EntryKind::Dir {
            walk_inner(store, &entry.hash, &path, visit)?;
        }
    }
    Ok(())
}

pub fn join_path(prefix: &str, name: &[u8]) -> String {
    let name = String::from_utf8_lossy(name);
    if prefix.is_empty() {
        name.into_owned()
    } else {
        format!("{prefix}/{name}")
    }
}
