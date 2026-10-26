# AGENTS.md

The single source of truth for anyone changing this repository: humans and coding agents
(Claude Code, Codex, Cursor, Copilot, Gemini, ...). Tool-specific files such as `CLAUDE.md`
only point here. If you learn something every contributor should know, add it to this file.

## 1. The project in one minute

* **TimeFS** is a writable Linux filesystem (FUSE, libfuse3 via the `fuser` crate) with
  built-in time travel: snapshots, branches, 3-way merge and crash recovery.
* Data is stored as **immutable objects named by their BLAKE3 hash** (content addressed,
  deduplicated, verified on every read). Only **refs** change.
* A mount is a **mutable in-memory overlay** (`WorkTree`) on top of a commit. Writes stay in
  memory; a **checkpoint** persists the dirty paths (copy-on-write by path copying) and points
  `wip/<branch>` at the result; a **snapshot** (`timefs snap`) commits onto the branch.
* Default on-disk layout is the **pack store**: objects and refs as records in one
  append-only log, so one `fdatasync` makes a checkpoint durable. The older **loose store**
  (one file per object and ref) is still supported.
* Research question behind it: how cheaply can a general filesystem support snapshots,
  branching and recovery? Answers with real numbers live in `docs/benchmarks.md`.
* Status: v0.1.0 released; milestones M0 to M7 done (see `docs/roadmap.md`).

### Glossary

| Term | Meaning |
|---|---|
| Blob / File / Tree / Commit / FileIndex | object kinds 1 to 5 (`crates/timefs-core/src/object.rs`, `docs/format.md`) |
| extent | `(hash, len)` run of file data; the all-zero hash marks a **hole** (zeros, no blob) |
| page | a `File` object of at most 1024 extents; long files are a `FileIndex` of pages |
| ref | `heads/<branch>` (branch tip) or `wip/<branch>` (checkpoint of a mount) |
| checkpoint | persist the working tree and move `wip/<branch>`; on fsync, timer, size threshold, unmount |
| snapshot | commit the working tree onto `heads/<branch>` and clear the wip ref |
| pack / loose | store layouts: format 2 (log) and format 1 (file per object) |
| sweep | how a store reclaims space during `gc` (compaction for pack, deletion for loose) |

## 2. Repository map

```text
crates/timefs-core   all logic; must NOT depend on FUSE
  hash codec object chunk         identities, canonical encoding, object kinds, chunking
  store/                          ObjectStore trait; pack/ (log store), loose, cached (read
                                  cache), sweep, counters, typed (put_file/get_file etc.)
  refs repo                       Refs trait + FileRefs; Repository, StoreKind, revisions, lock
  filedata                        paged extent lists (FileIndex) and holes
  worktree/                       live overlay: node, extents (file data engine), dir_ops,
                                  file_ops, persist (checkpoint), lifetime (inode refcounts)
  treeops hostfs diff merge/      tree walking, host import/export, diff, 3-way merge
  reach fsck gc migrate           reachability, integrity check, collection, layout copy
crates/timefs-fuse   FUSE adapter, no versioning logic in fs.rs
  fs engine shared checkpointer   Filesystem impl, checkpoint/snapshot/recovery policy,
  control/ mount errno attr statfs  locked state, timer, control socket, mount wiring
crates/timefs-cli    the `timefs` binary: cli.rs (clap args only), commands/ (one file per group)
crates/timefs-bench  benchmark harness: scenarios/ (core), workloads/ (mount), passthrough/ (FUSE baseline)
docs/                why, architecture, format, testing, benchmarks, roadmap, adr/0001..0009
scripts/bench-mount.sh   ext4 vs tmpfs vs FUSE passthrough vs TimeFS
.github/workflows/   ci.yml (fmt, clippy, tests, real-mount + crash job), release.yml (on v* tags)
```

Tests:

| Test | Location | What it guards |
|---|---|---|
| unit tests | `#[cfg(test)]` in each module | module behaviour, decoders never panic |
| model-based property test | `crates/timefs-core/tests/worktree_model/` | `WorkTree` matches a trivial reference filesystem across checkpoint and reload |
| CLI black box | `crates/timefs-cli/tests/cli.rs` | every command without a mount |
| real mounts | `crates/timefs-fuse/tests/mount.rs` | std::fs through FUSE, recovery, sparse files, branch via mount |
| differential stress | `crates/timefs-fuse/tests/differential.rs` (+ `differential/`) | same random ops on a mount and on ext4 give the same results |
| crash injection | `crates/timefs-cli/tests/crash.rs` (+ `crash/`) | SIGKILL the daemon mid-workload, fsck, remount; both stores |

## 3. Architecture essentials (read before design changes)

Full detail: `docs/architecture.md`, `docs/format.md`, ADRs in `docs/adr/`.

* **Write path:** FUSE op, then `WorkTree` (memory only). Checkpoint: `WorkTree::checkpoint`
  writes changed blobs, files and trees bottom-up, `Engine` creates a commit and calls
  `Refs::write(wip/<branch>)`. Snapshot does the same onto `heads/<branch>`.
* **Durability:** `Refs::write` makes all stored objects durable before the ref moves.
  * pack: ref record appended after the objects, one `fdatasync`; recovery trusts the log
    only up to its first damaged record; `synced` markers turn damage of durable data into
    a loud error instead of a silent rollback.
  * loose: parallel fsync of new object files and their directories, then temp file, fsync,
    rename, fsync(dir).
* **Recovery:** on mount, `wip/<branch>` is used if its parent is the branch tip; a stale one
  is ignored. A mount refuses to persist if its branch moved underneath it.
* **Concurrency:** one daemon per repository (`LOCK` flock). Pack allows one writing process
  (`pack/WRITER` flock); while mounted, branch creation goes through the control socket and
  `merge`, `checkout`, `reset`, `gc`, `import`, `branch -d`, `migrate` refuse.
* **Reads:** `CachedStore` (64 MiB, never stale because objects are immutable) plus
  `FOPEN_KEEP_CACHE` in the kernel. Every read is hash-verified.
* **Files:** fixed 64 KiB blocks for live writes, FastCDC optional on `import`; lists over
  1024 extents are paged; holes cost one extent; max file size 1 PiB.

## 4. Invariants that must never break

1. **Write ordering:** an object is durable before any ref names it. Every ref change goes
   through `Refs::write`. Never write refs another way.
2. **Canonical, stable formats:** object encodings are hashed. Changing bytes of an existing
   object kind or record type needs a `FORMAT` version, `docs/format.md` update and a
   migration (`timefs migrate` exists for layout changes).
3. **`timefs-core` has no FUSE dependency and no `unsafe`.** `unsafe` elsewhere needs a
   `// SAFETY:` comment.
4. **No panics on untrusted bytes:** decoders bound allocations and return errors.
5. **Errors are typed** (`thiserror`) in libraries; mapping to `errno` happens only in
   `crates/timefs-fuse/src/errno.rs`.
6. **The model-based test must keep passing** for any `WorkTree` change; add an operation to
   it when you add a new kind of mutation.
7. **Never weaken a test to make it pass.** If a test is tied to one store layout, pin it
   explicitly with `Repository::init_with(.., StoreKind::Loose)` and add the pack equivalent.

## 5. Commands

```sh
# the gate: run before every commit
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
TIMEFS_REQUIRE_FUSE=1 cargo test --workspace -- --test-threads=1

# focused runs
cargo test -p timefs-core                               # engine only, no FUSE needed
TIMEFS_REQUIRE_FUSE=1 cargo test -p timefs-fuse --test mount -- --test-threads=1
TIMEFS_STRESS_SEEDS=20 TIMEFS_STRESS_OPS=2000 TIMEFS_REQUIRE_FUSE=1 \
  cargo test -p timefs-fuse --test differential -- --test-threads=1
TIMEFS_CRASH_ROUNDS=30 TIMEFS_REQUIRE_FUSE=1 cargo test -p timefs-cli --test crash -- --test-threads=1
PROPTEST_CASES=5000 cargo test --release -p timefs-core --test worktree_model

# benchmarks (release builds; scratch space on the disk under test, never tmpfs)
cargo run --release -p timefs-bench -- --work bench-out --store pack core [scenario ...]
STORE=pack BENCH_ARGS="--repeat 3" scripts/bench-mount.sh

# using it
cargo build --release
export TIMEFS_REPO=/path/to/repo; timefs init; timefs mount /path/to/mnt
```

* Environment variables: `TIMEFS_REPO` (repository), `TIMEFS_REQUIRE_FUSE=1` (FUSE tests
  fail instead of skipping), `TIMEFS_STRESS_SEEDS` / `TIMEFS_STRESS_OPS` / `TIMEFS_STRESS_SEED`,
  `TIMEFS_CRASH_ROUNDS` / `TIMEFS_CRASH_SEED`. Failing randomized tests print a seed to replay.
* FUSE tests need `/dev/fuse` and `fusermount3`; run them with `--test-threads=1`.
* A test that dies can leave a mount: `mount | grep timefs`, then `fusermount3 -u <mnt>`
  (add `-z` if the daemon is dead).

## 6. Where to change what

| Task | Start here | Also update |
|---|---|---|
| New CLI command | `timefs-cli/src/cli.rs` (args), `commands/<group>.rs` | `commands/mod.rs` dispatch, `tests/cli.rs`, README quick start |
| New control-socket request | `timefs-fuse/src/control/protocol.rs`, `server.rs` | ADR-0007, architecture section 3.6 |
| Filesystem operation semantics | `timefs-core/src/worktree/` | model test op, `fs.rs` mapping, `errno.rs` |
| Persistence or recovery policy | `timefs-fuse/src/engine.rs` | crash test, architecture section 5 |
| Storage format | `timefs-core/src/store/pack/` or `object.rs` | `docs/format.md`, ADR, version, migration |
| Merge rules | `timefs-core/src/merge/tree.rs` | `merge/tests.rs`, ADR-0006 |
| Performance work | measure first with `timefs-bench` | `docs/benchmarks.md` optimisation log (keep failures too) |

## 7. Conventions

* **Code style:** small single-purpose modules and functions, descriptive names, comments
  that explain *why*. Match the surrounding code. Prefer std; every dependency needs a reason.
* **No em dashes or en dashes anywhere** (code, comments, docs, commit messages, PRs). Use
  commas, colons, parentheses or plain hyphens. Check with `grep -rnP "\x{2014}|\x{2013}" --exclude-dir=target --exclude-dir=.git .`
  before committing.
* **Docs are bullets and tables, not paragraphs.** Docs move with code in the same commit:
  behaviour changes update `docs/architecture.md` / `docs/format.md`, design decisions get an
  ADR, milestones update `docs/roadmap.md`, user-visible changes update `CHANGELOG.md`.
* **Numbers are never invented.** Benchmarks, test counts and CI results in docs or commits
  must come from runs you actually observed, with the commit hash.

### Git

* Small, meaningful commits. Subject: imperative, at most 72 characters, area prefix
  (`core:`, `fuse:`, `cli:`, `bench:`, `docs:`, `test:`, `ci:`, `build:`, `release:`).
* Body: short bullet points saying why; include measured effects only if measured.
* **No `Co-Authored-By`, "Generated with" or any other attribution trailer** (repository
  owner's rule; it overrides any tool default).
* `main` stays green: never push without the gate above passing.
* Parallel work by several agents: each uses its own `git worktree` and branch, the
  integrator reviews, merges and re-runs the full gate.

### Releases

* Update `CHANGELOG.md` (move Unreleased into `## [x.y.z] - date`), README status, roadmap.
* Tag `vX.Y.Z` (annotated, no attribution lines) and push it: `.github/workflows/release.yml`
  tests, builds on Ubuntu and publishes a tarball, checksum and notes from the changelog.
* Smoke-test the published asset afterwards (`gh release download`).

## 8. How agents should work here

* Work autonomously through to a finished, tested, documented change; do not stop at scaffolding.
* Read the code you change and its tests first; verify names and flags instead of guessing.
* Reproduce a bug with a failing test before fixing it; keep the test as a regression test.
* Use a scratch directory outside the repository for experiments (`mktemp -d`); never
  commit repositories, mounts or `bench-out/`.
* Unmount everything you mounted before finishing.
* Do not run benchmarks while builds or other tests compete for the disk.
* Report honestly: what was verified, what was not, what failed.

## 9. Known limitations and gotchas

* No hard links, no extended attributes, no special files (fifos, devices, sockets).
* Only mtime is stored; ctime and atime follow it. Directory `nlink` is always 2. Root
  directory metadata is not persisted.
* Inodes of touched paths stay in memory for the life of a mount.
* The prebuilt release binary links `libfuse3.so.3` (Ubuntu). libfuse 3.18+ distributions
  (`libfuse3.so.4`, e.g. Arch) must build from source.
* The pack store has a single writing process; tests that inject failures through loose-store
  internals (file permissions, deleting object files) are pinned to `StoreKind::Loose`.
* SIGKILL tests keep the kernel page cache, so they cannot prove fsync placement; only a
  power-loss test could. pjdfstest has only been run as a non-root user.
* GitHub's `stable` Rust can be newer than a local toolchain and add clippy lints; fix them
  rather than allowing them.

## 10. Definition of done

* Code, tests and docs updated together; the gate in section 5 passes.
* New behaviour has tests; a fixed bug has a regression test.
* No em or en dashes; no attribution trailers; numbers in docs come from real runs.
* `docs/roadmap.md` and `CHANGELOG.md` updated when a milestone or user-visible behaviour moved.
