//! Q3: how fast are branch creation and restoring an arbitrary commit?

use timefs_core::chunk::Chunker;
use timefs_core::hostfs::export_tree;
use timefs_core::Repository;

use super::common::resolve;
use super::Res;
use crate::env::{import_commit, open_tree, run_ok, Config};
use crate::gen::{build_tree, file_path, TreeSpec};
use crate::report::{ms, timed, Summary, Table};

pub fn run(cfg: &Config) -> Res {
    let mut table = Table::new(&["files", "operation", "ms median (min..max)"]);
    for &n in &cfg.sizes {
        let work = cfg.scratch(&format!("branch-{n}"))?;
        let src = work.join("src");
        build_tree(
            &src,
            &TreeSpec {
                files: n,
                file_size: cfg.file_size,
                seed: 1,
            },
        )?;
        let repo = Repository::init(work.join("repo"))?;
        let commit = import_commit(&repo, &src, &Chunker::default(), "base");
        let tree = repo.get_commit(&commit)?.tree;

        let mut create = Vec::new();
        for i in 0..cfg.repeat.max(5) {
            let (r, d) = timed(|| repo.create_branch(&format!("b{i}"), &commit));
            r?;
            create.push(ms(d));
        }
        table.row(vec![
            n.to_string(),
            "create branch".into(),
            Summary::of(&create).show(3),
        ]);

        let mut open = Vec::new();
        for _ in 0..cfg.repeat {
            let (_, d) = timed(|| {
                let mut wt = open_tree(&repo, tree);
                let ino = resolve(&mut wt, &file_path(n / 2));
                wt.read(ino, 0, 4096).expect("read");
            });
            open.push(ms(d));
        }
        table.row(vec![
            n.to_string(),
            "open commit and read one file".into(),
            Summary::of(&open).show(3),
        ]);

        let mut export = Vec::new();
        let mut copy = Vec::new();
        for i in 0..cfg.repeat.min(3) {
            let dest = work.join(format!("export-{i}"));
            let (r, d) = timed(|| export_tree(repo.store().as_ref(), &tree, &dest));
            r?;
            export.push(ms(d));
            let dest = work.join(format!("copy-{i}"));
            let (_, d) = timed(|| {
                run_ok(
                    "cp",
                    &["-r", src.to_str().unwrap(), dest.to_str().unwrap()],
                    None,
                )
            });
            copy.push(ms(d));
        }
        table.row(vec![
            n.to_string(),
            "materialise whole tree (timefs export)".into(),
            Summary::of(&export).show(1),
        ]);
        table.row(vec![
            n.to_string(),
            "cp -r of the tree".into(),
            Summary::of(&copy).show(1),
        ]);
        std::fs::remove_dir_all(&work)?;
    }
    table.print("Branch and restore latency (Q3)");
    Ok(())
}
