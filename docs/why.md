# Why TimeFS exists

## The itch

* Most filesystems only answer "what are the bytes in this file right now?".
* History is bolted on from outside:
  * `git`: opt-in, weak on large/binary files, needs discipline.
  * Backup tools: periodic and coarse.
  * Editor undo: per-file, per-session.
  * ZFS/Btrfs/LVM snapshots: powerful, but block-device level, root-only, admin-operated.
* Meanwhile Git, Nix, OCI image stores, restic/borg and Venti converged on one primitive: **immutable, content-addressed data plus tiny mutable pointers**.
* Once data is named by its hash:
  * identical data is stored once,
  * a snapshot is just a hash (no copying),
  * verifying and sharing data is trivial,
  * history is a graph you can walk, diff and merge.

## Questions

* Idea: what if the filesystem itself were built on that primitive?
* Research question: how cheaply can a general-purpose, POSIX-ish filesystem support snapshots, branching and recovery (latency, space, write amplification, complexity)?

## What it does better

| Need | Today | With TimeFS |
|------|-------|-------------|
| Undo a delete/overwrite | Hope for a backup | Every snapshot is browsable and restorable |
| Try something risky | `cp -r` (slow, doubles space) or Git (tracked text only) | `timefs branch try-it`, O(1), any file type |
| Compare two states | `diff -r` over two copies | Equal hashes skip whole subtrees; cost follows what changed |
| Merge divergent copies | Manual | 3-way tree merge with explicit conflicts |
| Snapshot cost | Full copy or admin volume snapshot | A 32-byte hash plus a commit object |
| Duplicate data | Stored N times | Stored once (block level) |
| Corruption detection | Often silent | Every object is checked against its name (`fsck`) |
| Crash recovery | fsck / luck | Objects before refs; recovery from `refs/wip/<branch>` |

## Non-goals

* **Not a Git replacement:** versions everything on a mounted filesystem, no `add`/`commit` ceremony.
* **Not distributed:** single node, single writer per mount; immutable objects leave room for replication later.
* **Not performance-first:** userspace over FUSE; the goal is to measure and explain the cost of versioning.
* **Not a security boundary:** no encryption, only POSIX mode bits.

## Engineering approach

* `timefs-core` has no FUSE dependency: unit, property and model-based tested in isolation.
* Documented on-disk format with a version marker ([format.md](format.md)).
* Crash consistency by construction ([architecture.md](architecture.md)).
* Decisions recorded as ADRs with rejected alternatives ([adr/](adr/)).
* Benchmarks are reproducible; numbers in docs come only from real runs ([benchmarks.md](benchmarks.md)).
* CI runs fmt, clippy, tests and FUSE integration tests.

## Prior art

| System | Lesson taken |
|--------|--------------|
| ZFS / Btrfs | CoW trees, cheap snapshots (but block-level, kernel-resident) |
| Git | Content-addressed DAG, 3-way merge (but no live filesystem) |
| Plan 9 Venti / Fossil | Write-once deduplicated blocks with snapshot FS on top |
| restic / borg / casync | Chunked, deduplicated repositories, content-defined chunking |
| Nix, OCI layers, lazy image filesystems | Immutable trees, fetch only what is used |
| NILFS2 / LFS | Log-structured continuous snapshots |
| Dolt / lakeFS | Branch/merge over large datasets |

* Niche: a mountable, writable, general-purpose userspace filesystem with Git-like branching as a first-class operation and measured costs as a deliverable.
