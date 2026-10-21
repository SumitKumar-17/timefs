//! Q8: merge cost should follow the number of changed paths, not the tree size.

use timefs_core::chunk::Chunker;
use timefs_core::merge::{merge_commits, MergeOutcome, Resolve};
use timefs_core::{Hash, Repository};

use super::common::{pick_files, touch_file};
use super::Res;
use crate::env::{import_commit, open_tree, Config};
use crate::gen::{build_tree, TreeSpec};
use crate::report::{ms, timed, Summary, Table};

pub fn run(cfg: &Config) -> Res {
    let mut table = Table::new(&["files", "changed per side", "merge ms median (min..max)"]);
    for &n in &cfg.sizes {
        let work = cfg.scratch(&format!("merge-{n}"))?;
        let src = work.join("src");
        build_tree(
            &src,
            &TreeSpec {
                files: n,
                file_size: cfg.file_size,
                seed: 1,
            },
        )?;
        let repo = Repository::init_with(work.join("repo"), cfg.store)?;
        let base = import_commit(&repo, &src, &Chunker::default(), "base");

        for m in [1usize, 10, 100, 1000].into_iter().filter(|&m| 2 * m <= n) {
            let both = pick_files(n, 2 * m, 7);
            let (ours_set, theirs_set) = both.split_at(m);
            let ours = edit_commit(&repo, base, ours_set, 0x11);
            let theirs = edit_commit(&repo, base, theirs_set, 0x22);
            let mut samples = Vec::new();
            for _ in 0..cfg.repeat {
                let (outcome, d) =
                    timed(|| merge_commits(&repo, &ours, &theirs, Resolve::Abort, "b", "merge"));
                assert!(
                    matches!(outcome?, MergeOutcome::Merged { .. }),
                    "disjoint edits must merge cleanly"
                );
                samples.push(ms(d));
            }
            table.row(vec![
                n.to_string(),
                m.to_string(),
                Summary::of(&samples).show(2),
            ]);
        }
        std::fs::remove_dir_all(&work)?;
    }
    table.print("Merge latency, disjoint edits on both sides (Q8)");
    Ok(())
}

/// A commit on top of `base` that modifies the given files.
fn edit_commit(repo: &Repository, base: Hash, files: &[usize], byte: u8) -> Hash {
    let tree = repo.get_commit(&base).unwrap().tree;
    let mut wt = open_tree(repo, tree);
    for &i in files {
        touch_file(&mut wt, i, byte);
    }
    let root = wt.checkpoint().unwrap();
    repo.store().sync().unwrap();
    repo.create_commit(root, vec![base], "b", "edit").unwrap()
}
