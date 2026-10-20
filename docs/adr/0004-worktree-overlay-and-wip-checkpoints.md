# ADR-0004: Mutable WorkTree overlay with `wip` checkpoint refs

**Status:** Accepted

## Context
Applications issue many small `write()`s. Creating immutable objects and rewriting the
path to the root on each one would be prohibitively slow, yet we still want durability and
crash recovery without implementing a write-ahead log.

## Decision
The mounted filesystem is a **mutable in-memory overlay** (`WorkTree`) on top of an
immutable base commit. Mutations only touch the overlay. A **checkpoint** persists the
overlay as a commit and points `refs/wip/<branch>` at it; a **snapshot** additionally
advances `refs/heads/<branch>`. Checkpoints are triggered by `fsync`, a timer, a dirty-bytes
threshold and unmount. On mount, a valid `wip` ref (whose parent is the branch tip) is
recovered.

## Alternatives
* **Write-ahead log of FUSE operations.** Finer-grained durability, but a second
  persistence format with its own replay/compaction/corruption handling.
* **Commit on every write.** Maximal history granularity, unusable write performance.
* **Write straight through to host files and snapshot lazily (overlayfs-style).** Splits
  state across two systems and complicates consistency.

## Consequences
* Durability contract equals a normal filesystem: un-`fsync`ed data may be lost on crash,
  bounded by the checkpoint interval.
* Checkpoint commits parent on the branch tip, so they never accumulate history.
* Memory use is bounded by the dirty-bytes threshold.
* Reuses the existing object/ref machinery for recovery — no extra format.
