//! A tiny deterministic PRNG (SplitMix64).
//!
//! The test must replay a failure from its seed alone, so it cannot depend on an external
//! crate whose output might change between versions.

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform-ish value in `0..n`; the modulo bias is irrelevant for a stress test.
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }

    /// True with probability `1 / n`.
    pub fn one_in(&mut self, n: u64) -> bool {
        self.below(n) == 0
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }

    pub fn fill(&mut self, buf: &mut [u8]) {
        for chunk in buf.chunks_mut(8) {
            let word = self.next_u64().to_le_bytes();
            chunk.copy_from_slice(&word[..chunk.len()]);
        }
    }
}

/// Write payloads are regenerated from a small seed so that a logged operation stays short.
/// Seed 0 means all zeros, which exercises the zero-block and hole paths.
pub fn payload(len: usize, seed: u64) -> Vec<u8> {
    let mut data = vec![0u8; len];
    if seed != 0 {
        Rng::new(seed).fill(&mut data);
    }
    data
}
