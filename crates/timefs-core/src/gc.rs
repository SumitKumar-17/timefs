//! Stop-the-world mark-and-sweep garbage collection (ADR-0008).
//!
//! Marks every object reachable from any ref, then deletes unreachable objects that are
//! older than a grace period. Requires the exclusive repository lock, which is why the
//! caller must pass a `&RepoLock`.

use std::time::Duration;

use crate::error::{Error, Result};
use crate::fsck::{collect_roots, FsckReport};
use crate::reach::mark_reachable;
use crate::repo::{RepoLock, Repository};

#[derive(Clone, Copy, Debug)]
pub struct GcOptions {
    /// Unreachable objects younger than this are kept (they may belong to an operation
    /// that was interrupted a moment ago).
    pub grace: Duration,
    /// Report what would be deleted without deleting anything.
    pub dry_run: bool,
}

impl Default for GcOptions {
    fn default() -> Self {
        GcOptions {
            grace: Duration::from_secs(3600),
            dry_run: false,
        }
    }
}

/// What a collection did; the same counters every store backend reports.
pub use crate::store::SweepReport as GcReport;

pub fn gc(repo: &Repository, _lock: &RepoLock, opts: GcOptions) -> Result<GcReport> {
    let store = repo.store().as_ref();

    // Never collect from a damaged repository: a missing ref target would make live data
    // look unreachable.
    let mut problems = FsckReport::default();
    let roots = collect_roots(repo, &mut problems)?;
    if !problems.bad_refs.is_empty() {
        return Err(Error::Invalid(format!(
            "refusing to collect: {} (run fsck)",
            problems.bad_refs[0]
        )));
    }
    let reach = mark_reachable(store, &roots)?;
    if let Some((hash, by)) = reach.missing.first() {
        return Err(Error::Invalid(format!(
            "refusing to collect: object {hash} (from {by}) is missing (run fsck)"
        )));
    }
    if let Some((hash, why)) = reach.undecodable.first() {
        return Err(Error::Invalid(format!(
            "refusing to collect: object {hash} is unreadable: {why} (run fsck)"
        )));
    }

    store.sweep(&reach.objects, opts.grace, opts.dry_run)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Chunker;
    use crate::fsck::fsck;
    use crate::hostfs::import_dir;
    use crate::store::ObjectStoreExt;
    use std::fs;

    const NOW: GcOptions = GcOptions {
        grace: Duration::ZERO,
        dry_run: false,
    };

    fn commit_dir(repo: &Repository, dir: &std::path::Path, msg: &str) -> crate::Hash {
        let (tree, _) = import_dir(repo.store().as_ref(), dir, &Chunker::default()).unwrap();
        repo.commit_to_branch("main", tree, "t", msg).unwrap()
    }

    fn setup() -> (tempfile::TempDir, Repository, std::path::PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let repo = Repository::init(d.path().join("repo")).unwrap();
        let src = d.path().join("src");
        fs::create_dir(&src).unwrap();
        (d, repo, src)
    }

    #[test]
    fn collects_history_that_is_no_longer_referenced() {
        let (_d, repo, src) = setup();
        fs::write(src.join("keep"), vec![1u8; 100_000]).unwrap();
        let first = commit_dir(&repo, &src, "one");
        fs::write(src.join("big"), vec![9u8; 300_000]).unwrap();
        commit_dir(&repo, &src, "two");
        // roll the branch back: the second commit and its data become garbage
        repo.set_branch_tip("main", &first).unwrap();

        let lock = repo.lock_exclusive().unwrap();
        let before = repo.store().list().unwrap().len();
        let report = gc(&repo, &lock, NOW).unwrap();
        assert!(report.deleted > 0 && report.bytes_freed > 0, "{report:?}");
        assert_eq!(report.examined as usize, before);
        assert_eq!(report.examined, report.reachable + report.deleted);
        let after = fsck(&repo).unwrap();
        assert!(after.is_clean(), "{after:?}");
        assert_eq!(after.unreachable, 0);
        // the surviving commit still reads fine
        assert!(repo.get_commit(&first).is_ok());
    }

    #[test]
    fn dry_run_and_grace_period_delete_nothing() {
        let (_d, repo, src) = setup();
        fs::write(src.join("f"), "x").unwrap();
        let first = commit_dir(&repo, &src, "one");
        fs::write(src.join("f"), "y").unwrap();
        commit_dir(&repo, &src, "two");
        repo.set_branch_tip("main", &first).unwrap();
        let lock = repo.lock_exclusive().unwrap();
        let total = repo.store().list().unwrap().len();

        let dry = gc(
            &repo,
            &lock,
            GcOptions {
                dry_run: true,
                ..NOW
            },
        )
        .unwrap();
        assert!(dry.deleted > 0);
        assert_eq!(repo.store().list().unwrap().len(), total);

        let graceful = gc(
            &repo,
            &lock,
            GcOptions {
                grace: Duration::from_secs(3600),
                dry_run: false,
            },
        )
        .unwrap();
        assert_eq!(graceful.deleted, 0);
        assert_eq!(graceful.kept_recent, dry.deleted);
        assert_eq!(repo.store().list().unwrap().len(), total);
    }

    #[test]
    fn checkpoint_refs_keep_their_data_alive() {
        let (_d, repo, src) = setup();
        fs::write(src.join("f"), "wip data").unwrap();
        let (tree, _) = import_dir(repo.store().as_ref(), &src, &Chunker::default()).unwrap();
        let wip = repo.create_commit(tree, vec![], "t", "checkpoint").unwrap();
        repo.refs().write("wip/main", &wip).unwrap();
        let lock = repo.lock_exclusive().unwrap();
        let report = gc(&repo, &lock, NOW).unwrap();
        assert_eq!(report.deleted, 0, "{report:?}");
    }

    #[test]
    fn refuses_to_run_on_a_damaged_repository() {
        let (_d, repo, src) = setup();
        fs::write(src.join("f"), vec![3u8; 100_000]).unwrap();
        commit_dir(&repo, &src, "one");
        let blob = repo
            .store()
            .list()
            .unwrap()
            .into_iter()
            .max_by_key(|o| o.size)
            .unwrap()
            .hash;
        repo.store().delete(&blob).unwrap();
        repo.store().put_blob(b"orphan").unwrap();
        let lock = repo.lock_exclusive().unwrap();
        let err = gc(&repo, &lock, NOW).unwrap_err();
        assert!(err.to_string().contains("missing"), "{err}");
        assert!(
            repo.store().list().unwrap().iter().any(|o| o.size == 7),
            "orphan must survive"
        );
    }

    #[test]
    fn removes_stale_temporary_files() {
        let (d, repo, _src) = setup();
        let tmp = d.path().join("repo/objects/tmp/12345-0");
        fs::write(&tmp, "half written").unwrap();
        let lock = repo.lock_exclusive().unwrap();
        let report = gc(&repo, &lock, NOW).unwrap();
        assert_eq!(report.temp_files_removed, 1);
        assert!(!tmp.exists());
    }
}
