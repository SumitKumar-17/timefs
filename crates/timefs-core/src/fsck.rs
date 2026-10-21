//! Repository integrity check.

use crate::error::Result;
use crate::hash::Hash;
use crate::object::ObjectKind;
use crate::reach::mark_reachable;
use crate::refs;
use crate::repo::Repository;

#[derive(Debug, Default)]
pub struct FsckReport {
    pub objects_checked: u64,
    pub bytes_checked: u64,
    /// Objects whose content does not match their name, or that cannot be decoded.
    pub corrupt: Vec<(Hash, String)>,
    /// Objects referenced by a ref or another object but absent.
    pub missing: Vec<(Hash, String)>,
    /// Refs that cannot be read or point at a non-commit.
    pub bad_refs: Vec<String>,
    /// Intact objects not reachable from any ref (reclaimable by `gc`).
    pub unreachable: u64,
}

impl FsckReport {
    pub fn is_clean(&self) -> bool {
        self.corrupt.is_empty() && self.missing.is_empty() && self.bad_refs.is_empty()
    }
}

pub fn fsck(repo: &Repository) -> Result<FsckReport> {
    let store = repo.store().as_ref();
    let mut report = FsckReport::default();

    let all = store.list()?;
    for info in &all {
        report.objects_checked += 1;
        report.bytes_checked += info.size;
        match store.get(&info.hash) {
            Ok((kind, payload)) => {
                if Hash::of_object(kind, &payload) != info.hash {
                    report
                        .corrupt
                        .push((info.hash, "content does not match its name".into()));
                }
            }
            Err(e) => report.corrupt.push((info.hash, e.to_string())),
        }
    }

    let roots = collect_roots(repo, &mut report)?;
    let reach = mark_reachable(store, &roots)?;
    report.missing = reach.missing;
    report.corrupt.extend(reach.undecodable);
    report.unreachable = all
        .iter()
        .filter(|o| !reach.objects.contains(&o.hash))
        .count() as u64;
    Ok(report)
}

/// Every ref tip that is a valid commit; bad refs are recorded in the report.
pub fn collect_roots(repo: &Repository, report: &mut FsckReport) -> Result<Vec<Hash>> {
    let mut roots = Vec::new();
    for prefix in [refs::HEADS, refs::WIP] {
        let listed = match repo.refs().list(prefix) {
            Ok(l) => l,
            Err(e) => {
                report.bad_refs.push(format!("{prefix}: {e}"));
                continue;
            }
        };
        for (name, hash) in listed {
            match repo.store().get(&hash) {
                Ok((ObjectKind::Commit, _)) => roots.push(hash),
                Ok((kind, _)) => report
                    .bad_refs
                    .push(format!("{prefix}/{name} points at a {}", kind.name())),
                // a missing commit is reported through mark_reachable
                Err(_) => roots.push(hash),
            }
        }
    }
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Chunker;
    use crate::hostfs::import_dir;
    use crate::repo::StoreKind;
    use crate::store::ObjectStoreExt;
    use std::fs;

    fn repo_with_commit() -> (tempfile::TempDir, Repository, Hash) {
        repo_with_commit_in(StoreKind::Pack)
    }

    fn repo_with_commit_in(kind: StoreKind) -> (tempfile::TempDir, Repository, Hash) {
        let d = tempfile::tempdir().unwrap();
        let repo = Repository::init_with(d.path().join("repo"), kind).unwrap();
        let src = d.path().join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("f"), vec![5u8; 150_000]).unwrap();
        let (tree, _) = import_dir(repo.store().as_ref(), &src, &Chunker::default()).unwrap();
        let c = repo.commit_to_branch("main", tree, "t", "first").unwrap();
        (d, repo, c)
    }

    #[test]
    fn healthy_repository_is_clean() {
        let (_d, repo, _) = repo_with_commit();
        let r = fsck(&repo).unwrap();
        assert!(r.is_clean(), "{r:?}");
        assert_eq!(r.unreachable, 0);
        assert!(r.objects_checked >= 5);
    }

    #[test]
    fn flipped_byte_in_a_loose_object_is_reported_as_corrupt() {
        let (d, repo, _) = repo_with_commit_in(StoreKind::Loose);
        let victim = repo.store().list().unwrap()[0].hash;
        let hex = victim.to_hex();
        let path = d
            .path()
            .join("repo/objects")
            .join(&hex[..2])
            .join(&hex[2..]);
        let mut bytes = fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(&path, bytes).unwrap();
        let r = fsck(&repo).unwrap();
        assert!(r.corrupt.iter().any(|(h, _)| *h == victim));
        assert!(!r.is_clean());
    }

    #[test]
    fn deleted_loose_object_is_reported_as_missing() {
        let (_d, repo, _) = repo_with_commit_in(StoreKind::Loose);
        let blob = repo
            .store()
            .list()
            .unwrap()
            .into_iter()
            .find(|o| o.size > 60_000)
            .unwrap()
            .hash;
        repo.store().delete(&blob).unwrap();
        let r = fsck(&repo).unwrap();
        assert!(r.missing.iter().any(|(h, _)| *h == blob), "{r:?}");
    }

    #[test]
    fn orphan_objects_count_as_unreachable_not_errors() {
        let (_d, repo, _) = repo_with_commit();
        repo.store().put_blob(b"orphan").unwrap();
        let r = fsck(&repo).unwrap();
        assert!(r.is_clean());
        assert_eq!(r.unreachable, 1);
    }

    #[test]
    fn flipped_byte_in_durable_pack_data_is_reported_as_corrupt() {
        let (d, repo, _) = repo_with_commit();
        // A second commit makes the first one's data provably durable (a sync marker
        // follows it in the log).
        let tree = repo
            .get_commit(&repo.resolve("HEAD").unwrap())
            .unwrap()
            .tree;
        repo.commit_to_branch("main", tree, "t", "second").unwrap();
        drop(repo);
        let segment = d.path().join("repo/pack/seg-00000001.log");
        let mut bytes = fs::read(&segment).unwrap();
        bytes[100] ^= 1; // inside the first object's data
        fs::write(&segment, bytes).unwrap();
        let err = Repository::open(d.path().join("repo")).err().unwrap();
        assert!(err.to_string().contains("already durable"), "{err}");
    }
}
