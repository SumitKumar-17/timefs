//! Applies the same operations to the real `WorkTree` and the model, and compares.

use std::collections::BTreeMap;
use std::sync::Arc;

use timefs_core::object::EntryKind;
use timefs_core::store::{LooseStore, ObjectStore};
use timefs_core::worktree::{FsError, FsResult, Ino, Options, WorkTree, ROOT_INO};

use crate::model::{Kind, MNode, MResult, Model};

pub const BLOCK: usize = 8;

#[derive(Clone, Debug)]
pub enum Op {
    Mkdir(Vec<String>),
    Create(Vec<String>),
    Symlink(Vec<String>, String),
    Write(Vec<String>, usize, Vec<u8>),
    Truncate(Vec<String>, usize),
    /// Metadata-only change (mode); content and tree shape stay the same.
    Chmod(Vec<String>, u32),
    Unlink(Vec<String>),
    Rmdir(Vec<String>),
    Rename(Vec<String>, Vec<String>),
    /// Persist, reopen from the resulting root hash, and keep going on the reopened tree.
    CheckpointReload,
}

pub struct Harness {
    _dir: tempfile::TempDir,
    store: Arc<dyn ObjectStore>,
    pub wt: WorkTree,
    pub model: Model,
}

impl Harness {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn ObjectStore> =
            Arc::new(LooseStore::open(dir.path().join("objects")).unwrap());
        let wt = open(&store, None);
        Harness {
            _dir: dir,
            store,
            wt,
            model: Model::new(),
        }
    }

    /// Apply `op` to both sides and assert they agree on success or on the error kind.
    pub fn apply(&mut self, op: &Op) -> Result<(), String> {
        if let Op::CheckpointReload = op {
            return self.checkpoint_reload();
        }
        let real = self.apply_real(op).map_err(kind_of);
        let expected = self.apply_model(op);
        if real != expected {
            return Err(format!("{op:?}: real {real:?} != model {expected:?}"));
        }
        Ok(())
    }

    fn checkpoint_reload(&mut self) -> Result<(), String> {
        let root = self.wt.checkpoint().map_err(|e| e.to_string())?;
        let again = self.wt.checkpoint().map_err(|e| e.to_string())?;
        if root != again {
            return Err("checkpoint without changes produced a different root".into());
        }
        self.store.sync().map_err(|e| e.to_string())?;
        self.wt = open(&self.store, Some(root));
        Ok(())
    }

    fn apply_real(&mut self, op: &Op) -> FsResult<()> {
        let wt = &mut self.wt;
        match op {
            Op::Mkdir(p) => {
                let (parent, name) = split(p);
                let dir = resolve_dir(wt, parent)?;
                wt.mkdir(dir, name.as_bytes(), 0o755, 0, 0).map(|_| ())
            }
            Op::Create(p) => {
                let (parent, name) = split(p);
                let dir = resolve_dir(wt, parent)?;
                wt.create(dir, name.as_bytes(), 0o644, 0, 0).map(|_| ())
            }
            Op::Symlink(p, target) => {
                let (parent, name) = split(p);
                let dir = resolve_dir(wt, parent)?;
                wt.symlink(dir, name.as_bytes(), target.as_bytes(), 0, 0)
                    .map(|_| ())
            }
            Op::Write(p, off, data) => {
                let ino = resolve_entry(wt, p)?;
                wt.write(ino, *off as u64, data).map(|_| ())
            }
            Op::Truncate(p, size) => {
                let ino = resolve_entry(wt, p)?;
                let change = timefs_core::worktree::SetAttr {
                    size: Some(*size as u64),
                    ..Default::default()
                };
                wt.setattr(ino, change).map(|_| ())
            }
            Op::Chmod(p, mode) => {
                let ino = resolve_entry(wt, p)?;
                let change = timefs_core::worktree::SetAttr {
                    mode: Some(*mode),
                    ..Default::default()
                };
                wt.setattr(ino, change).map(|_| ())
            }
            Op::Unlink(p) => {
                let (parent, name) = split(p);
                let dir = resolve_dir(wt, parent)?;
                wt.unlink(dir, name.as_bytes())
            }
            Op::Rmdir(p) => {
                let (parent, name) = split(p);
                let dir = resolve_dir(wt, parent)?;
                wt.rmdir(dir, name.as_bytes())
            }
            Op::Rename(from, to) => {
                let (fp, fname) = split(from);
                let (tp, tname) = split(to);
                let src_dir = resolve_dir(wt, fp)?;
                let dst_dir = resolve_dir(wt, tp)?;
                wt.rename(src_dir, fname.as_bytes(), dst_dir, tname.as_bytes(), false)
            }
            Op::CheckpointReload => unreachable!("handled by apply"),
        }
    }

    fn apply_model(&mut self, op: &Op) -> MResult<()> {
        let m = &mut self.model;
        match op {
            Op::Mkdir(p) => {
                let (a, n) = split(p);
                m.mkdir(a, n)
            }
            Op::Create(p) => {
                let (a, n) = split(p);
                m.create(a, n)
            }
            Op::Symlink(p, t) => {
                let (a, n) = split(p);
                m.symlink(a, n, t)
            }
            Op::Write(p, off, d) => {
                let (a, n) = split(p);
                m.write(a, n, *off, d)
            }
            Op::Truncate(p, s) => {
                let (a, n) = split(p);
                m.truncate(a, n, *s)
            }
            Op::Chmod(p, _) => {
                let (a, n) = split(p);
                m.chmod(a, n)
            }
            Op::Unlink(p) => {
                let (a, n) = split(p);
                m.unlink(a, n)
            }
            Op::Rmdir(p) => {
                let (a, n) = split(p);
                m.rmdir(a, n)
            }
            Op::Rename(f, t) => {
                let (fa, fnm) = split(f);
                let (ta, tn) = split(t);
                m.rename(fa, fnm, ta, tn)
            }
            Op::CheckpointReload => unreachable!("handled by apply"),
        }
    }

    /// Compare the full trees (names, kinds, contents, link targets).
    pub fn assert_same_tree(&mut self) -> Result<(), String> {
        let real = dump_real(&mut self.wt, ROOT_INO).map_err(|e| e.to_string())?;
        let expected = Model {
            root: self.model.root.clone(),
        }
        .root;
        if real != expected {
            return Err(format!(
                "trees differ\nreal:  {real:?}\nmodel: {expected:?}"
            ));
        }
        Ok(())
    }
}

fn open(store: &Arc<dyn ObjectStore>, root: Option<timefs_core::Hash>) -> WorkTree {
    let opts = Options {
        block_size: BLOCK,
        ..Options::default()
    };
    WorkTree::new(Arc::clone(store), root, opts)
}

fn split(path: &[String]) -> (&[String], &str) {
    let (name, parent) = path.split_last().expect("paths are non-empty");
    (parent, name)
}

fn resolve_dir(wt: &mut WorkTree, path: &[String]) -> FsResult<Ino> {
    let mut cur = ROOT_INO;
    for part in path {
        let attr = wt.lookup(cur, part.as_bytes())?;
        if attr.kind != EntryKind::Dir {
            return Err(FsError::NotDir);
        }
        cur = attr.ino;
    }
    Ok(cur)
}

fn resolve_entry(wt: &mut WorkTree, path: &[String]) -> FsResult<Ino> {
    let (parent, name) = split(path);
    let dir = resolve_dir(wt, parent)?;
    Ok(wt.lookup(dir, name.as_bytes())?.ino)
}

fn kind_of(e: FsError) -> Kind {
    match e {
        FsError::NotFound => Kind::NotFound,
        FsError::Exists => Kind::Exists,
        FsError::NotDir => Kind::NotDir,
        FsError::IsDir => Kind::IsDir,
        FsError::NotEmpty => Kind::NotEmpty,
        FsError::InvalidArgument(_) => Kind::Invalid,
        other => panic!("unexpected error from the working tree: {other}"),
    }
}

fn dump_real(wt: &mut WorkTree, dir: Ino) -> FsResult<BTreeMap<String, MNode>> {
    let mut out = BTreeMap::new();
    for e in wt.readdir(dir)? {
        let name = String::from_utf8(e.name).unwrap();
        let node = match e.kind {
            EntryKind::Dir => MNode::Dir(dump_real(wt, e.ino)?),
            EntryKind::File => MNode::File(wt.read(e.ino, 0, 1 << 20)?),
            EntryKind::Symlink => MNode::Link(String::from_utf8(wt.readlink(e.ino)?).unwrap()),
        };
        out.insert(name, node);
    }
    Ok(out)
}
