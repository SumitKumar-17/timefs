//! Helpers shared by several scenarios.

use std::path::Path;

use timefs_core::worktree::{Ino, WorkTree, ROOT_INO};

use crate::gen::{file_path, Rng};

/// Resolve a relative path to an inode by walking `lookup`.
pub fn resolve(wt: &mut WorkTree, path: &Path) -> Ino {
    let mut cur = ROOT_INO;
    for part in path.components() {
        let name = part.as_os_str().to_string_lossy().into_owned();
        cur = wt.lookup(cur, name.as_bytes()).expect("path exists").ino;
    }
    cur
}

/// Overwrite the first byte of generated file `i` (a minimal, real modification).
pub fn touch_file(wt: &mut WorkTree, i: usize, byte: u8) {
    let ino = resolve(wt, &file_path(i));
    wt.write(ino, 0, &[byte]).expect("write");
}

/// `k` distinct file indexes below `n`, deterministic for a seed.
pub fn pick_files(n: usize, k: usize, seed: u64) -> Vec<usize> {
    let mut rng = Rng::new(seed);
    let mut chosen = std::collections::BTreeSet::new();
    while chosen.len() < k.min(n) {
        chosen.insert(rng.below(n as u64) as usize);
    }
    chosen.into_iter().collect()
}
