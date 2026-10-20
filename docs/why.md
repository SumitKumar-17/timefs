# Why TimeFS exists

## The itch

Most filesystems answer one question: *what are the bytes in this file right now?*
Everything about the past, "what did this directory look like an hour ago?",
"undo that `rm -rf`", "try a risky refactor without losing my place", is bolted on
from outside: `git` (opt-in, ignores large/binary files, needs discipline), backup
tools (periodic, coarse), editors' undo (per-file, per-session), or heavyweight
volume managers (ZFS/Btrfs/LVM snapshots; powerful but block-device-level, root-only,
and operated by a storage admin rather than by the person doing the work).

At the same time, a different corner of systems engineering has quietly converged on a
better primitive: **immutable, content-addressed data plus tiny mutable pointers**.
Container image stores, Git, Nix, IPFS, restic/borg, Venti and the newer lazy-loading
image filesystems all rely on it. Once data is addressed by its hash:

* identical data is stored once (dedupe is free, not a feature),
* a snapshot is just a hash (O(1), no copying),
* sharing, caching and verifying data become trivial (the name *is* the checksum),
* history is a graph you can walk, diff and merge.

TimeFS started from a simple question:

> **What if the *filesystem itself* were built on that primitive, so that history,
> branching and recovery are properties of the filesystem rather than tools layered on
> top of it?**

and a measurable research question:

> **How cheaply can a general-purpose, POSIX-ish filesystem support snapshots,
> branching and recovery?**, in latency, space, write amplification, and complexity.

## What it can do better than the status quo

| Need | Today | With TimeFS |
|------|-------|-------------|
| "Undo that delete/overwrite" | Hope for a backup or editor history | Every checkpoint is browsable and restorable, with no copy cost |
| "Try something risky on this directory" | `cp -r` (slow, doubles space) or Git (only tracked text) | `timefs branch try-it`, O(1), works for any file type and size |
| "Compare two states of a tree" | `diff -r` over two full copies | Hash comparison skips every identical subtree; cost ∝ what changed |
| "Merge two divergent copies" | Manual | 3-way tree merge with explicit conflicts |
| Snapshot cost | Full copy, or admin-level volume snapshots | A 32-byte hash plus the commit object |
| Duplicate data (build artefacts, VM images, `node_modules`) | Stored N times | Stored once (block level) |
| Corruption detection | Often silent | Every object is verified against its own name |
| Crash recovery | fsck / luck | Atomic root-pointer swap; unreferenced objects are harmless garbage |

## What it deliberately is *not*

* **Not a Git replacement.** Git has a rich ecosystem and a human-oriented workflow.
  TimeFS versions *everything on a mounted filesystem*, including large binaries, with no
  `add`/`commit` ceremony, closer to "Time Machine meets `git branch`".
* **Not a distributed filesystem** (yet). Single node, single writer per mount. The
  object model is designed so that replication is a natural future extension
  (objects are immutable; syncing is "send me the hashes you don't have"), but it is
  out of scope for v1.
* **Not a performance-first filesystem.** It runs in userspace over FUSE. The goal is to
  *measure and explain* the cost of versioning, not to beat ext4.
* **Not a security boundary.** No encryption or access-control model beyond POSIX mode bits
  in v1.

## Why this is an engineering project, not a toy

* A **versioning engine with no FUSE dependency** (`timefs-core`) that can be unit-tested,
  fuzzed, property-tested and benchmarked in isolation.
* A **stable, documented on-disk format** with a version marker, canonical encoding and
  integrity verification (see [`format.md`](format.md)).
* **Crash-consistency by construction**: objects are written before the pointer that
  references them; pointers move atomically via `rename` (see [`architecture.md`](architecture.md)).
* **Design decisions recorded as ADRs** with the alternatives we rejected
  (see [`adr/`](adr/)).
* **Benchmarks that answer the research question** and are reproducible from the repo
  (see [`benchmarks.md`](benchmarks.md)); numbers in docs are only ever copied from real runs.
* **CI** (format, lint, tests, FUSE integration tests) on every push.

## Prior art we learn from

* **ZFS / Btrfs**: copy-on-write trees, cheap snapshots; block-device level and kernel-resident.
* **Git**: content-addressed object DAG, 3-way merge; file-level only, no live filesystem.
* **Plan 9 Venti / Fossil**: permanent, deduplicated, write-once block storage with a snapshot-based FS on top.
* **restic / borg / casync**: chunked, deduplicated, content-addressed backup repositories (casync-style content-defined chunking).
* **Nix store, OCI image layers, lazy-loading image filesystems**: immutable content-addressed
  trees; fetching only the files actually used.
* **NILFS2 / LFS**: log-structured, continuous snapshotting.
* **Dolt / lakeFS**: "git for data": branch/merge semantics over large datasets.

TimeFS's niche is the *combination*: a mountable, general-purpose, writable POSIX-style
filesystem in userspace, with Git-like branching *as a first-class operation* and
cost measurements as a first-class deliverable.
