# ADR-0004: Mutable WorkTree overlay with `wip` checkpoint refs

**Status:** Accepted (implemented)

## Context
* Applications issue many small `write()` calls.
* Writing immutable objects and the path to the root on each one is too slow.
* Durability and crash recovery are still wanted, without a write-ahead log.

## Decision
* The mount is a mutable in-memory overlay (`WorkTree`) over an immutable base commit.
* Mutations touch only the overlay.
* **Checkpoint:** persist the overlay as a commit and point `refs/wip/<branch>` at it.
* **Snapshot:** checkpoint, then advance `refs/heads/<branch>`.
* Triggers: `fsync`, timer (`--checkpoint-interval`), dirty-bytes threshold (`--flush-threshold-mib`), unmount.
* On mount, a valid wip ref (parent equals the branch tip) is recovered.

## Alternatives
* **Write-ahead log of FUSE operations:** finer durability, but a second persistence format with replay and compaction.
* **Commit on every write:** maximal granularity, unusable performance.
* **Write through to host files, snapshot lazily (overlayfs style):** splits state across two systems.

## Consequences
* Durability matches a normal filesystem: un-`fsync`ed data may be lost on a crash, bounded by the checkpoint interval.
* Checkpoint commits parent on the branch tip, so they do not accumulate history.
* Unpersisted data is bounded by the dirty-bytes threshold.
* Recovery reuses the object and ref machinery; no extra format.
* Inodes of touched nodes stay in memory for the life of the mount.
