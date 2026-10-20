//! Behavioural unit tests for the working tree.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use super::*;
use crate::store::{LooseStore, ObjectStore};

const BLOCK: usize = 16;

struct Fixture {
    _dir: tempfile::TempDir,
    store: Arc<LooseStore>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(LooseStore::open(dir.path().join("objects")).unwrap());
        Fixture { _dir: dir, store }
    }

    fn open(&self, root: Option<Hash>) -> WorkTree {
        let ticks = AtomicI64::new(1_000);
        let opts = Options {
            block_size: BLOCK,
            ..Options::default()
        };
        WorkTree::new(self.store.clone() as Arc<dyn ObjectStore>, root, opts)
            .with_clock(Box::new(move || ticks.fetch_add(1, Ordering::Relaxed)))
    }
}

fn write_file(wt: &mut WorkTree, parent: Ino, name: &str, data: &[u8]) -> Ino {
    let attr = wt
        .create(parent, name.as_bytes(), 0o644, 1000, 1000)
        .unwrap();
    wt.write(attr.ino, 0, data).unwrap();
    attr.ino
}

fn read_all(wt: &mut WorkTree, ino: Ino) -> Vec<u8> {
    wt.read(ino, 0, 1 << 20).unwrap()
}

#[test]
fn create_write_read_and_readdir_in_name_order() {
    let fx = Fixture::new();
    let mut wt = fx.open(None);
    let f = write_file(&mut wt, ROOT_INO, "zeta", b"hello world, spanning blocks");
    write_file(&mut wt, ROOT_INO, "alpha", b"x");
    wt.mkdir(ROOT_INO, b"mid", 0o755, 0, 0).unwrap();
    assert_eq!(read_all(&mut wt, f), b"hello world, spanning blocks");
    assert_eq!(wt.getattr(f).unwrap().size, 28);
    let names: Vec<_> = wt
        .readdir(ROOT_INO)
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(
        names,
        [b"alpha".to_vec(), b"mid".to_vec(), b"zeta".to_vec()]
    );
}

#[test]
fn error_cases() {
    let fx = Fixture::new();
    let mut wt = fx.open(None);
    let f = write_file(&mut wt, ROOT_INO, "f", b"data");
    let d = wt.mkdir(ROOT_INO, b"d", 0o755, 0, 0).unwrap().ino;
    assert!(matches!(
        wt.lookup(ROOT_INO, b"nope"),
        Err(FsError::NotFound)
    ));
    assert!(matches!(
        wt.create(ROOT_INO, b"f", 0o644, 0, 0),
        Err(FsError::Exists)
    ));
    assert!(matches!(
        wt.mkdir(ROOT_INO, b"d", 0o755, 0, 0),
        Err(FsError::Exists)
    ));
    assert!(matches!(wt.lookup(f, b"x"), Err(FsError::NotDir)));
    assert!(matches!(wt.read(d, 0, 10), Err(FsError::IsDir)));
    assert!(matches!(wt.unlink(ROOT_INO, b"d"), Err(FsError::IsDir)));
    assert!(matches!(wt.rmdir(ROOT_INO, b"f"), Err(FsError::NotDir)));
    let long = vec![b'x'; 300];
    assert!(matches!(
        wt.create(ROOT_INO, &long, 0o644, 0, 0),
        Err(FsError::NameTooLong)
    ));
    assert!(matches!(
        wt.create(ROOT_INO, b"a/b", 0o644, 0, 0),
        Err(FsError::InvalidArgument(_))
    ));
    write_file(&mut wt, d, "inner", b"1");
    assert!(matches!(wt.rmdir(ROOT_INO, b"d"), Err(FsError::NotEmpty)));
}

#[test]
fn rename_follows_posix_rules() {
    let fx = Fixture::new();
    let mut wt = fx.open(None);
    let a = write_file(&mut wt, ROOT_INO, "a", b"A");
    write_file(&mut wt, ROOT_INO, "b", b"B");
    let dir = wt.mkdir(ROOT_INO, b"dir", 0o755, 0, 0).unwrap().ino;
    let sub = wt.mkdir(dir, b"sub", 0o755, 0, 0).unwrap().ino;
    let empty = wt.mkdir(ROOT_INO, b"empty", 0o755, 0, 0).unwrap().ino;

    wt.rename(ROOT_INO, b"a", ROOT_INO, b"b", false).unwrap(); // file over file
    assert!(matches!(wt.lookup(ROOT_INO, b"a"), Err(FsError::NotFound)));
    let b = wt.lookup(ROOT_INO, b"b").unwrap().ino;
    assert_eq!(b, a);
    assert_eq!(read_all(&mut wt, b), b"A");

    assert!(matches!(
        wt.rename(ROOT_INO, b"b", ROOT_INO, b"dir", false),
        Err(FsError::IsDir)
    ));
    assert!(matches!(
        wt.rename(ROOT_INO, b"dir", ROOT_INO, b"b", false),
        Err(FsError::NotDir)
    ));
    assert!(matches!(
        wt.rename(ROOT_INO, b"empty", ROOT_INO, b"dir", false),
        Err(FsError::NotEmpty)
    ));
    assert!(matches!(
        wt.rename(ROOT_INO, b"dir", dir, b"x", false),
        Err(FsError::InvalidArgument(_))
    ));
    assert!(matches!(
        wt.rename(ROOT_INO, b"dir", sub, b"x", false),
        Err(FsError::InvalidArgument(_))
    ));
    assert!(matches!(
        wt.rename(ROOT_INO, b"b", ROOT_INO, b"dir", true),
        Err(FsError::Exists)
    ));
    wt.rename(ROOT_INO, b"dir", empty, b"moved", false).unwrap(); // dir into another dir
    assert_eq!(wt.lookup(empty, b"moved").unwrap().ino, dir);
    wt.rename(ROOT_INO, b"b", ROOT_INO, b"b", false).unwrap(); // onto itself
    assert!(wt.lookup(ROOT_INO, b"b").is_ok());
}

#[test]
fn unlinked_file_stays_usable_until_released() {
    let fx = Fixture::new();
    let mut wt = fx.open(None);
    let f = write_file(&mut wt, ROOT_INO, "tmp", b"still here");
    wt.open(f).unwrap();
    wt.unlink(ROOT_INO, b"tmp").unwrap();
    assert!(matches!(
        wt.lookup(ROOT_INO, b"tmp"),
        Err(FsError::NotFound)
    ));
    assert_eq!(read_all(&mut wt, f), b"still here");
    let with_node = wt.node_count();
    wt.release(f);
    wt.forget(f, 1);
    assert_eq!(
        wt.node_count(),
        with_node - 1,
        "inode freed after last reference"
    );
    assert!(matches!(wt.getattr(f), Err(FsError::NotFound)));
    assert_eq!(wt.dirty_bytes(), 0);
}

#[test]
fn checkpoint_and_reload_round_trip() {
    let fx = Fixture::new();
    let mut wt = fx.open(None);
    let big = vec![42u8; 100];
    write_file(&mut wt, ROOT_INO, "big", &big);
    let d = wt.mkdir(ROOT_INO, b"d", 0o700, 5, 6).unwrap().ino;
    write_file(&mut wt, d, "small", b"s");
    wt.symlink(ROOT_INO, b"ln", b"d/small", 1, 2).unwrap();
    assert!(wt.is_dirty());
    assert!(wt.dirty_bytes() >= 101);
    let root = wt.checkpoint().unwrap();
    assert!(!wt.is_dirty());
    assert_eq!(wt.dirty_bytes(), 0);

    let mut again = fx.open(Some(root));
    let big_ino = again.lookup(ROOT_INO, b"big").unwrap();
    assert_eq!(big_ino.size, 100);
    assert_eq!(read_all(&mut again, big_ino.ino), big);
    let dir = again.lookup(ROOT_INO, b"d").unwrap();
    assert_eq!((dir.mode, dir.uid, dir.gid), (0o700, 5, 6));
    let small = again.lookup(dir.ino, b"small").unwrap();
    assert_eq!(read_all(&mut again, small.ino), b"s");
    let link = again.lookup(ROOT_INO, b"ln").unwrap();
    assert_eq!(again.readlink(link.ino).unwrap(), b"d/small");
    assert!(!again.is_dirty(), "reading must not dirty the tree");
}

#[test]
fn unchanged_checkpoint_is_stable_and_free() {
    let fx = Fixture::new();
    let mut wt = fx.open(None);
    write_file(&mut wt, ROOT_INO, "f", b"abc");
    let first = wt.checkpoint().unwrap();
    let objects = fx.store.stats().new_objects;
    assert_eq!(wt.checkpoint().unwrap(), first);
    assert_eq!(fx.store.stats().new_objects, objects);
}

#[test]
fn editing_one_file_rewrites_only_its_path() {
    let fx = Fixture::new();
    let mut wt = fx.open(None);
    let mut dirs = Vec::new();
    for i in 0..10 {
        let d = wt
            .mkdir(ROOT_INO, format!("d{i}").as_bytes(), 0o755, 0, 0)
            .unwrap()
            .ino;
        for j in 0..10 {
            write_file(&mut wt, d, &format!("f{j}"), &[(i * 10 + j) as u8; 40]);
        }
        dirs.push(d);
    }
    let base = wt.checkpoint().unwrap();

    let mut wt = fx.open(Some(base));
    let d3 = wt.lookup(ROOT_INO, b"d3").unwrap().ino;
    let f7 = wt.lookup(d3, b"f7").unwrap().ino;
    let before = fx.store.stats().new_objects;
    wt.write(f7, 0, b"X").unwrap();
    let edited = wt.checkpoint().unwrap();
    let created = fx.store.stats().new_objects - before;
    assert_ne!(edited, base);
    // one blob + file object + d3 tree + root tree
    assert_eq!(created, 4, "path copying must rewrite only the edited path");
}

#[test]
fn directories_are_loaded_lazily() {
    let fx = Fixture::new();
    let mut wt = fx.open(None);
    for i in 0..5 {
        let d = wt
            .mkdir(ROOT_INO, format!("d{i}").as_bytes(), 0o755, 0, 0)
            .unwrap()
            .ino;
        for j in 0..20 {
            write_file(&mut wt, d, &format!("f{j}"), b"x");
        }
    }
    let root = wt.checkpoint().unwrap();
    let mut wt = fx.open(Some(root));
    assert_eq!(wt.node_count(), 1);
    wt.lookup(ROOT_INO, b"d2").unwrap();
    assert_eq!(wt.node_count(), 1 + 5, "only the root's entries are loaded");
    let d2 = wt.lookup(ROOT_INO, b"d2").unwrap().ino;
    wt.readdir(d2).unwrap();
    assert_eq!(wt.node_count(), 1 + 5 + 20);
}

#[test]
fn setattr_changes_metadata_and_size() {
    let fx = Fixture::new();
    let mut wt = fx.open(None);
    let f = write_file(&mut wt, ROOT_INO, "f", b"0123456789");
    let a = wt
        .setattr(
            f,
            SetAttr {
                mode: Some(0o100600),
                size: Some(4),
                mtime_ns: Some(7),
                ..SetAttr::default()
            },
        )
        .unwrap();
    assert_eq!((a.mode, a.size, a.mtime_ns), (0o600, 4, 7));
    assert_eq!(read_all(&mut wt, f), b"0123");
    let a = wt
        .setattr(
            f,
            SetAttr {
                size: Some(8),
                ..SetAttr::default()
            },
        )
        .unwrap();
    assert_eq!(a.size, 8);
    assert_eq!(read_all(&mut wt, f), b"0123\0\0\0\0");
    let root = wt.checkpoint().unwrap();
    let mut again = fx.open(Some(root));
    let f2 = again.lookup(ROOT_INO, b"f").unwrap();
    assert_eq!((f2.mode, f2.size), (0o600, 8));
}

#[test]
fn metadata_only_change_is_persisted() {
    let fx = Fixture::new();
    let mut wt = fx.open(None);
    let f = write_file(&mut wt, ROOT_INO, "f", b"data");
    let first = wt.checkpoint().unwrap();
    wt.setattr(
        f,
        SetAttr {
            uid: Some(4242),
            ..SetAttr::default()
        },
    )
    .unwrap();
    let second = wt.checkpoint().unwrap();
    assert_ne!(first, second);
    let mut again = fx.open(Some(second));
    assert_eq!(again.lookup(ROOT_INO, b"f").unwrap().uid, 4242);
}

#[test]
fn checkpointing_an_untouched_empty_tree_works() {
    let fx = Fixture::new();
    let mut wt = fx.open(None);
    let root = wt.checkpoint().unwrap();
    let mut again = fx.open(Some(root));
    assert!(again.readdir(ROOT_INO).unwrap().is_empty());
    assert_eq!(wt.checkpoint().unwrap(), root);
}
