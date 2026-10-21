# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

## [0.1.0] - 2026-10-21

First release.

### Added

* Pack store (format 2, now the default): objects and refs in one append-only log, one
  `fdatasync` per checkpoint, torn-tail trimming, sync markers that turn damage of durable
  data into a loud error, single-writer lock, compaction as GC (ADR-0009).
* `Refs` trait whose `write` makes objects durable first; `FileRefs` for the loose layout.
* Paged extent lists (`FileIndex`, object kind 5) and hole extents: a small edit to a big file
  rewrites one page; sparse files cost one extent per hole; files up to 1 PiB.
* `timefs init --store pack|loose`, `timefs migrate <dest> --store ...`, store kind in `stats`.
* `timefs branch <name>` works while a pack repository is mounted (sent to the mount).
* SIGKILL crash-injection test that alternates both stores (`crates/timefs-cli/tests/crash.rs`).

* Design docs: motivation, architecture, canonical on-disk format v1, ADRs, testing
  and benchmark methodology, roadmap, contributor guide, CI skeleton.
* Core objects: BLAKE3 identity, canonical binary codec (rejects non-minimal varints and
  trailing bytes), Blob, File, Tree and Commit objects with validating decoders.
* Chunking: fixed-size and FastCDC chunkers.
* Loose object store with atomic writes, read verification, dedupe counters and batched fsync.
* Refs with atomic writes (tmp, fsync, rename, fsync dir).
* Repository: init/open, flock, HEAD, branches, revision parsing, log, merge base.
* Host directory import/export, path lookup, tree walk and structural diff.
* Reachability marking and `fsck` integrity check.
* WorkTree overlay with an extent engine (stored, in-memory and hole segments); checkpoints
  rewrite only dirty paths; unlinked inodes stay valid until forget and release.
* Read-write FUSE mount (libfuse3 via `fuser`) as a thin adapter over WorkTree.
* Checkpoints to `refs/wip/<branch>`, snapshots to the branch, crash recovery on next mount.
* Periodic checkpoint thread with inline flush on a dirty threshold.
* Unix control socket with `snap`, `checkpoint` and `status`.
* Tree-level 3-way merge with explicit conflict types; conflicts abort untouched by default,
  `--resolve ours|theirs` settles them.
* Mark-and-sweep GC with a grace period; refuses damaged repositories.
* `promote_checkpoint` turns a saved checkpoint into a commit.
* CLI: `init`, `import`, `export`, `log`, `ls`, `cat`, `diff`, `branch`, `fsck`, `stats`,
  `mount` (graceful unmount on SIGINT/SIGTERM), `snap` (live or offline), `checkpoint`,
  `status`, `merge`, `checkout`, `reset`, `gc`; lock and checkpoint guards.
* `CachedStore`: a two-generation read cache for immutable objects.
* Benchmark harness: deterministic generators, median/min/max reports as Markdown tables.
* Core benchmark scenarios: snapshot (vs `cp -r` and git), branch/restore, merge, write
  amplification, loose store overhead, dedupe and chunker comparison.
* Mount benchmark workloads, a passthrough FUSE baseline and an fsync phase breakdown.
* Tests: unit tests, a model-based WorkTree property test, CLI black-box tests and real-mount
  FUSE integration tests.

### Changed

* Docs rewritten as bullets; README, architecture and ADRs brought up to date.
* WorkTree split into modules: error, node, extents, dir_ops, file_ops, persist, lifetime.
* Hex decoding uses `as_chunks` (fixes a clippy lint on newer stable toolchains).

### Fixed

* Checkpoint of a never-persisted empty tree (found by the model-based test).
* chmod, chown or touch on a file or symlink that was never read made every later checkpoint
  fail; metadata-only changes now reuse the existing content object. The model test gained a
  chmod operation and catches the old behaviour.
* A torn object left by a crash was trusted by deduplication; `put` now checks the stored
  length, rewrites a torn object, and fsyncs reused objects before a ref names them.
* A failed `sync` dropped its pending list, so a later sync could report success over
  objects that were never made durable.
* Unlinked-but-open files kept their data in memory forever; checkpoints now spill it.
* Files larger than the size limit are refused with `EFBIG` instead of hanging a checkpoint
  (the limit is now 1 PiB, see Added).
* A mount no longer overwrites a branch that moved underneath it (checkpoint and snapshot
  refuse), and a checkpoint equal to the branch state is cleared instead of written.
* Unlinked files report `nlink` 0; `RENAME_WHITEOUT` is rejected with `EINVAL`.
* Looking up a name longer than 255 bytes returned `ENOENT`; it now returns `ENAMETOOLONG`
  like native filesystems (found by the differential stress test and pjdfstest).

### Performance

* Loose store fsyncs objects in parallel over scoped threads; engine syncs once per snapshot.
  * Measured on ext4 (SATA SSD), 1000 files: initial import 4792 ms to 662 ms.
  * 100-file edit snapshot 471 ms to 70 ms, 1-file edit 21 ms to 9.5 ms.
* Loose store syncs files then directories in two parallel fsync rounds (A/B measured).
* Read cache and `FOPEN_KEEP_CACHE` (every write goes through the mount).
  * Warm sequential read 821 to 6539 MB/s, cold 4K random read 15.3k to 54.4k IOPS.
  * fsync latency still 9.4 ms (ref write needs 2 fsync rounds, see `docs/benchmarks.md`).
