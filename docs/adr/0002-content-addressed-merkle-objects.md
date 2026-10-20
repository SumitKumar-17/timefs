# ADR-0002: Content-addressed immutable Merkle objects

**Status:** Accepted

## Context
We need O(1) snapshots, cheap branching, dedupe and corruption detection.

## Decision
Four immutable object kinds (Blob, File, Tree, Commit) named by BLAKE3-256 of their
canonical encoding; the only mutable state is a small set of refs. Directories are Merkle
trees, so equal subtrees have equal hashes. See `docs/format.md`.

## Alternatives
* **Block-level CoW with a B-tree (ZFS/Btrfs style).** Powerful but requires owning block
  allocation, free-space maps and reference counting; far more code and a much larger
  bug surface for a userspace project.
* **Log-structured with a mutable inode map (NILFS style).** Snapshots are easy but
  branching and merge are not natural, and cleaning (segment GC) is complex.
* **SHA-256.** Ubiquitous but ~several× slower than BLAKE3 in software; hashing is on the
  hot path of every checkpoint. BLAKE3 is also parallelisable and has a stable, well-reviewed spec.
* **Mutable inodes + per-file version chains.** Cheap per-file history but no consistent
  whole-tree snapshot and no merge base.

## Consequences
* Dedupe and integrity checking are inherent, not features.
* Metadata (mode, mtime, size) lives in the *parent* tree entry. Consequence: a metadata
  change alters the parent hash; merge treats equal content + equal mode/uid/gid as equal and ignores mtime.
* Deleting data requires GC (ADR-0008), because objects are shared.
* Hash collisions are treated as impossible (256-bit); no collision handling is implemented.
