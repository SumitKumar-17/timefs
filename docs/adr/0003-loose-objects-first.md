# ADR-0003: Loose-object store first, packs only if measured

**Status:** Accepted

## Context
Storing one file per object is simple and obviously crash-safe (write temp, `rename`), but
a tree with millions of blocks means millions of small files and inodes on the host FS.

## Decision
Define an `ObjectStore` trait and ship a **loose-object backend** (`objects/ab/cdef…`) first.
Add a packed/append-only backend **only if** benchmarks (`docs/benchmarks.md`) show that
loose-object overhead (inode count, `fsync` cost, directory size, read latency) dominates.

## Alternatives
* **Pack/segment log from day one** with an in-memory index: better small-object
  performance, but needs index recovery, compaction and crash handling before we even have
  a working filesystem, premature complexity.
* **Embedded KV store (RocksDB/LMDB/sled).** Fast, but a large dependency whose crash
  semantics we would have to trust and explain; hides exactly what we want to study.

## Consequences
* Simple, debuggable on-disk state (`ls objects/`).
* Known weak spot: many small objects. This is tracked as a benchmark scenario, and the trait boundary keeps a pack backend a localised change.
