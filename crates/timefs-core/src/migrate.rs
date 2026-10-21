//! Copy a repository into a new one with a different store layout (for example loose to pack).
//!
//! The copy is non-destructive and hash-preserving: objects keep their names, so every commit,
//! tree and ref means exactly the same thing in the new repository.

use std::path::Path;

use crate::error::Result;
use crate::refs;
use crate::repo::{Repository, StoreKind};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MigrateReport {
    pub objects: u64,
    pub bytes: u64,
    pub refs: u64,
}

/// Create a repository at `dest` with layout `kind` holding everything `src` holds.
pub fn copy_repository(src: &Repository, dest: &Path, kind: StoreKind) -> Result<MigrateReport> {
    let out = Repository::init_with(dest, kind)?;
    let mut report = MigrateReport::default();
    for info in src.store().list()? {
        let (obj_kind, payload) = src.store().get(&info.hash)?;
        out.store().put(obj_kind, &payload)?;
        report.objects += 1;
        report.bytes += payload.len() as u64;
    }
    // Refs last: `Refs::write` makes the copied objects durable before naming them.
    for prefix in [refs::HEADS, refs::WIP] {
        for (name, hash) in src.refs().list(prefix)? {
            out.refs().write(&format!("{prefix}/{name}"), &hash)?;
            report.refs += 1;
        }
    }
    out.store().sync()?;
    out.set_head_branch(&src.head_branch()?)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Chunker;
    use crate::fsck::fsck;
    use crate::hostfs::import_dir;
    use std::fs;

    #[test]
    fn loose_to_pack_and_back_preserves_history() {
        let d = tempfile::tempdir().unwrap();
        let src = Repository::init_with(d.path().join("loose"), StoreKind::Loose).unwrap();
        let data = d.path().join("data");
        fs::create_dir(&data).unwrap();
        fs::write(data.join("a"), vec![1u8; 200_000]).unwrap();
        let (t1, _) = import_dir(src.store().as_ref(), &data, &Chunker::default()).unwrap();
        src.commit_to_branch("main", t1, "t", "one").unwrap();
        fs::write(data.join("b"), "second").unwrap();
        let (t2, _) = import_dir(src.store().as_ref(), &data, &Chunker::default()).unwrap();
        let tip = src.commit_to_branch("main", t2, "t", "two").unwrap();
        src.create_branch("feature", &tip).unwrap();
        src.set_head_branch("feature").unwrap();

        let packed = d.path().join("pack");
        let report = copy_repository(&src, &packed, StoreKind::Pack).unwrap();
        assert_eq!(report.refs, 2);
        assert_eq!(report.objects as usize, src.store().list().unwrap().len());

        let dest = Repository::open(&packed).unwrap();
        assert_eq!(dest.store_kind(), StoreKind::Pack);
        assert_eq!(dest.resolve("main").unwrap(), tip);
        assert_eq!(dest.head_branch().unwrap(), "feature");
        assert_eq!(dest.log(&tip, 10).unwrap().len(), 2);
        assert!(fsck(&dest).unwrap().is_clean());

        let back = d.path().join("loose-again");
        copy_repository(&dest, &back, StoreKind::Loose).unwrap();
        let again = Repository::open(&back).unwrap();
        assert_eq!(again.resolve("feature").unwrap(), tip);
        assert!(fsck(&again).unwrap().is_clean());
    }

    #[test]
    fn refuses_an_existing_destination() {
        let d = tempfile::tempdir().unwrap();
        let src = Repository::init_with(d.path().join("a"), StoreKind::Loose).unwrap();
        Repository::init(d.path().join("b")).unwrap();
        assert!(copy_repository(&src, &d.path().join("b"), StoreKind::Pack).is_err());
    }
}
