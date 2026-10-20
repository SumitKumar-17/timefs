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
| M4 | FUSE adapter: read-write mount, checkpoints, wip recovery, control socket | ☐ |
| M5 | Branches, 3-way merge, GC | ☐ |
| M6 | Benchmark suite, profiling, measured optimisations, results write-up | ☐ |
| M7 | Hardening: crash-injection tests, pjdfstest/fsx runs, docs polish, release | ☐ |

## Candidate future work (not committed)

* Packed object store (only if M6 shows loose-object overhead dominates, ADR-0003).
* Compression (zstd) per object.
* Line-level text merge for "both modified a regular file".
* Live `merge`/`checkout` against a mounted tree with kernel cache invalidation.
* Virtual read-only `/.snapshots/<id>/` view inside the mount.
* Two-level extent tree for very large files.
* Replication ("send the hashes the other side lacks").
* Extended attributes, hard links via inode objects.

## Progress log

Newest first. One line per notable change; link commits or benchmark runs when relevant.

* 2026-10-20, M3 done: WorkTree overlay split into modules; model-based test passes 5000 cases (70 unit tests).
* 2026-10-20, M2 done: refs, repository, import/export, diff, fsck, offline CLI (58 tests).
* 2026-10-20, M1 done: hash, canonical codec, objects, chunking (fixed + FastCDC), loose store.
* 2026-10-20, Project started. Architecture, format spec and ADRs written.
