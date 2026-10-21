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
| Differential stress | fsx-style: one seeded random sequence of std::fs operations runs on a mount and on a host directory (the oracle); every result and the final trees must match, across snapshots and a remount | `timefs-fuse/tests/differential.rs` |
| Conformance | pjdfstest against a mount (manual, see below) | outside the repo |

## Running

* All tests: `cargo test --workspace`.
* CI jobs: `check` (fmt, clippy with `-D warnings`, tests) and `fuse-integration` (real mount).

## Differential stress test

* Default (part of `cargo test`): 3 seeds x 400 operations, a few seconds.
* Long run: `TIMEFS_REQUIRE_FUSE=1 TIMEFS_STRESS_SEEDS=20 TIMEFS_STRESS_OPS=2000 cargo test -p timefs-fuse --test differential -- --test-threads=1`.
* `TIMEFS_STRESS_SEED` sets the first seed; a failure prints its seed, the last steps and the
  exact command to replay it.
* Operations: writes at random offsets (holes past EOF), ranged reads, append, `O_TRUNC`,
  `set_len`, mkdir, rmdir, unlink, rename (over targets, into subdirectories), symlink,
  readlink, chmod, fsync, stat, readdir, and handles held open across unlink and rename.
  TimeFS also gets `snap` and `checkpoint` over the control socket and one unmount/remount.
* Compared: errno of every call, read data, and the trees (names, types, permission bits,
  contents, symlink targets).
* Not compared, by design: timestamps (only mtime is stored; atime and ctime follow it),
  inode numbers and link counts (directory nlink is 2), directory sizes, root directory
  metadata (not persisted). Hard links, special files and `RENAME_EXCHANGE` are not generated.

## pjdfstest (manual)

* Build it outside the repo (`autoreconf -ifs && ./configure && make pjdfstest`), mount a
  scratch repository with `timefs mount`, then run `prove -r <pjdfstest>/tests` inside the mount.
* Most tests need root; as an ordinary user compare against the same run on a host directory
  and look only at the failures that TimeFS adds.
* Expected TimeFS-only failures: hard links, fifos, device nodes and sockets (unsupported),
  atime and ctime checks (follow mtime), directory nlink.

## Rules

* A bug fix lands with a regression test.
* FUSE tests skip (not fail) without FUSE; CI still runs them on a runner with `/dev/fuse`.
* No test depends on wall-clock timing for correctness.
* Conformance and benchmark results are recorded only from real runs.
