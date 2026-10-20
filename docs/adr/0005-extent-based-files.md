# ADR-0005: Extent-based files; no hard links in v1

**Status:** Accepted

## Context
Files need efficient random writes, cheap snapshots, and dedupe. Chunking strategy and
the hard-link question affect the object format.

## Decision
A file is an ordered list of `(blob hash, length)` extents. Blob size is **not** fixed by
the format. The default writer uses fixed 64 KiB blocks (random in-place writes only touch
overlapping extents). Content-defined chunking (FastCDC) is available at **import** time
because it dedupes better when bytes are inserted/removed mid-file; its benefit is
evaluated in the benchmarks rather than assumed. Truncate-extend creates in-memory `Hole`
extents materialised as references to one shared all-zero blob.

Hard links are **not supported** in v1 (`link()` fails). Content-level dedupe already gives the space benefit; hard links would require identity (inode) objects in the format and refcount-aware GC.

## Alternatives
* **Fixed-size blocks only**: simpler, but mid-file insertions defeat dedupe.
* **CDC for live writes**: an in-place edit shifts boundaries, forcing re-chunking of the tail on every checkpoint.
* **Inode objects (identity separated from names)**: supports hard links, but every rename/stat path gains an indirection and the format grows.

## Consequences
* Variable-size extents mean offset→extent lookup needs a prefix-sum/binary search.
* Flat extent lists will become long for huge files; a two-level extent tree is a documented future change (needs a format bump).
