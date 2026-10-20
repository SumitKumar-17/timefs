# ADR-0001: Implement in Rust on libfuse3 (via `fuser`)

**Status:** Accepted (implemented)

## Context
* The filesystem speaks FUSE and runs as a long-lived daemon owning persistent data.
* Candidates: C with libfuse3, Rust with `fuser`, Go with `go-fuse`.

## Decision
* Rust, using `fuser` with its `libfuse3` feature.
* Mounting goes through system libfuse3 (`fusermount3`); the logic is memory-safe Rust.

## Alternatives
* **C + libfuse3 directly:** best documented, but manual memory management is risky in a multi-threaded, integrity-critical daemon, and there is no cargo-like test/bench/CI story.
* **Go + go-fuse:** fast to iterate, but GC latency noise muddies the measurements, and it does not use libfuse.

## Consequences
* `fuser`'s `Filesystem` trait takes `&self`, so the filesystem needs interior mutability.
* Build needs `libfuse3` headers and `pkg-config` (documented in README and CI).
* `timefs-core` has no FUSE dependency, so the choice is isolated to `timefs-fuse`.
* `fuser` 0.x may break its API; the minor version is pinned.
