# Testing strategy

| Layer | Technique | Where |
|-------|-----------|-------|
| Codec, hashing, chunking | Unit + property tests (round-trip, canonical-form rejection, chunker covers input exactly) | `timefs-core` |
| Object store | Unit tests, concurrent put, corruption detection | `timefs-core` |
| Repository / refs | Unit tests incl. fault injection (leftover temp files, torn refs) | `timefs-core` |
| WorkTree | Model-based property tests: random POSIX operations on the WorkTree and an in-memory model must agree, including after checkpoint and reload | `timefs-core/tests/worktree_model` |
| Diff / merge | Unit + property tests (one-sided merges, merge with itself, disjoint changes) | `timefs-core` |
| GC / fsck | Create garbage, check reachable data survives; corrupt an object, check detection | `timefs-core` |
| CLI | End-to-end tests of the `timefs` binary | `timefs-cli/tests/cli.rs` |
| FUSE | Real mounts driven with ordinary `std::fs` calls; skipped when `/dev/fuse` is unavailable | `timefs-fuse/tests/mount.rs` |
| Crash injection | SIGKILL the real `timefs mount` daemon during a seeded random workload; then `fsck`, remount and require the recovered tree to be a state from the last acknowledged sync onward | `timefs-cli/tests/crash.rs` |
| Conformance | pjdfstest / fsx / fsstress against a mount (manual or scheduled; M7, not started) | `scripts/` |

## Running

* All tests: `cargo test --workspace`.
* Crash injection: `TIMEFS_REQUIRE_FUSE=1 cargo test -p timefs-cli --test crash -- --test-threads=1`
  (3 rounds of 3 crashes each). Longer runs: `TIMEFS_CRASH_ROUNDS=30`; replay a failing round
  with the seed it prints: `TIMEFS_CRASH_SEED=<seed> TIMEFS_CRASH_ROUNDS=1`.
* CI jobs: `check` (fmt, clippy with `-D warnings`, tests) and `fuse-integration` (real mount).

## Rules

* A bug fix lands with a regression test.
* FUSE tests skip (not fail) without FUSE; CI still runs them on a runner with `/dev/fuse`.
* No test depends on wall-clock timing for correctness.
* Conformance and benchmark results are recorded only from real runs.
