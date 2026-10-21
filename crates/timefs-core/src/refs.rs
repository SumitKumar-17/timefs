//! Mutable refs: the only mutable state in a repository.
//!
//! [`Refs`] is the interface; two backends implement it:
//! * [`FileRefs`] (loose repositories): one small text file per ref, updated by writing a
//!   temp file, fsyncing it, renaming it over the target and fsyncing the directory.
//! * the pack store (`store::pack`): ref records in the same append-only log as objects.

use std::fs::{self, File};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::hash::Hash;
use crate::store::ObjectStore;

pub const HEADS: &str = "heads";
pub const WIP: &str = "wip";

/// Check a ref name such as `heads/main` (relative to the `refs/` directory).
pub fn validate_ref_name(name: &str) -> Result<()> {
    let bad_char = |c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'));
    let bad = name.is_empty()
        || name.contains("..")
        || name.starts_with('/')
        || name.ends_with('/')
        || name.ends_with(".lock")
        || name.contains("//")
        || name.chars().any(bad_char)
        || name.split('/').any(|part| part.starts_with('.'));
    if bad {
        return Err(Error::InvalidRefName(name.to_string()));
    }
    Ok(())
}

/// Named pointers to commits.
pub trait Refs: Send + Sync {
    fn read(&self, name: &str) -> Result<Option<Hash>>;

    /// Make every object stored so far durable, then atomically set `name` to `hash`
    /// (the write ordering invariant: a ref never points at an object that could be lost).
    fn write(&self, name: &str, hash: &Hash) -> Result<()>;

    /// Remove a ref; `RefNotFound` if it does not exist.
    fn delete(&self, name: &str) -> Result<()>;

    /// All refs under `prefix` (for example `heads`), sorted by name, with names relative to
    /// `prefix`.
    fn list(&self, prefix: &str) -> Result<Vec<(String, Hash)>>;

    /// Create a ref; fails if it already exists.
    fn create(&self, name: &str, hash: &Hash) -> Result<()> {
        if self.read(name)?.is_some() {
            return Err(Error::RefExists(name.to_string()));
        }
        self.write(name, hash)
    }
}

/// One file per ref below `refs/`.
pub struct FileRefs {
    dir: PathBuf,
    /// Store whose objects must be durable before a ref moves. `None` only in unit tests
    /// that exercise refs alone.
    store: Option<Arc<dyn ObjectStore>>,
    tmp_seq: AtomicU64,
}

impl FileRefs {
    pub fn new(refs_dir: impl Into<PathBuf>) -> Self {
        FileRefs {
            dir: refs_dir.into(),
            store: None,
            tmp_seq: AtomicU64::new(0),
        }
    }

    /// Sync `store` before every ref update.
    pub fn with_store(mut self, store: Arc<dyn ObjectStore>) -> Self {
        self.store = Some(store);
        self
    }

    fn path(&self, name: &str) -> Result<PathBuf> {
        validate_ref_name(name)?;
        Ok(self.dir.join(name))
    }
}

impl Refs for FileRefs {
    fn read(&self, name: &str) -> Result<Option<Hash>> {
        match fs::read_to_string(self.path(name)?) {
            Ok(text) => Ok(Some(Hash::from_hex(text.trim_end_matches('\n'))?)),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn write(&self, name: &str, hash: &Hash) -> Result<()> {
        let path = self.path(name)?;
        if let Some(store) = &self.store {
            store.sync()?;
        }
        let parent = path.parent().expect("ref path has a parent");
        fs::create_dir_all(parent)?;
        let tmp = self.dir.join(format!(
            ".tmp-{}-{}",
            std::process::id(),
            self.tmp_seq.fetch_add(1, Ordering::Relaxed)
        ));
        Ok(write_atomic(&tmp, &path, format!("{hash}\n").as_bytes())?)
    }

    fn delete(&self, name: &str) -> Result<()> {
        let path = self.path(name)?;
        match fs::remove_file(&path) {
            Ok(()) => {
                if let Some(parent) = path.parent() {
                    File::open(parent)?.sync_all()?;
                }
                Ok(())
            }
            Err(e) if e.kind() == ErrorKind::NotFound => Err(Error::RefNotFound(name.to_string())),
            Err(e) => Err(e.into()),
        }
    }

    fn list(&self, prefix: &str) -> Result<Vec<(String, Hash)>> {
        validate_ref_name(prefix)?;
        let mut out = Vec::new();
        collect(&self.dir.join(prefix), String::new(), &mut out)?;
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }
}

/// Crash-safely replace `target` with `contents`: write `tmp`, fsync it, rename it over
/// `target`, then fsync the target's directory so the rename itself is durable. A crash
/// leaves either the old or the new contents, never a truncated file. `tmp` must be on
/// the same filesystem as `target`; it is removed if any step fails.
pub(crate) fn write_atomic(tmp: &Path, target: &Path, contents: &[u8]) -> std::io::Result<()> {
    let result = (|| -> std::io::Result<()> {
        let mut f = File::create(tmp)?;
        f.write_all(contents)?;
        f.sync_all()?;
        fs::rename(tmp, target)?;
        let dir = match target.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        };
        File::open(dir)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}

fn collect(dir: &Path, rel: String, out: &mut Vec<(String, Hash)>) -> Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let rel_name = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        if entry.file_type()?.is_dir() {
            collect(&entry.path(), rel_name, out)?;
        } else {
            let text = fs::read_to_string(entry.path())?;
            out.push((rel_name, Hash::from_hex(text.trim_end_matches('\n'))?));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs() -> (tempfile::TempDir, FileRefs) {
        let d = tempfile::tempdir().unwrap();
        let r = FileRefs::new(d.path().join("refs"));
        (d, r)
    }

    #[test]
    fn name_validation() {
        for ok in ["heads/main", "heads/feature/x-1.2", "wip/main"] {
            assert!(validate_ref_name(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "/a",
            "a/",
            "a//b",
            "a/../b",
            "a b",
            "a.lock",
            "heads/.hidden",
            "h\u{e9}ads/x",
        ] {
            assert!(validate_ref_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn write_read_overwrite_delete() {
        let (_d, r) = refs();
        assert_eq!(r.read("heads/main").unwrap(), None);
        r.write("heads/main", &Hash([1; 32])).unwrap();
        assert_eq!(r.read("heads/main").unwrap(), Some(Hash([1; 32])));
        r.write("heads/main", &Hash([2; 32])).unwrap();
        assert_eq!(r.read("heads/main").unwrap(), Some(Hash([2; 32])));
        r.delete("heads/main").unwrap();
        assert_eq!(r.read("heads/main").unwrap(), None);
        assert!(matches!(r.delete("heads/main"), Err(Error::RefNotFound(_))));
    }

    #[test]
    fn create_refuses_existing() {
        let (_d, r) = refs();
        r.create("heads/a", &Hash([1; 32])).unwrap();
        assert!(matches!(
            r.create("heads/a", &Hash([2; 32])),
            Err(Error::RefExists(_))
        ));
    }

    #[test]
    fn list_is_sorted_and_recursive_and_ignores_temp_files() {
        let (_d, r) = refs();
        r.write("heads/b", &Hash([2; 32])).unwrap();
        r.write("heads/a", &Hash([1; 32])).unwrap();
        r.write("heads/team/x", &Hash([3; 32])).unwrap();
        let names: Vec<_> = r.list("heads").unwrap().into_iter().map(|p| p.0).collect();
        assert_eq!(names, ["a", "b", "team/x"]);
        assert!(r.list("wip").unwrap().is_empty());
    }

    #[test]
    fn corrupt_ref_is_an_error_not_a_panic() {
        let (d, r) = refs();
        r.write("heads/main", &Hash([1; 32])).unwrap();
        fs::write(d.path().join("refs/heads/main"), "not a hash\n").unwrap();
        assert!(r.read("heads/main").is_err());
    }
}
