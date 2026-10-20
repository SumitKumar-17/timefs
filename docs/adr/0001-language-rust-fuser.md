# ADR-0001: Implement in Rust on libfuse3 (via `fuser`)

**Status:** Accepted

## Context
The filesystem must speak the FUSE protocol and run as a long-lived daemon that owns
persistent data. Candidates: C with libfuse3 directly, Rust with `fuser`, Go with `go-fuse`.

## Decision
Rust, using the `fuser` crate built with its `libfuse3` feature, so mounting goes through
the system **libfuse3** (`fusermount3`), i.e. the reference implementation, while the
filesystem logic is memory-safe Rust.

## Alternatives
* **C + libfuse3 directly.** Most literal, best documented. But a data-integrity-critical,
  multi-threaded daemon is exactly where manual memory management hurts, and C has no
  built-in test/bench/CI story comparable to `cargo`.
* **Go + go-fuse.** Fastest to iterate; but a GC runtime adds latency noise that muddies
  the very measurements this project exists to make, and it does not use libfuse.

## Consequences
* `fuser`'s `Filesystem` trait takes `&self`; the filesystem needs interior mutability.
* Build requires `libfuse3` headers and `pkg-config` (documented in the README and CI).
* The core library has no FUSE dependency, so the C-vs-Rust choice is isolated to one crate.
* `fuser` 0.x may break its API between releases; we pin the minor version.
