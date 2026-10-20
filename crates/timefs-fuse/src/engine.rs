//! Persistence policy for a mounted branch: checkpoints, snapshots and crash recovery.
//!
//! * checkpoint: persist the working tree and point `refs/wip/<branch>` at a commit
//!   parented on the branch tip. Cheap, frequent, and the source of crash recovery.
//! * snapshot: commit the working tree onto the branch and clear the wip ref.
//!
//! Write ordering invariant: objects are made durable before any ref points at them.

use std::os::unix::fs::MetadataExt;
use std::sync::{Arc, Mutex};

use timefs_core::refs::validate_ref_name;
use timefs_core::repo::{branch_ref, wip_ref};
use timefs_core::store::ObjectStore;
use timefs_core::worktree::{Options, WorkTree};
use timefs_core::{Error, Hash, Repository, Result};

/// What happened when the working tree was opened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Recovery {
    /// State was restored from a checkpoint newer than the branch tip.
    pub restored_checkpoint: bool,
    /// A checkpoint exists but does not descend from the branch tip, so it was ignored.
    pub ignored_stale_checkpoint: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotOutcome {
    Created(Hash),
    /// The working tree equals the branch tip; nothing to commit.
    Unchanged(Hash),
}

pub struct Engine {
    repo: Arc<Repository>,
    branch: String,
    author: String,
    block_size: usize,
    /// Root tree hash most recently written to a ref (branch tip or wip checkpoint).
    last_persisted: Mutex<Option<Hash>>,
}

impl Engine {
    pub fn new(
        repo: Arc<Repository>,
        branch: &str,
        author: &str,
        block_size: usize,
    ) -> Result<Self> {
        validate_ref_name(&branch_ref(branch))?;
        Ok(Engine {
            repo,
            branch: branch.to_string(),
            author: author.to_string(),
            block_size,
            last_persisted: Mutex::new(None),
        })
    }

    pub fn branch(&self) -> &str {
        &self.branch
    }

    pub fn repo(&self) -> &Arc<Repository> {
        &self.repo
    }

    /// Open the working tree for the branch, recovering from a checkpoint when one is valid.
    pub fn open_tree(&self) -> Result<(WorkTree, Recovery)> {
        let tip = self.repo.branch_tip(&self.branch)?;
        let mut recovery = Recovery::default();
        let mut root = None;
        let mut root_time = 0;

        if let Some(wip) = self.repo.refs().read(&wip_ref(&self.branch))? {
            let commit = self.repo.get_commit(&wip)?;
            if commit.parents == tip.into_iter().collect::<Vec<_>>() {
                root = Some(commit.tree);
                root_time = commit.time_ns;
                recovery.restored_checkpoint = true;
            } else {
                recovery.ignored_stale_checkpoint = true;
            }
        }
        if root.is_none() {
            if let Some(tip) = tip {
                let commit = self.repo.get_commit(&tip)?;
                root = Some(commit.tree);
                root_time = commit.time_ns;
            }
        }
        *self.last_persisted.lock().unwrap() = root;

        let meta = std::fs::metadata(self.repo.root())?;
        let opts = Options {
            block_size: self.block_size,
            root_uid: meta.uid(),
            root_gid: meta.gid(),
            root_mtime_ns: root_time,
        };
        let store: Arc<dyn ObjectStore> = Arc::clone(self.repo.store());
        Ok((WorkTree::new(store, root, opts), recovery))
    }

    /// Persist the working tree and record it as the branch's checkpoint. Returns the
    /// checkpoint commit, or `None` when nothing changed since the last persisted state.
    pub fn checkpoint(&self, wt: &mut WorkTree) -> Result<Option<Hash>> {
        let tree = self.persist_tree(wt)?;
        if *self.last_persisted.lock().unwrap() == Some(tree) {
            return Ok(None);
        }
        let parents: Vec<Hash> = self.repo.branch_tip(&self.branch)?.into_iter().collect();
        let commit = self
            .repo
            .create_commit(tree, parents, &self.author, "checkpoint")?;
        self.repo.store().sync()?;
        self.repo.refs().write(&wip_ref(&self.branch), &commit)?;
        *self.last_persisted.lock().unwrap() = Some(tree);
        Ok(Some(commit))
    }

    /// Commit the working tree onto the branch.
    pub fn snapshot(&self, wt: &mut WorkTree, message: &str) -> Result<SnapshotOutcome> {
        let tree = self.persist_tree(wt)?;
        let tip = self.repo.branch_tip(&self.branch)?;
        if let Some(tip) = tip {
            if self.repo.get_commit(&tip)?.tree == tree {
                return Ok(SnapshotOutcome::Unchanged(tip));
            }
        }
        let commit =
            self.repo
                .create_commit(tree, tip.into_iter().collect(), &self.author, message)?;
        self.repo.set_branch_tip(&self.branch, &commit)?;
        match self.repo.refs().delete(&wip_ref(&self.branch)) {
            Ok(()) | Err(Error::RefNotFound(_)) => {}
            Err(e) => return Err(e),
        }
        *self.last_persisted.lock().unwrap() = Some(tree);
        Ok(SnapshotOutcome::Created(commit))
    }

    /// Write dirty paths as objects and make them durable.
    fn persist_tree(&self, wt: &mut WorkTree) -> Result<Hash> {
        let tree = wt.checkpoint()?;
        self.repo.store().sync()?;
        Ok(tree)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use timefs_core::worktree::ROOT_INO;

    fn setup() -> (tempfile::TempDir, Engine) {
        let d = tempfile::tempdir().unwrap();
        let repo = Arc::new(Repository::init(d.path().join("repo")).unwrap());
        let engine = Engine::new(repo, "main", "tester", 64 * 1024).unwrap();
        (d, engine)
    }

    fn write(wt: &mut WorkTree, name: &str, data: &[u8]) {
        let a = wt.create(ROOT_INO, name.as_bytes(), 0o644, 0, 0).unwrap();
        wt.write(a.ino, 0, data).unwrap();
    }

    #[test]
    fn snapshot_commits_and_reports_unchanged() {
        let (_d, e) = setup();
        let (mut wt, _) = e.open_tree().unwrap();
        write(&mut wt, "a", b"1");
        let SnapshotOutcome::Created(first) = e.snapshot(&mut wt, "one").unwrap() else {
            panic!("expected a commit");
        };
        assert_eq!(e.repo().branch_tip("main").unwrap(), Some(first));
        assert!(
            matches!(e.snapshot(&mut wt, "again").unwrap(), SnapshotOutcome::Unchanged(h) if h == first)
        );
        write(&mut wt, "b", b"2");
        let SnapshotOutcome::Created(second) = e.snapshot(&mut wt, "two").unwrap() else {
            panic!("expected a commit");
        };
        assert_eq!(e.repo().get_commit(&second).unwrap().parents, vec![first]);
    }

    #[test]
    fn checkpoint_is_recovered_by_the_next_open() {
        let (_d, e) = setup();
        let (mut wt, rec) = e.open_tree().unwrap();
        assert_eq!(rec, Recovery::default());
        write(&mut wt, "keep", b"data");
        assert!(e.checkpoint(&mut wt).unwrap().is_some());
        assert!(
            e.checkpoint(&mut wt).unwrap().is_none(),
            "second checkpoint has nothing to do"
        );
        drop(wt); // simulate a crash: no snapshot, no final checkpoint

        let (mut again, rec) = e.open_tree().unwrap();
        assert!(rec.restored_checkpoint);
        assert!(again.lookup(ROOT_INO, b"keep").is_ok());
    }

    #[test]
    fn snapshot_clears_the_checkpoint() {
        let (_d, e) = setup();
        let (mut wt, _) = e.open_tree().unwrap();
        write(&mut wt, "f", b"x");
        e.checkpoint(&mut wt).unwrap();
        e.snapshot(&mut wt, "s").unwrap();
        assert!(e.repo().refs().read(&wip_ref("main")).unwrap().is_none());
        let (_, rec) = e.open_tree().unwrap();
        assert_eq!(rec, Recovery::default());
    }

    #[test]
    fn stale_checkpoint_is_ignored_not_applied() {
        let (_d, e) = setup();
        let (mut wt, _) = e.open_tree().unwrap();
        write(&mut wt, "f", b"x");
        e.checkpoint(&mut wt).unwrap(); // wip parented on "no tip"
                                        // the branch advances by other means (for example an offline merge)
        let tree = e
            .repo()
            .get_commit(&e.repo().refs().read(&wip_ref("main")).unwrap().unwrap())
            .unwrap()
            .tree;
        let other = e.repo().create_commit(tree, vec![], "x", "other").unwrap();
        e.repo().set_branch_tip("main", &other).unwrap();
        let (_, rec) = e.open_tree().unwrap();
        assert!(rec.ignored_stale_checkpoint && !rec.restored_checkpoint);
    }
}
