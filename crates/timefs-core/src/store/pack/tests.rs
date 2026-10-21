//! Behaviour of the pack store, including crash recovery at every interesting boundary.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::time::Duration;

use super::*;
use crate::store::ObjectStoreExt;

fn dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn open(d: &tempfile::TempDir) -> PackStore {
    PackStore::open(d.path().join("pack")).unwrap()
}

/// Length of the single segment file.
fn segment_len(d: &tempfile::TempDir, id: u32) -> u64 {
    fs::metadata(segment::path(&d.path().join("pack"), id))
        .unwrap()
        .len()
}

fn truncate_segment(d: &tempfile::TempDir, id: u32, len: u64) {
    OpenOptions::new()
        .write(true)
        .open(segment::path(&d.path().join("pack"), id))
        .unwrap()
        .set_len(len)
        .unwrap();
}

#[test]
fn objects_and_refs_persist_across_reopen() {
    let d = dir();
    let (blob, commitish) = {
        let s = open(&d);
        let blob = s.put_blob(b"hello pack").unwrap();
        assert_eq!(s.put_blob(b"hello pack").unwrap(), blob, "deduplicated");
        let other = s.put_blob(b"second").unwrap();
        Refs::write(&s, "heads/main", &other).unwrap();
        Refs::write(&s, "wip/main", &blob).unwrap();
        Refs::delete(&s, "wip/main").unwrap();
        assert_eq!(s.stats().new_objects, 2);
        (blob, other)
    };
    let s = open(&d);
    assert_eq!(s.get_blob(&blob).unwrap(), b"hello pack");
    assert_eq!(Refs::read(&s, "heads/main").unwrap(), Some(commitish));
    assert_eq!(Refs::read(&s, "wip/main").unwrap(), None);
    assert_eq!(
        Refs::list(&s, "heads").unwrap(),
        vec![("main".to_string(), commitish)]
    );
    assert!(matches!(
        Refs::delete(&s, "wip/main"),
        Err(Error::RefNotFound(_))
    ));
    assert!(matches!(
        Refs::create(&s, "heads/main", &blob),
        Err(Error::RefExists(_))
    ));
}

#[test]
fn a_torn_tail_drops_the_damaged_record_and_everything_after_it() {
    let d = dir();
    let (kept, lost) = {
        let s = open(&d);
        let kept = s.put_blob(b"durable").unwrap();
        Refs::write(&s, "heads/a", &kept).unwrap();
        let lost = s.put_blob(&[9u8; 1000]).unwrap();
        Refs::write(&s, "heads/b", &lost).unwrap();
        (kept, lost)
    };
    // Cut inside the second object: its ref record, written later, must disappear too.
    let full = segment_len(&d, 1);
    truncate_segment(&d, 1, full - 600);

    let s = open(&d);
    assert_eq!(s.get_blob(&kept).unwrap(), b"durable");
    assert!(matches!(s.get(&lost), Err(Error::NotFound(_))));
    assert_eq!(Refs::read(&s, "heads/a").unwrap(), Some(kept));
    assert_eq!(
        Refs::read(&s, "heads/b").unwrap(),
        None,
        "a ref must never survive an object it names"
    );

    // The next writer trims the garbage and appends after the valid prefix.
    let again = s.put_blob(b"after recovery").unwrap();
    s.sync().unwrap();
    drop(s);
    let s = open(&d);
    assert_eq!(s.get_blob(&again).unwrap(), b"after recovery");
    assert_eq!(s.get_blob(&kept).unwrap(), b"durable");
}

#[test]
fn only_one_process_may_write_but_others_can_read() {
    let d = dir();
    let first = open(&d);
    let h = first.put_blob(b"from the writer").unwrap();
    first.sync().unwrap();

    let second = open(&d);
    assert_eq!(second.get_blob(&h).unwrap(), b"from the writer");
    assert!(matches!(second.put_blob(b"x"), Err(Error::Locked)));
    assert!(matches!(
        Refs::write(&second, "heads/main", &h),
        Err(Error::Locked)
    ));
    drop(first);
    assert!(
        second.put_blob(b"x").is_ok(),
        "the lock is released with the writer"
    );
}

#[test]
fn segments_rotate_and_stay_readable() {
    let d = dir();
    let hashes: Vec<Hash> = {
        let s = open(&d).with_segment_limit(4096);
        let hashes = (0..50u8).map(|i| s.put_blob(&[i; 300]).unwrap()).collect();
        s.sync().unwrap();
        assert!(s.segment_count() > 3, "small limit must rotate");
        hashes
    };
    let s = open(&d);
    for (i, h) in hashes.iter().enumerate() {
        assert_eq!(s.get_blob(h).unwrap(), vec![i as u8; 300]);
    }
}

#[test]
fn a_damaged_sealed_segment_is_reported() {
    let d = dir();
    {
        let s = open(&d).with_segment_limit(1024);
        for i in 0..10u8 {
            s.put_blob(&[i; 400]).unwrap();
        }
        s.sync().unwrap();
    }
    truncate_segment(&d, 1, 10);
    assert!(matches!(
        PackStore::open(d.path().join("pack")),
        Err(Error::Corrupt(_))
    ));
}

#[test]
fn reads_verify_content() {
    let d = dir();
    let s = open(&d);
    let h = s.put_blob(b"precious").unwrap();
    s.sync().unwrap();
    let path = segment::path(&d.path().join("pack"), 1);
    let mut bytes = fs::read(&path).unwrap();
    let at = (HEADER_LEN + OBJECT_PREFIX) as usize;
    bytes[at] ^= 0xFF;
    fs::write(&path, bytes).unwrap();
    assert!(matches!(s.get(&h), Err(Error::HashMismatch { .. })));
}

#[test]
fn compaction_keeps_live_data_and_refs_and_frees_the_rest() {
    let d = dir();
    let s = open(&d).with_segment_limit(2048);
    let live = s.put_blob(&[1u8; 500]).unwrap();
    let dead: Vec<Hash> = (2..8u8).map(|i| s.put_blob(&[i; 500]).unwrap()).collect();
    Refs::write(&s, "heads/main", &live).unwrap();
    let keep: HashSet<Hash> = [live].into_iter().collect();

    let dry = s.sweep(&keep, Duration::ZERO, true).unwrap();
    assert_eq!((dry.examined, dry.reachable, dry.deleted), (7, 1, 6));
    assert!(s.has(&dead[0]).unwrap(), "a dry run changes nothing");

    let before = s.segment_count();
    let report = s.sweep(&keep, Duration::ZERO, false).unwrap();
    assert_eq!(report.deleted, 6);
    assert!(s.segment_count() < before);
    assert_eq!(s.get_blob(&live).unwrap(), vec![1u8; 500]);
    assert!(matches!(s.get(&dead[0]), Err(Error::NotFound(_))));
    assert_eq!(Refs::read(&s, "heads/main").unwrap(), Some(live));

    // still consistent after reopening, and writable
    drop(s);
    let s = open(&d);
    assert_eq!(s.get_blob(&live).unwrap(), vec![1u8; 500]);
    assert_eq!(ObjectStore::list(&s).unwrap().len(), 1);
    assert_eq!(Refs::read(&s, "heads/main").unwrap(), Some(live));
    s.put_blob(b"new").unwrap();
}

#[test]
fn individual_deletes_are_refused() {
    let d = dir();
    let s = open(&d);
    let h = s.put_blob(b"x").unwrap();
    assert!(ObjectStore::delete(&s, &h).is_err());
    assert!(s.has(&h).unwrap());
}

#[test]
fn list_and_prefix_lookup() {
    let d = dir();
    let s = open(&d);
    let hashes: Vec<Hash> = (0..20u8).map(|i| s.put_blob(&[i]).unwrap()).collect();
    assert_eq!(ObjectStore::list(&s).unwrap().len(), 20);
    let target = hashes[7];
    assert_eq!(s.list_prefix(&target.to_hex()[..10]).unwrap(), vec![target]);
    assert!(s.list_prefix("a").is_err());
}
