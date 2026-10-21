//! In-memory model of the mounted tree: what the writer believes the filesystem holds.
//!
//! Only names, kinds and file bytes are modelled. Modes and times are deliberately left
//! out: the crash contract is about data, and timestamps would make every state unique.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, PartialEq, Eq)]
pub enum Node {
    Dir,
    /// Shared so that the many intermediate states kept between syncs stay cheap to clone.
    File(Arc<Vec<u8>>),
}

/// Paths are relative, `/`-separated, without a leading slash; the root is implicit.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Tree(BTreeMap<String, Node>);

pub fn parent_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(p, _)| p)
}

impl Tree {
    pub fn is_dir(&self, path: &str) -> bool {
        path.is_empty() || matches!(self.0.get(path), Some(Node::Dir))
    }

    pub fn exists(&self, path: &str) -> bool {
        path.is_empty() || self.0.contains_key(path)
    }

    pub fn file(&self, path: &str) -> Option<&Arc<Vec<u8>>> {
        match self.0.get(path) {
            Some(Node::File(data)) => Some(data),
            _ => None,
        }
    }

    pub fn files(&self) -> Vec<String> {
        self.paths(|n| matches!(n, Node::File(_)))
    }

    pub fn dirs(&self) -> Vec<String> {
        self.paths(|n| matches!(n, Node::Dir))
    }

    fn paths(&self, keep: impl Fn(&Node) -> bool) -> Vec<String> {
        self.0
            .iter()
            .filter(|(_, n)| keep(n))
            .map(|(p, _)| p.clone())
            .collect()
    }

    pub fn is_empty_dir(&self, path: &str) -> bool {
        self.is_dir(path) && self.children(path).next().is_none()
    }

    fn children<'a>(&'a self, dir: &'a str) -> impl Iterator<Item = &'a String> + 'a {
        self.0.keys().filter(move |p| is_under(p, dir))
    }

    pub fn with_file(&self, path: &str, data: Vec<u8>) -> Tree {
        let mut t = self.clone();
        t.0.insert(path.to_string(), Node::File(Arc::new(data)));
        t
    }

    pub fn with_dir(&self, path: &str) -> Tree {
        let mut t = self.clone();
        t.0.insert(path.to_string(), Node::Dir);
        t
    }

    pub fn without(&self, path: &str) -> Tree {
        let mut t = self.clone();
        t.0.remove(path);
        t
    }

    /// POSIX rename: `src` (and its subtree) replaces whatever file sat at `dst`.
    pub fn renamed(&self, src: &str, dst: &str) -> Tree {
        let mut t = self.clone();
        let moved: Vec<String> =
            t.0.keys()
                .filter(|p| p.as_str() == src || is_under(p, src))
                .cloned()
                .collect();
        t.0.remove(dst);
        for old in moved {
            let node = t.0.remove(&old).expect("listed above");
            let new = format!("{dst}{}", &old[src.len()..]);
            t.0.insert(new, node);
        }
        t
    }

    /// Read the whole tree under `root`. Any I/O error (for example `EIO` from a torn
    /// object) is returned, because recovered data must always be readable.
    pub fn read_from(root: &Path) -> io::Result<Tree> {
        let mut tree = Tree::default();
        read_dir_into(root, "", &mut tree)?;
        Ok(tree)
    }

    /// A short human-readable description of how `self` differs from `other`.
    pub fn diff(&self, other: &Tree) -> String {
        let mut out = Vec::new();
        for (path, node) in &self.0 {
            match other.0.get(path) {
                None => out.push(format!("  only in left: {path}")),
                Some(o) if o != node => out.push(format!(
                    "  differs: {path} ({} vs {})",
                    describe(node),
                    describe(o)
                )),
                Some(_) => {}
            }
        }
        for path in other.0.keys().filter(|p| !self.0.contains_key(*p)) {
            out.push(format!("  only in right: {path}"));
        }
        out.join("\n")
    }
}

fn is_under(path: &str, dir: &str) -> bool {
    if dir.is_empty() {
        return !path.contains('/');
    }
    path.len() > dir.len() && path.starts_with(dir) && path.as_bytes()[dir.len()] == b'/'
}

fn describe(node: &Node) -> String {
    match node {
        Node::Dir => "dir".into(),
        Node::File(d) => format!("{} bytes", d.len()),
    }
}

fn read_dir_into(root: &Path, rel: &str, tree: &mut Tree) -> io::Result<()> {
    for entry in fs::read_dir(root.join(rel))? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .expect("workload uses ASCII");
        let path = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        let kind = entry.file_type()?;
        if kind.is_dir() {
            tree.0.insert(path.clone(), Node::Dir);
            read_dir_into(root, &path, tree)?;
        } else if kind.is_file() {
            let data = fs::read(root.join(&path))?;
            tree.0.insert(path, Node::File(Arc::new(data)));
        } else {
            return Err(io::Error::other(format!("unexpected file type at {path}")));
        }
    }
    Ok(())
}
