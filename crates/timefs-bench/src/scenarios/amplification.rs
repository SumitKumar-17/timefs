//! Q7: how many bytes does a checkpoint write per byte the application changed?

use std::sync::Arc;

use timefs_core::store::ObjectStore;
use timefs_core::worktree::{Options, WorkTree, ROOT_INO};
use timefs_core::Repository;

use super::Res;
use crate::env::{new_repo, Config};
use crate::gen::Rng;
use crate::report::{human_bytes, ms, timed, Table};

const FILE_SIZE: usize = 32 * 1024 * 1024;
const WRITE_SIZE: usize = 4096;
const BLOCK_SIZES: [usize; 4] = [4 * 1024, 16 * 1024, 64 * 1024, 256 * 1024];
const WRITE_COUNTS: [usize; 3] = [1, 16, 256];

pub fn run(cfg: &Config) -> Res {
    let mut table = Table::new(&[
        "block size",
        "random 4 KiB writes",
        "bytes changed",
        "bytes written by checkpoint",
        "write amplification",
        "checkpoint ms",
    ]);
    for block in BLOCK_SIZES {
        let repo = new_repo(cfg, &format!("amp-{block}"))?;
        let base = build_base(&repo, block);
        for k in WRITE_COUNTS {
            let (written, d) = measure(&repo, base, block, k);
            let changed = (k * WRITE_SIZE) as u64;
            table.row(vec![
                human_bytes(block as u64),
                k.to_string(),
                human_bytes(changed),
                human_bytes(written),
                format!("{:.1}x", written as f64 / changed as f64),
                format!("{:.2}", ms(d)),
            ]);
        }
    }
    table.print("Checkpoint write amplification, 32 MiB file (Q7)");
    Ok(())
}

fn options(block: usize) -> Options {
    Options {
        block_size: block,
        ..Options::default()
    }
}

fn open(repo: &Repository, root: Option<timefs_core::Hash>, block: usize) -> WorkTree {
    let store: Arc<dyn ObjectStore> = Arc::clone(repo.store());
    WorkTree::new(store, root, options(block))
}

/// Persist a 32 MiB file of random data and return its root tree.
fn build_base(repo: &Repository, block: usize) -> timefs_core::Hash {
    let mut wt = open(repo, None, block);
    let ino = wt.create(ROOT_INO, b"data", 0o644, 0, 0).unwrap().ino;
    let mut rng = Rng::new(5);
    for off in (0..FILE_SIZE).step_by(1 << 20) {
        wt.write(ino, off as u64, &rng.bytes(1 << 20)).unwrap();
    }
    let root = wt.checkpoint().unwrap();
    repo.store().sync().unwrap();
    root
}

/// Apply `k` random 4 KiB overwrites to a fresh tree and checkpoint; returns bytes written.
fn measure(
    repo: &Repository,
    base: timefs_core::Hash,
    block: usize,
    k: usize,
) -> (u64, std::time::Duration) {
    let mut wt = open(repo, Some(base), block);
    let ino = wt.lookup(ROOT_INO, b"data").unwrap().ino;
    let mut rng = Rng::new(k as u64);
    for _ in 0..k {
        let off = rng.below((FILE_SIZE - WRITE_SIZE) as u64);
        wt.write(ino, off, &rng.bytes(WRITE_SIZE)).unwrap();
    }
    let before = repo.store().stats();
    let (_, d) = timed(|| {
        wt.checkpoint().unwrap();
        repo.store().sync().unwrap();
    });
    (repo.store().stats().new_bytes - before.new_bytes, d)
}
