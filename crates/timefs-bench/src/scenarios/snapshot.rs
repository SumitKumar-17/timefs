//! Q1 and Q2: how long does a snapshot take, and what does it cost in space?

use timefs_core::chunk::Chunker;
use timefs_core::Repository;

use super::common::{pick_files, touch_file};
use super::Res;
use crate::env::{have, import_commit, open_tree, run_ok, Config};
use crate::gen::{build_tree, TreeSpec};
use crate::report::{human_bytes, ms, timed, Summary, Table};

const EDIT_COUNTS: [usize; 4] = [0, 1, 10, 100];

pub fn run(cfg: &Config) -> Res {
    let mut table = Table::new(&[
        "files",
        "operation",
        "ms median (min..max)",
        "new objects",
        "new bytes",
    ]);
    for &n in &cfg.sizes {
        let work = cfg.scratch(&format!("snapshot-{n}"))?;
        let spec = TreeSpec {
            files: n,
            file_size: cfg.file_size,
            seed: 1,
        };
        let src = work.join("src");
        build_tree(&src, &spec)?;

        let copy = work.join("copy");
        let (_, cp) = timed(|| {
            run_ok(
                "cp",
                &["-r", src.to_str().unwrap(), copy.to_str().unwrap()],
                None,
            )
        });
        table.row(vec![
            n.to_string(),
            "cp -r (full copy)".into(),
            format!("{:.1}", ms(cp)),
            "n/a".into(),
            "full size".into(),
        ]);

        let repo = Repository::init_with(work.join("repo"), cfg.store)?;
        let (_, import) = timed(|| import_commit(&repo, &src, &Chunker::default(), "initial"));
        let st = repo.store().stats();
        table.row(vec![
            n.to_string(),
            "timefs initial import".into(),
            format!("{:.1}", ms(import)),
            st.new_objects.to_string(),
            human_bytes(st.new_bytes),
        ]);

        for &k in EDIT_COUNTS.iter().filter(|&&k| k <= n) {
            table.row(edit_snapshot_row(cfg, &repo, n, k));
        }
        if have("git") {
            table.row(git_row(&copy, n));
        }
        std::fs::remove_dir_all(&work)?;
    }
    table.print("Snapshot latency and space (Q1, Q2)");
    Ok(())
}

/// Edit `k` files in a fresh working tree, then time the full snapshot sequence.
fn edit_snapshot_row(cfg: &Config, repo: &Repository, n: usize, k: usize) -> Vec<String> {
    let (mut times, mut objects, mut bytes) = (Vec::new(), Vec::new(), Vec::new());
    for rep in 0..cfg.repeat {
        let tip = repo.branch_tip("main").unwrap().expect("tip");
        let tree = repo.get_commit(&tip).unwrap().tree;
        let mut wt = open_tree(repo, tree);
        for i in pick_files(n, k, 100 + rep as u64) {
            touch_file(&mut wt, i, 0xA0 + rep as u8);
        }
        let before = repo.store().stats();
        // The same sequence the mount uses: persist, commit, then one sync before the ref moves.
        let (_, d) = timed(|| {
            let root = wt.checkpoint().unwrap();
            if root != tree {
                repo.commit_to_branch("main", root, "bench", "edit")
                    .unwrap();
            }
        });
        let after = repo.store().stats();
        times.push(ms(d));
        objects.push((after.new_objects - before.new_objects) as f64);
        bytes.push((after.new_bytes - before.new_bytes) as f64);
    }
    vec![
        n.to_string(),
        format!("timefs snapshot after editing {k} file(s)"),
        Summary::of(&times).show(2),
        format!("{:.0}", Summary::of(&objects).median),
        human_bytes(Summary::of(&bytes).median as u64),
    ]
}

/// `git commit -a` after editing one file: the closest everyday equivalent.
fn git_row(dir: &std::path::Path, n: usize) -> Vec<String> {
    let git = |args: &[&str]| {
        let mut full = vec![
            "-c",
            "user.name=bench",
            "-c",
            "user.email=bench@example.com",
        ];
        full.extend_from_slice(args);
        run_ok("git", &full, Some(dir))
    };
    assert!(
        git(&["init", "-q"]) && git(&["add", "-A"]),
        "git setup failed"
    );
    assert!(
        git(&["commit", "-q", "-m", "initial"]),
        "git initial commit failed"
    );
    let first = dir.join(crate::gen::file_path(0));
    let mut data = std::fs::read(&first).unwrap();
    data[0] ^= 0xFF;
    std::fs::write(&first, data).unwrap();
    let (ok, d) = timed(|| git(&["commit", "-q", "-a", "-m", "edit"]));
    assert!(ok, "git commit -a failed");
    vec![
        n.to_string(),
        "git commit -a after editing 1 file".into(),
        format!("{:.1}", ms(d)),
        "n/a".into(),
        "n/a".into(),
    ]
}
