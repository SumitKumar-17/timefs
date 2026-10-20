use std::fs;

use super::*;
use crate::chunk::Chunker;
use crate::hostfs::import_dir;
use crate::store::ObjectStore;
use crate::treeops;

struct Fixture {
    dir: tempfile::TempDir,
    repo: Repository,
}

/// A directory layout as `(path, content)` pairs; content `None` makes a directory.
type Layout<'a> = &'a [(&'a str, Option<&'a str>)];

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path().join("repo")).unwrap();
        Fixture { dir, repo }
    }

    /// Import a layout as a commit with the given parents.
    fn commit(&self, name: &str, layout: Layout, parents: Vec<Hash>) -> Hash {
        let root = self.dir.path().join(name);
        fs::create_dir_all(&root).unwrap();
        for (path, content) in layout {
            let p = root.join(path);
            match content {
                None => fs::create_dir_all(&p).unwrap(),
                Some(c) => {
                    fs::create_dir_all(p.parent().unwrap()).unwrap();
                    fs::write(&p, c).unwrap();
                }
            }
        }
        let (tree, _) = import_dir(self.repo.store().as_ref(), &root, &Chunker::default()).unwrap();
        self.repo.create_commit(tree, parents, "t", name).unwrap()
    }

    fn read(&self, commit: &Hash, path: &str) -> Option<String> {
        let tree = self.repo.get_commit(commit).unwrap().tree;
        let store: &dyn ObjectStore = self.repo.store().as_ref();
        let entry = treeops::lookup_path(store, &tree, path).unwrap()?;
        Some(String::from_utf8(treeops::read_file(store, &entry.hash).unwrap()).unwrap())
    }

    fn exists(&self, commit: &Hash, path: &str) -> bool {
        let tree = self.repo.get_commit(commit).unwrap().tree;
        treeops::lookup_path(self.repo.store().as_ref(), &tree, path)
            .unwrap()
            .is_some()
    }

    fn merge(&self, ours: &Hash, theirs: &Hash, resolve: Resolve) -> MergeOutcome {
        merge_commits(&self.repo, ours, theirs, resolve, "t", "merge").unwrap()
    }
}

fn merged(outcome: MergeOutcome) -> Hash {
    match outcome {
        MergeOutcome::Merged { commit, .. } => commit,
        other => panic!("expected a merge commit, got {other:?}"),
    }
}

fn conflicts(outcome: MergeOutcome) -> Vec<Conflict> {
    match outcome {
        MergeOutcome::Conflicts(c) => c,
        other => panic!("expected conflicts, got {other:?}"),
    }
}

#[test]
fn disjoint_changes_merge_cleanly() {
    let fx = Fixture::new();
    let base = fx.commit(
        "base",
        &[
            ("keep", Some("k")),
            ("edit", Some("v1")),
            ("gone", Some("g")),
        ],
        vec![],
    );
    let ours = fx.commit(
        "ours",
        &[
            ("keep", Some("k")),
            ("edit", Some("v1")),
            ("ours_new", Some("o")),
        ],
        vec![base],
    );
    let theirs = fx.commit(
        "theirs",
        &[
            ("keep", Some("k")),
            ("edit", Some("v2")),
            ("gone", Some("g")),
            ("theirs_new", Some("t")),
        ],
        vec![base],
    );
    let m = merged(fx.merge(&ours, &theirs, Resolve::Abort));
    assert_eq!(fx.read(&m, "edit").unwrap(), "v2");
    assert_eq!(fx.read(&m, "ours_new").unwrap(), "o");
    assert_eq!(fx.read(&m, "theirs_new").unwrap(), "t");
    assert_eq!(fx.read(&m, "keep").unwrap(), "k");
    assert!(
        !fx.exists(&m, "gone"),
        "our deletion must win over their untouched copy"
    );
    assert_eq!(fx.repo.get_commit(&m).unwrap().parents, vec![ours, theirs]);
}

#[test]
fn merge_is_commutative_when_clean() {
    let fx = Fixture::new();
    let base = fx.commit("base", &[("a", Some("1")), ("b", Some("1"))], vec![]);
    let x = fx.commit(
        "x",
        &[("a", Some("2")), ("b", Some("1")), ("nx", Some("x"))],
        vec![base],
    );
    let y = fx.commit(
        "y",
        &[("a", Some("1")), ("b", Some("3")), ("ny", Some("y"))],
        vec![base],
    );
    let (m1, m2) = (
        merged(fx.merge(&x, &y, Resolve::Abort)),
        merged(fx.merge(&y, &x, Resolve::Abort)),
    );
    let (t1, t2) = (
        fx.repo.get_commit(&m1).unwrap().tree,
        fx.repo.get_commit(&m2).unwrap().tree,
    );
    assert_eq!(t1, t2);
}

#[test]
fn modify_modify_conflicts_and_resolutions() {
    let fx = Fixture::new();
    let base = fx.commit("base", &[("f", Some("base")), ("other", Some("o"))], vec![]);
    let ours = fx.commit(
        "ours",
        &[("f", Some("ours")), ("other", Some("o"))],
        vec![base],
    );
    let theirs = fx.commit(
        "theirs",
        &[("f", Some("theirs")), ("other", Some("o2"))],
        vec![base],
    );

    let c = conflicts(fx.merge(&ours, &theirs, Resolve::Abort));
    assert_eq!(c.len(), 1);
    assert_eq!(
        (c[0].path.as_str(), c[0].kind),
        ("f", ConflictKind::ModifyModify)
    );

    let m = merged(fx.merge(&ours, &theirs, Resolve::Ours));
    assert_eq!(
        (fx.read(&m, "f").unwrap(), fx.read(&m, "other").unwrap()),
        ("ours".into(), "o2".into())
    );
    let m = merged(fx.merge(&ours, &theirs, Resolve::Theirs));
    assert_eq!(fx.read(&m, "f").unwrap(), "theirs");
}

#[test]
fn modify_delete_and_add_add_conflicts() {
    let fx = Fixture::new();
    let base = fx.commit("base", &[("f", Some("1"))], vec![]);
    let ours = fx.commit(
        "ours",
        &[("f", Some("changed")), ("n", Some("ours"))],
        vec![base],
    );
    let theirs = fx.commit("theirs", &[("n", Some("theirs"))], vec![base]);
    let mut got: Vec<_> = conflicts(fx.merge(&ours, &theirs, Resolve::Abort))
        .into_iter()
        .map(|c| (c.path, c.kind))
        .collect();
    got.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        got,
        [
            ("f".to_string(), ConflictKind::ModifyDelete),
            ("n".to_string(), ConflictKind::AddAdd)
        ]
    );
    let m = merged(fx.merge(&ours, &theirs, Resolve::Theirs));
    assert!(
        !fx.exists(&m, "f"),
        "resolving to theirs applies their deletion"
    );
}

#[test]
fn identical_changes_on_both_sides_do_not_conflict() {
    let fx = Fixture::new();
    let base = fx.commit("base", &[("f", Some("1"))], vec![]);
    let ours = fx.commit("ours", &[("f", Some("same"))], vec![base]);
    let theirs = fx.commit("theirs", &[("f", Some("same"))], vec![base]);
    assert_eq!(
        fx.read(&merged(fx.merge(&ours, &theirs, Resolve::Abort)), "f")
            .unwrap(),
        "same"
    );
}

#[test]
fn touching_a_file_on_one_side_is_not_a_change() {
    let fx = Fixture::new();
    let base = fx.commit("base", &[("f", Some("1")), ("g", Some("1"))], vec![]);
    std::thread::sleep(std::time::Duration::from_millis(20));
    // same content, newer mtime on ours; theirs edits g
    let ours = fx.commit("ours", &[("f", Some("1")), ("g", Some("1"))], vec![base]);
    let theirs = fx.commit("theirs", &[("f", Some("1")), ("g", Some("2"))], vec![base]);
    let m = merged(fx.merge(&ours, &theirs, Resolve::Abort));
    assert_eq!(fx.read(&m, "g").unwrap(), "2");
}

#[test]
fn directories_added_on_both_sides_merge_recursively() {
    let fx = Fixture::new();
    let base = fx.commit("base", &[("keep", Some("k"))], vec![]);
    let ours = fx.commit(
        "ours",
        &[("keep", Some("k")), ("d/a", Some("a"))],
        vec![base],
    );
    let theirs = fx.commit(
        "theirs",
        &[("keep", Some("k")), ("d/b", Some("b"))],
        vec![base],
    );
    let m = merged(fx.merge(&ours, &theirs, Resolve::Abort));
    assert_eq!(fx.read(&m, "d/a").unwrap(), "a");
    assert_eq!(fx.read(&m, "d/b").unwrap(), "b");
}

#[test]
fn file_versus_directory_is_a_type_conflict() {
    let fx = Fixture::new();
    let base = fx.commit("base", &[("x", Some("file"))], vec![]);
    let ours = fx.commit("ours", &[("x", Some("changed file"))], vec![base]);
    let theirs = fx.commit("theirs", &[("x/inner", Some("now a dir"))], vec![base]);
    let c = conflicts(fx.merge(&ours, &theirs, Resolve::Abort));
    assert_eq!(
        (c[0].path.as_str(), c[0].kind),
        ("x", ConflictKind::TypeChange)
    );
}

#[test]
fn up_to_date_and_fast_forward() {
    let fx = Fixture::new();
    let a = fx.commit("a", &[("f", Some("1"))], vec![]);
    let b = fx.commit("b", &[("f", Some("2"))], vec![a]);
    assert!(matches!(
        fx.merge(&b, &a, Resolve::Abort),
        MergeOutcome::UpToDate
    ));
    assert!(matches!(
        fx.merge(&b, &b, Resolve::Abort),
        MergeOutcome::UpToDate
    ));
    assert!(
        matches!(fx.merge(&a, &b, Resolve::Abort), MergeOutcome::FastForward { tip } if tip == b)
    );
}

#[test]
fn unrelated_histories_merge_with_an_empty_base() {
    let fx = Fixture::new();
    let a = fx.commit("a", &[("only_a", Some("a"))], vec![]);
    let b = fx.commit("b", &[("only_b", Some("b"))], vec![]);
    let m = merged(fx.merge(&a, &b, Resolve::Abort));
    assert!(fx.exists(&m, "only_a") && fx.exists(&m, "only_b"));
}

#[test]
fn merging_a_commit_with_itself_via_the_tree_layer_is_identity() {
    let fx = Fixture::new();
    let a = fx.commit("a", &[("f", Some("1")), ("d/g", Some("2"))], vec![]);
    let tree = fx.repo.get_commit(&a).unwrap().tree;
    let mut m = TreeMerger::new(fx.repo.store().as_ref(), Resolve::Abort);
    assert_eq!(m.merge(Some(&tree), &tree, &tree).unwrap(), tree);
    assert!(m.conflicts.is_empty());
}
