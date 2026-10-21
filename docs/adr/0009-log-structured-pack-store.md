# ADR-0009: Log-structured pack store

**Status:** Accepted and implemented. Part 1: pack log, refs in the log, compaction (the
default store). Part 2: paged extent lists and hole extents.

## Context
Measured on the loose store (see [benchmarks.md](../benchmarks.md)):
* **fsync latency:** an application `fsync` on a TimeFS mount costs about 9 to 10 ms against
  about 2 ms on ext4. The phase breakdown shows why:
  * object sync: two parallel fsync rounds (files, then directories), about 4 to 5 ms
  * ref update: temp file fsync, rename, directory fsync, about 4 to 5 ms
  * persisting and committing in memory: under 1 ms
* **Space:** small objects round up to the host block size: `du` shows about 2.7x the
  stored bytes for a tree of 4 KiB files.
* **Inodes:** about 2.3 host inodes per user file (blob, file object, plus trees).
* **Flat extent lists** make the file object of a 32 MiB file at 4 KiB blocks about 280 KiB,
  rewritten on every checkpoint that touches the file (write amplification table, Q7).

## Decision
* Add a second `ObjectStore` backend: append-only segment files plus an index.
  * Record: `len | kind | hash | payload | crc32`; objects are appended, never rewritten.
  * Index: hash to `(segment, offset, len)`, rebuilt by scanning segments when its own
    checkpoint is missing or stale.
* Put ref updates in the same log as records (`ref name -> hash`), so one `fdatasync` of
  the active segment makes objects and the ref durable together: one journal round instead
  of four.
* Recovery: replay the log; a torn tail record fails its CRC and is truncated.
* GC becomes compaction: copy live records into a new segment, then drop old segments.
* Part 2: split long extent lists into pages of 1024 extents behind a `FileIndex` object,
  so a small edit to a big file rewrites one page and the index; store holes as extents
  with a reserved hash instead of one zero-block reference per block.

## Implementation notes (part 2)
* `timefs-core/src/filedata.rs` stores and loads file objects; `put_file`/`get_file` use it.
* Files of up to 1024 extents stay one flat `File`, so existing repositories are unchanged.
* Measured: one 4 KiB write to a 32 MiB file at 4 KiB blocks now writes 42.4 KiB (was
  280.1 KiB); the file size limit went from 64 GiB to 1 PiB.

## Implementation notes (part 1)
* `timefs-core/src/store/pack/`: `record` (framing, scanning), `segment` (files), `mod` (store).
* Index rebuild reads only record headers of sealed segments; the active segment is
  fully verified because only it can be torn.
* `synced` markers (one per batch after an `fdatasync`) separate torn tails from corruption
  of data that was already durable; the latter fails the open loudly.
* One writer process (`pack/WRITER` lock); readers in other processes see a consistent prefix.
* Record checks use the first 8 bytes of BLAKE3 instead of a CRC: no new dependency.

## Alternatives
* **Keep loose objects, call `syncfs(2)` once per checkpoint:** one round, but it flushes
  every dirty page on the host filesystem, so latency depends on unrelated applications.
* **Skip the temp-file fsync before rename:** relies on ext4 `data=ordered` behaviour; not
  correct on other filesystems, so rejected.
* **Embedded KV store:** still rejected for the reasons in ADR-0003.

## Consequences
* fsync latency close to one host fsync: measured in [benchmarks.md](../benchmarks.md).
* New code paths that need crash-injection tests: torn records, index rebuild, compaction
  interrupted halfway.
* Format 2 is a new layout chosen at `init`; existing loose repositories keep working and
  `timefs migrate <dest> --store pack` copies one into the new layout (hash-preserving, verified by fsck).
* The loose backend stays as the simple, debuggable reference implementation.
