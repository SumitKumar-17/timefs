//! Bidirectional map between FUSE inode numbers and host paths.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const ROOT: u64 = 1;

pub struct Inodes {
    by_ino: HashMap<u64, PathBuf>,
    by_path: HashMap<PathBuf, u64>,
    next: u64,
}

impl Inodes {
    pub fn new(root: PathBuf) -> Self {
        let mut t = Inodes {
            by_ino: HashMap::new(),
            by_path: HashMap::new(),
            next: ROOT + 1,
        };
        t.by_ino.insert(ROOT, root.clone());
        t.by_path.insert(root, ROOT);
        t
    }

    pub fn path(&self, ino: u64) -> Option<&Path> {
        self.by_ino.get(&ino).map(PathBuf::as_path)
    }

    /// The inode for `path`, allocating one if the path is new.
    pub fn ino_for(&mut self, path: PathBuf) -> u64 {
        if let Some(&ino) = self.by_path.get(&path) {
            return ino;
        }
        let ino = self.next;
        self.next += 1;
        self.by_ino.insert(ino, path.clone());
        self.by_path.insert(path, ino);
        ino
    }

    pub fn forget_path(&mut self, path: &Path) {
        if let Some(ino) = self.by_path.remove(path) {
            self.by_ino.remove(&ino);
        }
    }

    /// Re-point `from` (and everything below it) at `to` after a rename.
    pub fn rename(&mut self, from: &Path, to: &Path) {
        self.forget_path(to);
        let moved: Vec<(PathBuf, u64)> = self
            .by_path
            .iter()
            .filter(|(p, _)| p.starts_with(from))
            .map(|(p, &i)| (p.clone(), i))
            .collect();
        for (old, ino) in moved {
            let new = to.join(old.strip_prefix(from).expect("prefix checked"));
            self.by_path.remove(&old);
            self.by_path.insert(new.clone(), ino);
            self.by_ino.insert(ino, new);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_stable_inodes_and_follows_renames() {
        let mut t = Inodes::new(PathBuf::from("/r"));
        let a = t.ino_for(PathBuf::from("/r/a"));
        assert_eq!(t.ino_for(PathBuf::from("/r/a")), a);
        let inner = t.ino_for(PathBuf::from("/r/a/x"));
        t.rename(Path::new("/r/a"), Path::new("/r/b"));
        assert_eq!(t.path(a), Some(Path::new("/r/b")));
        assert_eq!(t.path(inner), Some(Path::new("/r/b/x")));
        t.forget_path(Path::new("/r/b"));
        assert_eq!(t.path(a), None);
    }
}
