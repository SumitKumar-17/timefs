//! Q4 and Q5: deduplication on realistic data, and fixed versus content-defined chunks.

use std::fs;
use std::path::Path;

use timefs_core::chunk::Chunker;
use timefs_core::hostfs::import_dir;
use timefs_core::Repository;

use super::Res;
use crate::env::{have, new_repo, run_ok, Config};
use crate::gen::Rng;
use crate::report::{human_bytes, Table};

const IMAGE_SIZE: usize = 32 * 1024 * 1024;
const EDIT_ROUNDS: usize = 8;
const OVERWRITES_PER_ROUND: usize = 20;

fn chunkers() -> Vec<(&'static str, Chunker)> {
    vec![
        ("fixed 4K", Chunker::Fixed { size: 4096 }),
        ("fixed 64K", Chunker::Fixed { size: 65536 }),
        ("cdc avg 4K", Chunker::cdc(4096)),
        ("cdc avg 16K", Chunker::cdc(16384)),
        ("cdc avg 64K", Chunker::cdc(65536)),
    ]
}

pub fn run(cfg: &Config) -> Res {
    edit_series(cfg)?;
    git_history(cfg)
}

/// A large file edited repeatedly: in-place overwrites, then insertions.
fn edit_series(cfg: &Config) -> Res {
    let mut table = Table::new(&[
        "chunker",
        "initial stored",
        "added per overwrite round (20 x 1 KiB)",
        "added per insert round (20 x 100 B)",
    ]);
    for (name, chunker) in chunkers() {
        let work = cfg.scratch("dedupe-image")?;
        let repo = new_repo(cfg, "dedupe-image-repo")?;
        let file = work.join("image.bin");
        let mut rng = Rng::new(11);
        let mut data = rng.bytes(IMAGE_SIZE);

        fs::write(&file, &data)?;
        let initial = snapshot(&repo, &work, &chunker);
        let (mut over, mut ins) = (Vec::new(), Vec::new());
        for _ in 0..EDIT_ROUNDS {
            for _ in 0..OVERWRITES_PER_ROUND {
                let off = rng.below((IMAGE_SIZE - 1024) as u64) as usize;
                let patch = rng.bytes(1024);
                data[off..off + 1024].copy_from_slice(&patch);
            }
            fs::write(&file, &data)?;
            over.push(snapshot(&repo, &work, &chunker));
        }
        for _ in 0..EDIT_ROUNDS {
            for _ in 0..OVERWRITES_PER_ROUND {
                let off = rng.below(data.len() as u64) as usize;
                data.splice(off..off, rng.bytes(100));
            }
            fs::write(&file, &data)?;
            ins.push(snapshot(&repo, &work, &chunker));
        }
        table.row(vec![
            name.to_string(),
            human_bytes(initial),
            human_bytes(mean(&over)),
            human_bytes(mean(&ins)),
        ]);
    }
    table.print("Space added per snapshot of a 32 MiB file, by chunker (Q5)");
    Ok(())
}

fn mean(v: &[u64]) -> u64 {
    v.iter().sum::<u64>() / v.len().max(1) as u64
}

/// Import `dir` and return the bytes of new objects it added.
fn snapshot(repo: &Repository, dir: &Path, chunker: &Chunker) -> u64 {
    let before = repo.store().stats().new_bytes;
    import_dir(repo.store().as_ref(), dir, chunker).expect("import");
    repo.store().stats().new_bytes - before
}

/// The last revisions of a real git repository (this one by default), imported in order.
fn git_history(cfg: &Config) -> Res {
    if !have("git") || !have("tar") {
        println!("\n(git history dedupe skipped: git or tar not available)");
        return Ok(());
    }
    let revs = git_revisions(25);
    if revs.len() < 2 {
        println!("\n(git history dedupe skipped: fewer than two commits)");
        return Ok(());
    }
    let mut table = Table::new(&[
        "chunker",
        "revisions",
        "logical bytes (sum)",
        "stored bytes",
        "ratio",
    ]);
    for (name, chunker) in chunkers() {
        let repo = new_repo(cfg, "dedupe-git-repo")?;
        let (mut logical, mut stored) = (0u64, 0u64);
        for rev in &revs {
            let dir = cfg.scratch("dedupe-git-src")?;
            if !extract_revision(rev, &dir) {
                return Err(format!("could not extract revision {rev}").into());
            }
            let before = repo.store().stats().new_bytes;
            let (_, stats) = import_dir(repo.store().as_ref(), &dir, &chunker)?;
            logical += stats.bytes;
            stored += repo.store().stats().new_bytes - before;
        }
        table.row(vec![
            name.to_string(),
            revs.len().to_string(),
            human_bytes(logical),
            human_bytes(stored),
            format!("{:.2}x", logical as f64 / stored as f64),
        ]);
    }
    table.print("Deduplication across the last commits of a real git history (Q4)");
    Ok(())
}

/// Oldest-first revision ids of the repository containing the current directory.
fn git_revisions(count: usize) -> Vec<String> {
    let out = std::process::Command::new("git")
        .args(["rev-list", "--max-count", &count.to_string(), "HEAD"])
        .output();
    let Ok(out) = out else { return Vec::new() };
    let mut revs: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    revs.reverse();
    revs
}

fn extract_revision(rev: &str, dest: &Path) -> bool {
    let cmd = format!("git archive {rev} | tar -x -C {}", dest.display());
    run_ok("sh", &["-c", &cmd], None)
}
