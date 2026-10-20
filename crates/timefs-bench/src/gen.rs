//! Deterministic data generation, so every benchmark run sees identical bytes.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// xorshift64*: small, fast and good enough for test data.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1) ^ 0x9E37_79B9_7F4A_7C15)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform value in `0..n` (`n > 0`).
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }

    pub fn fill(&mut self, buf: &mut [u8]) {
        for chunk in buf.chunks_mut(8) {
            let v = self.next_u64().to_le_bytes();
            chunk.copy_from_slice(&v[..chunk.len()]);
        }
    }

    pub fn bytes(&mut self, len: usize) -> Vec<u8> {
        let mut v = vec![0u8; len];
        self.fill(&mut v);
        v
    }
}

/// Files per directory at each level of the generated tree.
const FANOUT: usize = 100;

/// Relative path of file `i` in a generated tree (`d00/d07/f000731`).
pub fn file_path(i: usize) -> PathBuf {
    PathBuf::from(format!(
        "d{:02}/d{:02}/f{:06}",
        i / (FANOUT * FANOUT),
        (i / FANOUT) % FANOUT,
        i
    ))
}

#[derive(Clone, Copy, Debug)]
pub struct TreeSpec {
    pub files: usize,
    /// Mean file size in bytes; actual sizes vary between 0.5x and 1.5x.
    pub file_size: usize,
    pub seed: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct TreeInfo {
    pub bytes: u64,
}

/// Content of generated file `i`: deterministic and unique per file.
pub fn file_content(spec: &TreeSpec, i: usize) -> Vec<u8> {
    let mut rng = Rng::new(spec.seed ^ (i as u64).wrapping_mul(0x1000_0000_01B3));
    let size = spec.file_size / 2 + rng.below(spec.file_size.max(1) as u64) as usize;
    rng.bytes(size)
}

/// Write a tree of `spec.files` files under `root` (which must not exist).
pub fn build_tree(root: &Path, spec: &TreeSpec) -> io::Result<TreeInfo> {
    let mut bytes = 0u64;
    for i in 0..spec.files {
        let path = root.join(file_path(i));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = file_content(spec, i);
        bytes += data.len() as u64;
        fs::write(path, data)?;
    }
    Ok(TreeInfo { bytes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_deterministic_and_spread() {
        let (mut a, mut b) = (Rng::new(7), Rng::new(7));
        assert_eq!(a.bytes(100), b.bytes(100));
        assert_ne!(Rng::new(1).bytes(16), Rng::new(2).bytes(16));
        let mut r = Rng::new(3);
        assert!((0..1000).all(|_| r.below(10) < 10));
    }

    #[test]
    fn paths_fan_out() {
        assert_eq!(file_path(0), Path::new("d00/d00/f000000"));
        assert_eq!(file_path(10_731), Path::new("d01/d07/f010731"));
    }

    #[test]
    fn content_is_stable_and_distinct() {
        let spec = TreeSpec {
            files: 3,
            file_size: 100,
            seed: 1,
        };
        assert_eq!(file_content(&spec, 1), file_content(&spec, 1));
        assert_ne!(file_content(&spec, 1), file_content(&spec, 2));
    }
}
