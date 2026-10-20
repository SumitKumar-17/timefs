# Architecture Decision Records

Each ADR records one significant decision: context, the decision, the alternatives we
rejected, and the consequences. ADRs are append-only; a superseded decision gets a new ADR
that links back, instead of being rewritten.

| # | Title | Status |
|---|-------|--------|
| [0001](0001-language-rust-fuser.md) | Implement in Rust on libfuse3 (via `fuser`) | Accepted |
| [0002](0002-content-addressed-merkle-objects.md) | Content-addressed immutable Merkle objects | Accepted |
| [0003](0003-loose-objects-first.md) | Loose-object store first, packs only if measured | Accepted |
| [0004](0004-worktree-overlay-and-wip-checkpoints.md) | Mutable WorkTree overlay with `wip` checkpoint refs | Accepted |
| [0005](0005-extent-based-files.md) | Extent-based files; no hard links in v1 | Accepted |
| [0006](0006-merge-semantics.md) | Tree-level 3-way merge, explicit conflicts | Accepted |
| [0007](0007-control-plane.md) | Unix-socket control plane + offline repo commands | Accepted |
| [0008](0008-stop-the-world-gc.md) | Stop-the-world mark-and-sweep GC with grace period | Accepted |
| [0009](0009-log-structured-pack-store.md) | Log-structured pack store, refs in the log, extent pages | Proposed |
