# ADR-0005: Extent-based files; no hard links in v1

**Status:** Accepted (implemented)

## Context
* Files need cheap random writes, cheap snapshots and dedupe.
* Chunking strategy and hard-link support affect the object format.

## Decision
* A file is an ordered list of `(blob hash, length)` extents; block size is not fixed by the format.
* Live writes use fixed 64 KiB blocks, so in-place writes touch only overlapped extents.
* Content-defined chunking (FastCDC) is available at `import` time (`--chunker cdc[:AVG]`), because it dedupes better under mid-file insertions. Its benefit is measured, not assumed.
* Truncate-extend creates in-memory `Hole` extents backed by one shared all-zero blob.
* Hard links are not supported: dedupe already gives the space benefit, and links would need inode objects and refcount-aware GC.

## Alternatives
* **Fixed-size blocks only:** simpler, but mid-file insertions defeat dedupe.
* **CDC for live writes:** an in-place edit shifts boundaries and forces re-chunking the tail each checkpoint.
* **Inode objects (identity separate from names):** allow hard links, but add indirection to rename/stat and grow the format.

## Consequences
* Offset to extent lookup needs a prefix-sum / binary search.
* Long extent lists are paged and holes are stored as extents without blobs ([ADR-0009](0009-log-structured-pack-store.md), part 2).
