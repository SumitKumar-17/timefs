//! Differential stress test (in the spirit of fsx): the same seeded random sequence of
//! filesystem operations runs against a TimeFS mount and against a plain host directory,
//! which serves as the oracle. Every operation must succeed or fail identically, and the
//! two trees must match, including across snapshots and a full unmount and remount.
//!
//! Defaults are sized for a quick CI run; long runs are configured through
//! `TIMEFS_STRESS_SEEDS`, `TIMEFS_STRESS_OPS` and `TIMEFS_STRESS_SEED` (the first seed).

mod common;
#[path = "differential/mod.rs"]
mod support;

use common::Env;

const DEFAULT_SEEDS: u64 = 3;
const DEFAULT_OPS: usize = 400;
/// Fixed so that the default run is the same everywhere and a CI failure replays locally.
const DEFAULT_FIRST_SEED: u64 = 0x7146_5eed;

fn env_number<T: std::str::FromStr>(name: &str, default: T) -> T {
    match std::env::var(name) {
        Ok(v) => v
            .parse()
            .unwrap_or_else(|_| panic!("{name} must be a number, got {v:?}")),
        Err(_) => default,
    }
}

#[test]
fn random_operations_match_a_host_directory() {
    let first = env_number("TIMEFS_STRESS_SEED", DEFAULT_FIRST_SEED);
    let seeds = env_number("TIMEFS_STRESS_SEEDS", DEFAULT_SEEDS);
    let ops = env_number("TIMEFS_STRESS_OPS", DEFAULT_OPS);
    for seed in (0..seeds).map(|i| first.wrapping_add(i)) {
        let env = Env::new();
        let mounted = mount_or_skip!(env);
        if let Err(failure) = support::runner::run(&env, mounted, seed, ops) {
            panic!(
                "seed {seed} diverged {failure}\nreplay with: TIMEFS_STRESS_SEED={seed} \
                 TIMEFS_STRESS_SEEDS=1 TIMEFS_STRESS_OPS={ops} \
                 cargo test -p timefs-fuse --test differential -- --test-threads=1"
            );
        }
    }
}
