# ADR-0008: Stop-the-world mark-and-sweep GC with grace period

**Status:** Accepted

## Context
Immutable shared objects need garbage collection once refs move or branches are deleted.

## Decision
Mark all objects reachable from every ref (heads and wip), then delete unmarked objects
whose mtime is older than a grace period (default 1 hour). v1 requires the repository lock
(i.e. not mounted), which makes the algorithm trivially correct.

## Alternatives
* **Reference counting**: every put/ref-move updates counts; crash recovery of counts is hard.
* **Concurrent/incremental GC**: needs write barriers or generation tracking; deferred.

## Consequences
* GC cannot run while mounted; documented. The grace period guards against deleting
  objects of an interrupted operation.
* Cost is O(total objects) per run; fine for the target scale, revisit with benchmarks.
