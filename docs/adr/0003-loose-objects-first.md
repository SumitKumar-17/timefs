# ADR-0003: Loose-object store first, packs only if measured

**Status:** Accepted (loose store implemented; packs not built)

## Context
* One file per object is simple and crash-safe (write temp, `rename`).
* Millions of blocks mean millions of small files and inodes on the host.

## Decision
* Define an `ObjectStore` trait and ship a loose backend (`objects/ab/cdef...`).
* Add a packed backend only if benchmarks show loose overhead dominates (inode count, `fsync` cost, directory size, read latency).
* Benchmarks: [benchmarks.md](../benchmarks.md) (M6, in progress).

## Alternatives
* **Pack/segment log from day one:** better small-object performance, but needs index recovery, compaction and crash handling first.
* **Embedded KV store (RocksDB/LMDB/sled):** fast, but a large dependency with crash semantics to trust, and it hides what we want to study.

## Consequences
* Simple, debuggable on-disk state (`ls objects/`).
* Weak spot: many small objects, tracked as a benchmark scenario.
* The trait boundary keeps a pack backend a localised change.
