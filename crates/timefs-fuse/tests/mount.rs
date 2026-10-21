//! End-to-end tests against a real FUSE mount.

mod common;

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{symlink, PermissionsExt};

use common::Env;
use timefs_core::hostfs::export_tree;
use timefs_core::treeops;
use timefs_fuse::control::request;

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

#[test]
fn basic_file_and_directory_operations() {
    let env = Env::new();
    let m = mount_or_skip!(env);
    let root = &env.mnt;

    fs::write(root.join("hello.txt"), "hello world").unwrap();
    assert_eq!(
        fs::read_to_string(root.join("hello.txt")).unwrap(),
        "hello world"
    );

    fs::create_dir_all(root.join("a/b/c")).unwrap();
    fs::write(root.join("a/b/c/deep.txt"), "deep").unwrap();
    fs::rename(root.join("a/b/c/deep.txt"), root.join("a/moved.txt")).unwrap();
    assert!(!root.join("a/b/c/deep.txt").exists());
    assert_eq!(
        fs::read_to_string(root.join("a/moved.txt")).unwrap(),
        "deep"
    );

    symlink("hello.txt", root.join("link")).unwrap();
    assert_eq!(
        fs::read_link(root.join("link")).unwrap(),
        std::path::Path::new("hello.txt")
    );
    assert_eq!(
        fs::read_to_string(root.join("link")).unwrap(),
        "hello world"
    );

    fs::set_permissions(root.join("hello.txt"), fs::Permissions::from_mode(0o600)).unwrap();
    let mode = fs::metadata(root.join("hello.txt"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o7777, 0o600);

    let mut names: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["a", "hello.txt", "link"]);

    assert!(
        fs::remove_dir(root.join("a")).is_err(),
        "non-empty directory"
    );
    fs::remove_file(root.join("a/moved.txt")).unwrap();
    fs::remove_dir_all(root.join("a")).unwrap();
    assert!(!root.join("a").exists());

    m.unmount().unwrap();
}

#[test]
fn partial_writes_truncate_and_append() {
    let env = Env::new();
    let m = mount_or_skip!(env);
    let path = env.mnt.join("data.bin");

    let mut f = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    f.write_all(&pattern(300_000, 1)).unwrap();
    f.seek(SeekFrom::Start(1000)).unwrap();
    f.write_all(b"PATCH").unwrap();
    f.set_len(200_000).unwrap();
    drop(f);

    let mut expect = pattern(300_000, 1);
    expect[1000..1005].copy_from_slice(b"PATCH");
    expect.truncate(200_000);
    assert_eq!(fs::read(&path).unwrap(), expect);

    let mut f = OpenOptions::new().append(true).open(&path).unwrap();
    f.write_all(b"tail").unwrap();
    drop(f);
    expect.extend_from_slice(b"tail");
    assert_eq!(fs::read(&path).unwrap(), expect);

    // sparse: seek far past the end and write
    let sparse = env.mnt.join("sparse");
    let mut f = File::create(&sparse).unwrap();
    f.seek(SeekFrom::Start(1_000_000)).unwrap();
    f.write_all(b"x").unwrap();
    drop(f);
    let got = fs::read(&sparse).unwrap();
    assert_eq!(got.len(), 1_000_001);
    assert!(got[..1_000_000].iter().all(|&b| b == 0));
    m.unmount().unwrap();
}

#[test]
fn snapshot_through_the_control_socket_and_time_travel() {
    let env = Env::new();
    let m = mount_or_skip!(env);
    fs::write(env.mnt.join("doc.txt"), "version 1").unwrap();
    fs::create_dir(env.mnt.join("dir")).unwrap();
    fs::write(env.mnt.join("dir/keep"), "same").unwrap();

    let reply = request(&env.socket(), "snap first").unwrap();
    assert!(reply.starts_with("ok created"), "{reply}");
    assert!(request(&env.socket(), "snap again")
        .unwrap()
        .starts_with("ok unchanged"));

    fs::write(env.mnt.join("doc.txt"), "version 2, longer").unwrap();
    fs::remove_file(env.mnt.join("dir/keep")).unwrap();
    assert!(request(&env.socket(), "snap second")
        .unwrap()
        .starts_with("ok created"));
    let status = request(&env.socket(), "status").unwrap();
    assert!(
        status.contains("branch main") && status.contains("dirty false"),
        "{status}"
    );
    m.unmount().unwrap();

    let repo = env.open_repo();
    let head = repo.resolve("HEAD").unwrap();
    let old = repo.resolve("HEAD~1").unwrap();
    let store = repo.store().as_ref();
    let read = |commit, path: &str| {
        let tree = repo.get_commit(&commit).unwrap().tree;
        treeops::lookup_path(store, &tree, path)
            .unwrap()
            .map(|e| treeops::read_file(store, &e.hash).unwrap())
    };
    assert_eq!(read(old, "doc.txt").unwrap(), b"version 1");
    assert_eq!(read(head, "doc.txt").unwrap(), b"version 2, longer");
    assert_eq!(read(old, "dir/keep").unwrap(), b"same");
    assert!(read(head, "dir/keep").is_none());
    assert_eq!(repo.log(&head, 10).unwrap().len(), 2);

    let out = env.dir.path().join("export");
    let tree = repo.get_commit(&old).unwrap().tree;
    export_tree(store, &tree, &out).unwrap();
    assert_eq!(
        fs::read_to_string(out.join("doc.txt")).unwrap(),
        "version 1"
    );
}

#[test]
fn state_survives_unmount_and_remount() {
    let env = Env::new();
    let big = pattern(2_500_000, 9);
    {
        let m = mount_or_skip!(env);
        fs::write(env.mnt.join("big"), &big).unwrap();
        fs::create_dir(env.mnt.join("d")).unwrap();
        fs::write(env.mnt.join("d/small"), "s").unwrap();
        m.unmount().unwrap(); // final checkpoint, no explicit snapshot
    }
    let m = mount_or_skip!(env);
    assert!(m.recovery.restored_checkpoint);
    assert_eq!(fs::read(env.mnt.join("big")).unwrap(), big);
    assert_eq!(fs::read_to_string(env.mnt.join("d/small")).unwrap(), "s");
    m.unmount().unwrap();
}

#[test]
fn fsynced_data_survives_a_simulated_crash() {
    let env = Env::new();
    let cfg = timefs_fuse::MountConfig {
        final_checkpoint: false,
        ..Env::config()
    };
    {
        let m = mount_or_skip!(env, &cfg);
        let mut f = File::create(env.mnt.join("durable")).unwrap();
        f.write_all(b"fsynced").unwrap();
        f.sync_all().unwrap(); // checkpoint
        let mut g = File::create(env.mnt.join("volatile")).unwrap();
        g.write_all(b"not synced").unwrap();
        drop((f, g));
        m.unmount().unwrap(); // no final checkpoint: unsynced data is lost
    }
    let m = mount_or_skip!(env);
    assert!(m.recovery.restored_checkpoint);
    assert_eq!(
        fs::read_to_string(env.mnt.join("durable")).unwrap(),
        "fsynced"
    );
    assert!(!env.mnt.join("volatile").exists());
    m.unmount().unwrap();
}

#[test]
fn unlinked_open_file_remains_readable() {
    let env = Env::new();
    let m = mount_or_skip!(env);
    let path = env.mnt.join("temp");
    fs::write(&path, "still readable").unwrap();
    let mut f = File::open(&path).unwrap();
    fs::remove_file(&path).unwrap();
    let mut s = String::new();
    f.read_to_string(&mut s).unwrap();
    assert_eq!(s, "still readable");
    drop(f);
    m.unmount().unwrap();
}

#[test]
fn a_repository_can_only_be_mounted_once() {
    let env = Env::new();
    let m = mount_or_skip!(env);
    let second = env.dir.path().join("mnt2");
    fs::create_dir(&second).unwrap();
    let err = timefs_fuse::mount(env.open_repo(), &second, &Env::config())
        .err()
        .unwrap();
    assert!(err.to_string().contains("locked"), "{err}");
    m.unmount().unwrap();
}

#[test]
fn statfs_reports_the_host_volume() {
    let env = Env::new();
    let m = mount_or_skip!(env);
    let out = std::process::Command::new("df")
        .arg("-P")
        .arg(&env.mnt)
        .output()
        .unwrap();
    assert!(out.status.success());
    m.unmount().unwrap();
}

#[test]
fn a_terabyte_sparse_file_costs_almost_nothing() {
    let env = Env::new();
    let path = env.mnt.join("sparse.img");
    {
        let m = mount_or_skip!(env);
        let f = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)
            .unwrap();
        f.set_len(1 << 40).unwrap();
        std::os::unix::fs::FileExt::write_all_at(&f, b"needle", (1 << 39) + 3).unwrap();
        f.sync_all().unwrap();
        drop(f);
        m.unmount().unwrap();
    }
    let used: u64 = walk_size(&env.repo);
    assert!(
        used < 1 << 20,
        "repository grew by {used} bytes for a sparse file"
    );

    let m = mount_or_skip!(env);
    assert_eq!(fs::metadata(&path).unwrap().len(), 1 << 40);
    let f = File::open(&path).unwrap();
    let mut buf = [0u8; 10];
    std::os::unix::fs::FileExt::read_exact_at(&f, &mut buf, (1 << 39) + 1).unwrap();
    assert_eq!(&buf, b"\0\0needle\0\0");
    drop(f);
    m.unmount().unwrap();
}

/// Bytes of all files below `dir`.
fn walk_size(dir: &std::path::Path) -> u64 {
    fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap())
        .map(|e| {
            let meta = e.metadata().unwrap();
            if meta.is_dir() {
                walk_size(&e.path())
            } else {
                meta.len()
            }
        })
        .sum()
}

#[test]
fn branches_can_be_created_through_the_mount_while_it_holds_the_writer_lock() {
    let env = Env::new();
    let m = mount_or_skip!(env);
    fs::write(env.mnt.join("f"), "data").unwrap();
    assert!(request(&env.socket(), "snap base")
        .unwrap()
        .starts_with("ok created"));

    // A second process cannot write to the pack log while the mount owns it...
    let other = env.open_repo();
    let tip = other.resolve("main").unwrap();
    let err = other.create_branch("direct", &tip).unwrap_err();
    assert!(matches!(err, timefs_core::Error::Locked), "{err}");
    // ...so it asks the mount to do it.
    let reply = request(&env.socket(), &format!("branch feature {tip}")).unwrap();
    assert!(reply.starts_with("ok created branch feature"), "{reply}");
    assert!(request(&env.socket(), &format!("branch feature {tip}"))
        .unwrap()
        .starts_with("err"));
    m.unmount().unwrap();

    let repo = env.open_repo();
    assert_eq!(repo.branch_tip("feature").unwrap(), Some(tip));
}
