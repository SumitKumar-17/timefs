# ADR-0008: Stop-the-world mark-and-sweep GC with grace period

**Status:** Accepted (implemented)

## Context
* Shared immutable objects need collection once refs move or branches are deleted.

## Decision
* Mark everything reachable from every ref (heads and wip).
* Delete unmarked objects whose mtime is older than the grace period (`--grace-secs`, default 3600).
* `--dry-run` reports without deleting.
* Requires the repository lock (not mounted), which keeps the algorithm simple and correct.

## Alternatives
* **Reference counting:** every put and ref move updates counts; recovering counts after a crash is hard.
* **Concurrent/incremental GC:** needs write barriers or generation tracking; deferred.

## Consequences
* GC cannot run while mounted.
* The grace period protects objects of an interrupted operation.
* Cost is O(total objects) per run; revisit with benchmarks.
