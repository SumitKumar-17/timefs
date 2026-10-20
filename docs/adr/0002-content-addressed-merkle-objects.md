# ADR-0002: Content-addressed immutable Merkle objects

**Status:** Accepted (implemented)

## Context
* Need O(1) snapshots, cheap branching, dedupe and corruption detection.

## Decision
* Four immutable object kinds: Blob, File, Tree, Commit.
* Each is named by the BLAKE3-256 hash of its canonical encoding.
* Directories are Merkle trees, so equal subtrees have equal hashes.
* The only mutable state is a small set of refs.
* Details: [format.md](../format.md).

## Alternatives
* **Block-level CoW with a B-tree (ZFS/Btrfs style):** needs block allocation, free-space maps and refcounts; far more code and bug surface.
* **Log-structured with a mutable inode map (NILFS style):** easy snapshots, but branching and merge are unnatural and segment cleaning is complex.
* **SHA-256:** several times slower than BLAKE3 in software, and hashing is on the checkpoint hot path.
* **Mutable inodes with per-file version chains:** cheap per-file history, but no whole-tree snapshot and no merge base.

## Consequences
* Dedupe and integrity checking are inherent.
* Metadata (mode, mtime, size) lives in the parent tree entry, so a metadata change alters the parent hash.
* Merge treats same kind, content, mode, uid and gid as equal and ignores mtime.
* Deleting data needs GC ([ADR-0008](0008-stop-the-world-gc.md)) because objects are shared.
* Hash collisions are treated as impossible; no handling exists.
