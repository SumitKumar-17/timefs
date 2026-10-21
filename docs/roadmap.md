# Roadmap and progress log

Status: ☐ not started · ◐ in progress · ☑ done. This file is the source of truth for
project state; update it in the same commit that changes the state.

## Milestones

| # | Milestone | Status |
|---|-----------|--------|
| M0 | Repo, docs, ADRs, CI skeleton | ☑ |
| M1 | Core: hash, codec, object store (loose), chunking, blob/file objects, tests | ☑ |
| M2 | Core: trees, commits, refs, repository, import/export, restore, diff, `fsck`, CLI offline commands | ☑ |
| M3 | `WorkTree` overlay (all POSIX ops in-memory) with property tests against a model | ☑ |
| M4 | FUSE adapter: read-write mount, checkpoints, wip recovery, control socket | ☑ |
| M5 | Branches, 3-way merge, GC | ☑ |
| M6 | Benchmark suite, profiling, measured optimisations, results write-up | ☑ |
| M7 | Hardening: crash-injection tests, pjdfstest/fsx runs, docs polish, release | ◐ |

## Candidate future work (not committed)

* Extent pages for large files (ADR-0009 part 2); lifts the 64 GiB file cap.
* Concurrent writers on a pack repository (route offline ref changes through the mount).
* Compression (zstd) per object.
* Line-level text merge for "both modified a regular file".
* Live `merge`/`checkout` against a mounted tree with kernel cache invalidation.
* Virtual read-only `/.snapshots/<id>/` view inside the mount.
* Replication ("send the hashes the other side lacks").
* Extended attributes, hard links via inode objects.

## Progress log

Newest first. One line per notable change; link commits or benchmark runs when relevant.

* 2026-10-21, M7: pack store (ADR-0009 part 1) as default, migrate command, SIGKILL crash test on both stores.
* 2026-10-21, M7: fsx-style differential stress test against a host oracle; pjdfstest run by hand; lookup now returns ENAMETOOLONG.
* 2026-10-20, M6 done: results in benchmarks.md; review fixes landed (150 tests); M7 hardening started.
* 2026-10-20, M6: parallel fsync, read cache, kernel page cache; measured before/after in benchmarks.md.
* 2026-10-20, M5 done: 3-way merge, GC, checkout, reset, offline snap (116 tests).
* 2026-10-20, M4 done: read-write FUSE mount, checkpoints, wip recovery, control socket (93 tests, 8 on a real mount).
* 2026-10-20, M3 done: WorkTree overlay split into modules; model-based test passes 5000 cases (70 unit tests).
* 2026-10-20, M2 done: refs, repository, import/export, diff, fsck, offline CLI (58 tests).
* 2026-10-20, M1 done: hash, canonical codec, objects, chunking (fixed + FastCDC), loose store.
* 2026-10-20, Project started. Architecture, format spec and ADRs written.
