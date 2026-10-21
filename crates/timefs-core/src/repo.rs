//! A TimeFS repository: object store + refs + HEAD + exclusive lock.

use std::collections::{BTreeSet, BinaryHeap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::hash::Hash;
use crate::object::Commit;
use crate::refs::{self, FileRefs, Refs};
use crate::store::{CachedStore, LooseStore, ObjectStore, ObjectStoreExt, PackStore};
use crate::timeutil::now_ns;

const FORMAT_FILE: &str = "FORMAT";
const LOOSE_FORMAT: &str = "timefs-repo 1";
const PACK_FORMAT: &str = "timefs-repo 2";
const HEAD_FILE: &str = "HEAD";
const LOCK_FILE: &str = "LOCK";
const DEFAULT_BRANCH: &str = "main";
/// Read cache size for objects (immutable, so it never needs invalidation).
pub const DEFAULT_CACHE_BYTES: usize = 64 * 1024 * 1024;

/// How objects and refs are stored on disk (see `docs/format.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreKind {
    /// One file per object and per ref (format 1).
    Loose,
    /// Objects and refs in an append-only log (format 2, ADR-0009).
    Pack,
}

impl StoreKind {
    fn format_line(self) -> &'static str {
        match self {
            StoreKind::Loose => LOOSE_FORMAT,
            StoreKind::Pack => PACK_FORMAT,
        }
    }

    fn from_format_line(line: &str) -> Option<Self> {
        match line {
            LOOSE_FORMAT => Some(StoreKind::Loose),
            PACK_FORMAT => Some(StoreKind::Pack),
            _ => None,
        }
    }
}

/// Held while a process has exclusive use of the repository. Released on drop.
pub struct RepoLock {
    _file: File,
}

pub struct Repository {
    root: PathBuf,
    kind: StoreKind,
    store: Arc<dyn ObjectStore>,
    refs: Arc<dyn Refs>,
}

impl Repository {
    /// Create a new repository at `path` with the default store (the pack log).
    pub fn init(path: impl AsRef<Path>) -> Result<Self> {
        Self::init_with(path, StoreKind::Pack)
    }

    /// Create a new repository at `path` (the directory is created if missing).
    pub fn init_with(path: impl AsRef<Path>, kind: StoreKind) -> Result<Self> {
        let root = path.as_ref().to_path_buf();
        if root.join(FORMAT_FILE).exists() {
            return Err(Error::Invalid(format!(
                "{} is already a repository",
                root.display()
            )));
        }
        match kind {
            StoreKind::Loose => {
                fs::create_dir_all(root.join("objects"))?;
                fs::create_dir_all(root.join("refs").join(refs::HEADS))?;
                fs::create_dir_all(root.join("refs").join(refs::WIP))?;
            }
            StoreKind::Pack => fs::create_dir_all(root.join("pack"))?,
        }
        File::create(root.join(LOCK_FILE))?;
        fs::write(
            root.join(HEAD_FILE),
            format!("ref: refs/heads/{DEFAULT_BRANCH}\n"),
        )?;
        // FORMAT last: a half-initialised directory is not mistaken for a repository.
        fs::write(root.join(FORMAT_FILE), format!("{}\n", kind.format_line()))?;
        Self::open(root)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let root = path.as_ref().to_path_buf();
        let kind = match fs::read_to_string(root.join(FORMAT_FILE)) {
            Ok(text) => StoreKind::from_format_line(text.trim())
                .ok_or_else(|| Error::UnsupportedFormat(text.trim().to_string()))?,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                return Err(Error::NotARepository(root.display().to_string()))
            }
            Err(e) => return Err(e.into()),
        };
        let with_cache = |disk: Arc<dyn ObjectStore>| -> Arc<dyn ObjectStore> {
            Arc::new(CachedStore::new(disk, DEFAULT_CACHE_BYTES))
        };
        let (store, refs): (Arc<dyn ObjectStore>, Arc<dyn Refs>) = match kind {
            StoreKind::Loose => {
                let store = with_cache(Arc::new(LooseStore::open(root.join("objects"))?));
                let refs = FileRefs::new(root.join("refs")).with_store(Arc::clone(&store));
                (store, Arc::new(refs))
            }
            StoreKind::Pack => {
                let pack = Arc::new(PackStore::open(root.join("pack"))?);
                (with_cache(Arc::clone(&pack) as Arc<dyn ObjectStore>), pack)
            }
        };
        Ok(Repository {
            root,
            kind,
            store,
            refs,
        })
    }

    pub fn store_kind(&self) -> StoreKind {
        self.kind
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn store(&self) -> &Arc<dyn ObjectStore> {
        &self.store
    }

    pub fn refs(&self) -> &dyn Refs {
        self.refs.as_ref()
    }

    /// Take the exclusive repository lock without blocking.
    pub fn lock_exclusive(&self) -> Result<RepoLock> {
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.root.join(LOCK_FILE))?;
        match file.try_lock() {
            Ok(()) => Ok(RepoLock { _file: file }),
            Err(fs::TryLockError::WouldBlock) => Err(Error::Locked),
            Err(fs::TryLockError::Error(e)) => Err(e.into()),
        }
    }

    pub fn control_socket_path(&self) -> PathBuf {
        self.root.join("timefs.sock")
    }

    // ---- HEAD and branches -------------------------------------------------------------

    pub fn head_branch(&self) -> Result<String> {
        let text = fs::read_to_string(self.root.join(HEAD_FILE))?;
        text.trim()
            .strip_prefix("ref: refs/heads/")
            .map(str::to_string)
            .ok_or_else(|| Error::corrupt("HEAD is not a branch reference"))
    }

    /// Point HEAD at `branch`. The write is atomic (temp file, fsync, rename, fsync dir)
    /// because a torn HEAD would make every command fail.
    pub fn set_head_branch(&self, branch: &str) -> Result<()> {
        refs::validate_ref_name(&branch_ref(branch))?;
        // Unique per process and per call so concurrent writers never share a temp file.
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let tmp = self.root.join(format!(
            ".{HEAD_FILE}.tmp-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        refs::write_atomic(
            &tmp,
            &self.root.join(HEAD_FILE),
            format!("ref: refs/heads/{branch}\n").as_bytes(),
        )?;
        Ok(())
    }

    pub fn branch_tip(&self, branch: &str) -> Result<Option<Hash>> {
        self.refs.read(&branch_ref(branch))
    }

    pub fn branches(&self) -> Result<Vec<(String, Hash)>> {
        self.refs.list(refs::HEADS)
    }

    pub fn create_branch(&self, name: &str, at: &Hash) -> Result<()> {
        self.get_commit(at)?;
        self.refs.create(&branch_ref(name), at)
    }

    pub fn delete_branch(&self, name: &str) -> Result<()> {
        if self.head_branch()? == name {
            return Err(Error::Invalid("cannot delete the current branch".into()));
        }
        self.refs.delete(&branch_ref(name))
    }

    /// Move a branch to `commit`. `Refs::write` makes objects durable first.
    pub fn set_branch_tip(&self, branch: &str, commit: &Hash) -> Result<()> {
        self.refs.write(&branch_ref(branch), commit)
    }

    // ---- Commits -----------------------------------------------------------------------

    pub fn create_commit(
        &self,
        tree: Hash,
        parents: Vec<Hash>,
        author: &str,
        message: &str,
    ) -> Result<Hash> {
        self.store.put_commit(&Commit {
            tree,
            parents,
            time_ns: now_ns(),
            author: author.to_string(),
            message: message.to_string(),
        })
    }

    pub fn get_commit(&self, hash: &Hash) -> Result<Commit> {
        self.store.get_commit(hash)
    }

    /// Commit `tree` on top of `branch` and advance the branch.
    pub fn commit_to_branch(
        &self,
        branch: &str,
        tree: Hash,
        author: &str,
        message: &str,
    ) -> Result<Hash> {
        let parents: Vec<Hash> = self.branch_tip(branch)?.into_iter().collect();
        let commit = self.create_commit(tree, parents, author, message)?;
        self.set_branch_tip(branch, &commit)?;
        Ok(commit)
    }

    /// Turn the pending checkpoint of `branch` into a commit on the branch (what `snap` does
    /// for a mounted tree). Returns `None` when there is no checkpoint, or when it adds
    /// nothing over the branch tip.
    pub fn promote_checkpoint(
        &self,
        branch: &str,
        author: &str,
        message: &str,
    ) -> Result<Option<Hash>> {
        let wip_name = wip_ref(branch);
        let Some(wip) = self.refs.read(&wip_name)? else {
            return Ok(None);
        };
        let checkpoint = self.get_commit(&wip)?;
        let tip = self.branch_tip(branch)?;
        if checkpoint.parents != tip.into_iter().collect::<Vec<_>>() {
            return Err(Error::Invalid(format!(
                "the checkpoint of branch {branch} is stale (the branch moved after it was written)"
            )));
        }
        let unchanged = match tip {
            Some(t) => self.get_commit(&t)?.tree == checkpoint.tree,
            None => false,
        };
        let result = if unchanged {
            None
        } else {
            let commit =
                self.create_commit(checkpoint.tree, checkpoint.parents, author, message)?;
            self.set_branch_tip(branch, &commit)?;
            Some(commit)
        };
        self.refs.delete(&wip_name)?;
        Ok(result)
    }

    /// Whether `branch` has unsnapshotted changes saved in a checkpoint.
    pub fn has_checkpoint(&self, branch: &str) -> Result<bool> {
        Ok(self.refs.read(&wip_ref(branch))?.is_some())
    }

    /// History from `tip`, newest first (by commit time, ties broken by hash).
    pub fn log(&self, tip: &Hash, limit: usize) -> Result<Vec<(Hash, Commit)>> {
        let mut commits: HashMap<Hash, Commit> = HashMap::new();
        let mut seen = HashSet::from([*tip]);
        let mut heap = BinaryHeap::new();
        let first = self.get_commit(tip)?;
        heap.push((first.time_ns, *tip));
        commits.insert(*tip, first);
        let mut out = Vec::new();
        while let Some((_, hash)) = heap.pop() {
            let commit = commits.remove(&hash).expect("queued commits are loaded");
            for p in &commit.parents {
                if seen.insert(*p) {
                    let parent = self.get_commit(p)?;
                    heap.push((parent.time_ns, *p));
                    commits.insert(*p, parent);
                }
            }
            out.push((hash, commit));
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    /// Every commit reachable from `tip` (inclusive), with its parents.
    pub fn ancestry(&self, tip: &Hash) -> Result<HashMap<Hash, Vec<Hash>>> {
        let mut map = HashMap::new();
        let mut stack = vec![*tip];
        while let Some(h) = stack.pop() {
            if map.contains_key(&h) {
                continue;
            }
            let parents = self.get_commit(&h)?.parents;
            stack.extend(parents.iter().copied());
            map.insert(h, parents);
        }
        Ok(map)
    }

    /// Best common ancestor of two commits. With several (criss-cross merges) the lowest
    /// hash is chosen so the result is deterministic (ADR-0006).
    pub fn merge_base(&self, a: &Hash, b: &Hash) -> Result<Option<Hash>> {
        let anc_a = self.ancestry(a)?;
        let anc_b = self.ancestry(b)?;
        let common: BTreeSet<Hash> = anc_a
            .keys()
            .filter(|h| anc_b.contains_key(h))
            .copied()
            .collect();
        // A common ancestor reachable from another common ancestor is not "best".
        let mut redundant = HashSet::new();
        let mut stack: Vec<Hash> = common
            .iter()
            .flat_map(|h| anc_a[h].iter().copied())
            .collect();
        while let Some(h) = stack.pop() {
            if redundant.insert(h) {
                stack.extend(anc_a[&h].iter().copied());
            }
        }
        Ok(common.into_iter().find(|h| !redundant.contains(h)))
    }

    // ---- Revision parsing --------------------------------------------------------------

    /// Resolve `HEAD`, a branch name, a full or abbreviated hash, optionally followed by
    /// `~N` (N-th first parent).
    pub fn resolve(&self, rev: &str) -> Result<Hash> {
        let (base, ancestors) = split_ancestor_suffix(rev)?;
        let mut hash = self.resolve_base(base)?;
        for _ in 0..ancestors {
            hash = *self
                .get_commit(&hash)?
                .parents
                .first()
                .ok_or_else(|| Error::BadRevision(rev.to_string()))?;
        }
        Ok(hash)
    }

    fn resolve_base(&self, name: &str) -> Result<Hash> {
        if name == "HEAD" {
            let branch = self.head_branch()?;
            return self.branch_tip(&branch)?.ok_or_else(|| {
                Error::BadRevision(format!("HEAD (branch {branch} has no commits)"))
            });
        }
        if let Some(tip) = self.refs.read(&branch_ref(name)).ok().flatten() {
            return Ok(tip);
        }
        if name.len() >= 4 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            if name.len() == 64 {
                return Hash::from_hex(name);
            }
            let matches = self.store.list_prefix(name)?;
            return match matches.as_slice() {
                [one] => Ok(*one),
                [] => Err(Error::BadRevision(name.to_string())),
                _ => Err(Error::AmbiguousRevision(name.to_string())),
            };
        }
        Err(Error::BadRevision(name.to_string()))
    }
}

pub fn branch_ref(branch: &str) -> String {
    format!("{}/{}", refs::HEADS, branch)
}

pub fn wip_ref(branch: &str) -> String {
    format!("{}/{}", refs::WIP, branch)
}

fn split_ancestor_suffix(rev: &str) -> Result<(&str, usize)> {
    match rev.rsplit_once('~') {
        None => Ok((rev, 0)),
        Some((base, n)) => {
            let n = if n.is_empty() {
                1
            } else {
                n.parse().map_err(|_| Error::BadRevision(rev.to_string()))?
            };
            Ok((base, n))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::Tree;

    fn repo() -> (tempfile::TempDir, Repository) {
        let d = tempfile::tempdir().unwrap();
        let r = Repository::init(d.path().join("repo")).unwrap();
        (d, r)
    }

    fn commit(r: &Repository, parents: Vec<Hash>, msg: &str) -> Hash {
        let tree = r.store().put_tree(&Tree::new()).unwrap();
        r.create_commit(tree, parents, "t", msg).unwrap()
    }

    #[test]
    fn init_open_and_reject_double_init() {
        let (d, r) = repo();
        assert_eq!(r.head_branch().unwrap(), "main");
        assert!(Repository::init(d.path().join("repo")).is_err());
        assert!(Repository::open(d.path().join("repo")).is_ok());
        assert!(matches!(
            Repository::open(d.path().join("nothing")),
            Err(Error::NotARepository(_))
        ));
    }

    #[test]
    fn lock_is_exclusive() {
        let (_d, r) = repo();
        let first = r.lock_exclusive().unwrap();
        assert!(matches!(r.lock_exclusive(), Err(Error::Locked)));
        drop(first);
        assert!(r.lock_exclusive().is_ok());
    }

    #[test]
    fn branches_and_head() {
        let (_d, r) = repo();
        let c = commit(&r, vec![], "root");
        r.set_branch_tip("main", &c).unwrap();
        r.create_branch("dev", &c).unwrap();
        assert!(r.create_branch("dev", &c).is_err());
        assert_eq!(r.branches().unwrap().len(), 2);
        assert!(r.delete_branch("main").is_err());
        r.delete_branch("dev").unwrap();
        r.set_head_branch("feature/x").unwrap();
        assert_eq!(r.head_branch().unwrap(), "feature/x");
    }

    #[test]
    fn set_head_branch_round_trips_and_leaves_no_temp_files() {
        let (_d, r) = repo();
        for branch in ["dev", "feature/x", "main"] {
            r.set_head_branch(branch).unwrap();
            assert_eq!(r.head_branch().unwrap(), branch);
        }
        let leftovers: Vec<_> = fs::read_dir(r.root())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
        assert!(r.set_head_branch("bad..name").is_err());
        assert_eq!(
            r.head_branch().unwrap(),
            "main",
            "a rejected name leaves HEAD alone"
        );
    }

    #[test]
    fn resolve_variants() {
        let (_d, r) = repo();
        let a = commit(&r, vec![], "a");
        let b = commit(&r, vec![a], "b");
        r.set_branch_tip("main", &b).unwrap();
        assert_eq!(r.resolve("HEAD").unwrap(), b);
        assert_eq!(r.resolve("main").unwrap(), b);
        assert_eq!(r.resolve("main~1").unwrap(), a);
        assert_eq!(r.resolve("HEAD~").unwrap(), a);
        assert_eq!(r.resolve(&b.to_hex()).unwrap(), b);
        assert_eq!(r.resolve(&a.to_hex()[..10]).unwrap(), a);
        assert!(r.resolve("HEAD~5").is_err());
        assert!(r.resolve("nope").is_err());
    }

    #[test]
    fn log_orders_newest_first_and_honours_limit() {
        let (_d, r) = repo();
        let a = commit(&r, vec![], "a");
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = commit(&r, vec![a], "b");
        std::thread::sleep(std::time::Duration::from_millis(2));
        let c = commit(&r, vec![b], "c");
        let msgs: Vec<_> = r
            .log(&c, 10)
            .unwrap()
            .into_iter()
            .map(|x| x.1.message)
            .collect();
        assert_eq!(msgs, ["c", "b", "a"]);
        assert_eq!(r.log(&c, 2).unwrap().len(), 2);
    }

    /// A tree containing one file, so it differs from the empty tree.
    fn non_empty_tree(r: &Repository) -> Hash {
        use crate::object::{EntryKind, TreeEntry};
        let blob = r.store().put_file(&Default::default()).unwrap();
        let entry = TreeEntry {
            name: b"f".to_vec(),
            kind: EntryKind::File,
            mode: 0o644,
            uid: 0,
            gid: 0,
            size: 0,
            mtime_ns: 0,
            hash: blob,
        };
        r.store()
            .put_tree(&Tree::from_entries(vec![entry]).unwrap())
            .unwrap()
    }

    #[test]
    fn promote_checkpoint_covers_each_case() {
        let (_d, r) = repo();
        let base = commit(&r, vec![], "base");
        r.set_branch_tip("main", &base).unwrap();
        let base_tree = r.get_commit(&base).unwrap().tree;

        // no checkpoint
        assert_eq!(r.promote_checkpoint("main", "t", "m").unwrap(), None);

        // a checkpoint identical to the tip adds nothing and is cleared
        let same = r
            .create_commit(base_tree, vec![base], "t", "checkpoint")
            .unwrap();
        r.refs().write("wip/main", &same).unwrap();
        assert!(r.has_checkpoint("main").unwrap());
        assert_eq!(r.promote_checkpoint("main", "t", "m").unwrap(), None);
        assert!(!r.has_checkpoint("main").unwrap());

        // a checkpoint with new content becomes a commit on the branch
        let changed = r
            .create_commit(non_empty_tree(&r), vec![base], "t", "checkpoint")
            .unwrap();
        r.refs().write("wip/main", &changed).unwrap();
        let promoted = r.promote_checkpoint("main", "t", "saved").unwrap().unwrap();
        assert_eq!(r.branch_tip("main").unwrap(), Some(promoted));
        let c = r.get_commit(&promoted).unwrap();
        assert_eq!((c.parents, c.message.as_str()), (vec![base], "saved"));
        assert!(!r.has_checkpoint("main").unwrap());

        // a checkpoint that no longer descends from the tip is refused
        let stale = r
            .create_commit(base_tree, vec![base], "t", "checkpoint")
            .unwrap();
        r.refs().write("wip/main", &stale).unwrap();
        assert!(r.promote_checkpoint("main", "t", "m").is_err());
    }

    #[test]
    fn merge_base_of_diverged_branches() {
        let (_d, r) = repo();
        let root = commit(&r, vec![], "root");
        let base = commit(&r, vec![root], "base");
        let left = commit(&r, vec![base], "left");
        let right = commit(&r, vec![base], "right");
        assert_eq!(r.merge_base(&left, &right).unwrap(), Some(base));
        assert_eq!(r.merge_base(&left, &base).unwrap(), Some(base));
        assert_eq!(r.merge_base(&left, &left).unwrap(), Some(left));
        let unrelated = commit(&r, vec![], "other root");
        assert_eq!(r.merge_base(&left, &unrelated).unwrap(), None);
    }

    #[test]
    fn merge_base_after_a_merge_picks_the_merge_not_the_old_fork() {
        let (_d, r) = repo();
        let base = commit(&r, vec![], "base");
        let a = commit(&r, vec![base], "a");
        let b = commit(&r, vec![base], "b");
        let m = commit(&r, vec![a, b], "merge");
        let a2 = commit(&r, vec![m], "a2");
        let b2 = commit(&r, vec![b], "b2");
        // common ancestors of a2 and b2: base, b. b is the best (base is behind it).
        assert_eq!(r.merge_base(&a2, &b2).unwrap(), Some(b));
    }
}
