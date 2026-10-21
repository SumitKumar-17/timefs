//! Import a host directory into the object store, and export a tree back out.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::Path;

use crate::chunk::Chunker;
use crate::error::{Error, Result};
use crate::hash::Hash;
use crate::object::{EntryKind, Extent, FileObj, Tree, TreeEntry};
use crate::store::{ObjectStore, ObjectStoreExt};
use crate::timeutil::{ns_to_system_time, system_time_to_ns};

const PERM_MASK: u32 = 0o7777;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TreeStats {
    pub files: u64,
    pub dirs: u64,
    pub symlinks: u64,
    /// Entries that are neither files, directories nor symlinks (sockets, fifos, devices).
    pub skipped: u64,
    pub bytes: u64,
}

/// Snapshot `dir` into the store and return the root tree hash.
pub fn import_dir(
    store: &dyn ObjectStore,
    dir: &Path,
    chunker: &Chunker,
) -> Result<(Hash, TreeStats)> {
    chunker.validate()?;
    let mut stats = TreeStats::default();
    let tree = import_tree(store, dir, chunker, &mut stats)?;
    Ok((tree, stats))
}

fn import_tree(
    store: &dyn ObjectStore,
    dir: &Path,
    chunker: &Chunker,
    stats: &mut TreeStats,
) -> Result<Hash> {
    let mut entries = Vec::new();
    for item in fs::read_dir(dir)? {
        let item = item?;
        let meta = fs::symlink_metadata(item.path())?;
        let ft = meta.file_type();
        let (kind, hash, size) = if ft.is_dir() {
            stats.dirs += 1;
            (
                EntryKind::Dir,
                import_tree(store, &item.path(), chunker, stats)?,
                0,
            )
        } else if ft.is_file() {
            let (hash, size) = import_file(store, &item.path(), chunker)?;
            stats.files += 1;
            stats.bytes += size;
            (EntryKind::File, hash, size)
        } else if ft.is_symlink() {
            let target = fs::read_link(item.path())?;
            let bytes = target.as_os_str().as_bytes();
            stats.symlinks += 1;
            (
                EntryKind::Symlink,
                store.put_blob(bytes)?,
                bytes.len() as u64,
            )
        } else {
            stats.skipped += 1;
            continue;
        };
        entries.push(TreeEntry {
            name: item.file_name().as_bytes().to_vec(),
            kind,
            mode: meta.mode() & PERM_MASK,
            uid: meta.uid(),
            gid: meta.gid(),
            size,
            mtime_ns: mtime_ns(&meta),
            hash,
        });
    }
    store.put_tree(&Tree::from_entries(entries)?)
}

fn import_file(store: &dyn ObjectStore, path: &Path, chunker: &Chunker) -> Result<(Hash, u64)> {
    let mut extents = Vec::new();
    chunker.for_each_chunk(File::open(path)?, |chunk| {
        // All-zero blocks become holes: no blob, and the file stays sparse on export.
        extents.push(if chunk.iter().all(|&b| b == 0) {
            Extent::hole(chunk.len() as u64)
        } else {
            Extent {
                hash: store.put_blob(chunk)?,
                len: chunk.len() as u64,
            }
        });
        Ok(())
    })?;
    let file = FileObj { extents };
    let size = file.size();
    Ok((store.put_file(&file)?, size))
}

fn mtime_ns(meta: &fs::Metadata) -> i64 {
    meta.modified().map(system_time_to_ns).unwrap_or(0)
}

/// Write `tree` under `dest`, which must not exist or must be an empty directory.
pub fn export_tree(store: &dyn ObjectStore, tree: &Hash, dest: &Path) -> Result<TreeStats> {
    match fs::read_dir(dest) {
        Ok(mut it) => {
            if it.next().is_some() {
                return Err(Error::Invalid(format!("{} is not empty", dest.display())));
            }
        }
        Err(e) if e.kind() == ErrorKind::NotFound => fs::create_dir_all(dest)?,
        Err(e) => return Err(e.into()),
    }
    let mut stats = TreeStats::default();
    export_dir(store, tree, dest, &mut stats)?;
    Ok(stats)
}

fn export_dir(
    store: &dyn ObjectStore,
    tree: &Hash,
    dest: &Path,
    stats: &mut TreeStats,
) -> Result<()> {
    for e in store.get_tree(tree)?.entries() {
        let path = dest.join(OsStr::from_bytes(&e.name));
        match e.kind {
            EntryKind::Dir => {
                fs::create_dir(&path)?;
                stats.dirs += 1;
                export_dir(store, &e.hash, &path, stats)?;
            }
            EntryKind::File => {
                write_file(store, &e.hash, &path)?;
                stats.files += 1;
                stats.bytes += e.size;
            }
            EntryKind::Symlink => {
                let target = store.get_blob(&e.hash)?;
                symlink(OsStr::from_bytes(&target), &path)?;
                stats.symlinks += 1;
                continue;
            }
        }
        apply_metadata(&path, e)?;
    }
    Ok(())
}

/// Write a file's data; holes are skipped over so the copy is sparse too.
fn write_file(store: &dyn ObjectStore, file: &Hash, path: &Path) -> Result<()> {
    use std::io::{Seek, SeekFrom, Write};
    let obj = store.get_file(file)?;
    let mut out = File::create(path)?;
    for extent in &obj.extents {
        if extent.is_hole() {
            out.seek(SeekFrom::Current(extent.len as i64))?;
        } else {
            out.write_all(&store.get_blob(&extent.hash)?)?;
        }
    }
    out.set_len(obj.size())?;
    Ok(())
}

/// Best effort: ownership needs privileges, so a failed chown is ignored.
fn apply_metadata(path: &Path, e: &TreeEntry) -> Result<()> {
    let _ = std::os::unix::fs::chown(path, Some(e.uid), Some(e.gid));
    fs::set_permissions(path, fs::Permissions::from_mode(e.mode))?;
    File::open(path)?.set_modified(ns_to_system_time(e.mtime_ns))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::diff_trees;
    use crate::store::LooseStore;
    use crate::treeops;

    fn store() -> (tempfile::TempDir, LooseStore) {
        let d = tempfile::tempdir().unwrap();
        let s = LooseStore::open(d.path().join("objects")).unwrap();
        (d, s)
    }

    fn sample(dir: &Path) {
        fs::create_dir_all(dir.join("sub/deeper")).unwrap();
        fs::create_dir(dir.join("empty")).unwrap();
        fs::write(dir.join("a.txt"), "hello").unwrap();
        fs::write(dir.join("sub/b.bin"), vec![7u8; 200_000]).unwrap();
        fs::write(dir.join("sub/deeper/zero"), b"").unwrap();
        fs::set_permissions(dir.join("a.txt"), fs::Permissions::from_mode(0o600)).unwrap();
        symlink("a.txt", dir.join("link")).unwrap();
    }

    #[test]
    fn import_then_export_reproduces_the_directory() {
        let (d, s) = store();
        let src = d.path().join("src");
        fs::create_dir(&src).unwrap();
        sample(&src);

        let (tree, stats) = import_dir(&s, &src, &Chunker::default()).unwrap();
        assert_eq!((stats.files, stats.symlinks, stats.bytes), (3, 1, 200_005));
        assert_eq!(stats.dirs, 3);

        let out = d.path().join("out");
        export_tree(&s, &tree, &out).unwrap();
        assert_eq!(fs::read(out.join("a.txt")).unwrap(), b"hello");
        assert_eq!(fs::read(out.join("sub/b.bin")).unwrap(), vec![7u8; 200_000]);
        assert!(fs::read(out.join("sub/deeper/zero")).unwrap().is_empty());
        assert!(out.join("empty").is_dir());
        assert_eq!(fs::read_link(out.join("link")).unwrap(), Path::new("a.txt"));
        let mode = fs::metadata(out.join("a.txt"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & PERM_MASK, 0o600);

        // Re-importing the export gives the same content. Symlink mtimes cannot be restored
        // with std (no lutimes), so compare structurally: `diff` ignores mtime by design.
        let (again, _) = import_dir(&s, &out, &Chunker::default()).unwrap();
        assert!(diff_trees(&s, Some(&tree), Some(&again))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn identical_files_share_blocks() {
        let (d, s) = store();
        let src = d.path().join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("one"), vec![1u8; 300_000]).unwrap();
        fs::write(src.join("two"), vec![1u8; 300_000]).unwrap();
        import_dir(&s, &src, &Chunker::default()).unwrap();
        let st = s.stats();
        assert!(st.dedup_hits >= 5, "{st:?}");
    }

    #[test]
    fn export_refuses_a_non_empty_destination() {
        let (d, s) = store();
        let src = d.path().join("src");
        fs::create_dir(&src).unwrap();
        let (tree, _) = import_dir(&s, &src, &Chunker::default()).unwrap();
        let dest = d.path().join("busy");
        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("x"), "x").unwrap();
        assert!(export_tree(&s, &tree, &dest).is_err());
    }

    #[test]
    fn imported_content_is_readable_through_treeops() {
        let (d, s) = store();
        let src = d.path().join("src");
        fs::create_dir(&src).unwrap();
        sample(&src);
        let (tree, _) = import_dir(&s, &src, &Chunker::cdc(8192)).unwrap();
        let entry = treeops::lookup_path(&s, &tree, "sub/b.bin")
            .unwrap()
            .unwrap();
        assert_eq!(entry.size, 200_000);
        assert_eq!(
            treeops::read_file(&s, &entry.hash).unwrap(),
            vec![7u8; 200_000]
        );
        assert!(treeops::lookup_path(&s, &tree, "sub/missing")
            .unwrap()
            .is_none());
        assert!(treeops::lookup_path(&s, &tree, "a.txt/x")
            .unwrap()
            .is_none());
        let mut paths = Vec::new();
        treeops::walk(&s, &tree, &mut |p, _| {
            paths.push(p.to_string());
            Ok(())
        })
        .unwrap();
        assert_eq!(
            paths,
            [
                "a.txt",
                "empty",
                "link",
                "sub",
                "sub/b.bin",
                "sub/deeper",
                "sub/deeper/zero"
            ]
        );
    }
}
