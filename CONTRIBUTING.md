# Contributing to TimeFS

* [`AGENTS.md`](AGENTS.md) is the authoritative guide (architecture, invariants, commands,
  conventions). This file is the short version for human contributors.

Read [AGENTS.md](AGENTS.md) first; it is the full rulebook. This page is the short version.

## Prerequisites

* Linux with FUSE (`/dev/fuse`) and `fusermount3`.
* `libfuse3` dev files and `pkg-config` (Debian/Ubuntu: `libfuse3-dev fuse3 pkg-config`; Arch: `fuse3`).
* Rust stable (pinned in `rust-toolchain.toml`).

## Build and test

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release

# benchmarks (release build; scratch space must be on the disk under test)
cargo run --release -p timefs-bench -- --work bench-out core
BENCH_ARGS="--repeat 3" scripts/bench-mount.sh
```

* FUSE integration tests need `/dev/fuse` and `fusermount3`; they skip when unavailable
  unless `TIMEFS_REQUIRE_FUSE=1` is set (CI sets it).
* Run FUSE tests with `--test-threads=1`.
* If a test dies and leaves a mount: `fusermount3 -u <mountpoint>`.

## Workflow

* Small, meaningful commits; one logical change each.
* Subject: imperative, at most 72 chars, prefixed with the area:
  `core:`, `fuse:`, `cli:`, `bench:`, `docs:`, `ci:`, `test:`, `build:`.
* Body: short bullet points explaining why, not just what.
* No `Co-Authored-By` or any other attribution trailers.
* Keep `main` green: fmt, clippy and tests pass before pushing.

## Definition of done

* Code, tests and docs updated in the same commit.
* Bug fixes include a regression test; WorkTree changes keep the model-based test passing.
* `cargo fmt`, `clippy -D warnings` and `cargo test --workspace` (FUSE tests included) are clean.
* `docs/roadmap.md` updated if a milestone moved.

## Design changes

* Read `docs/architecture.md` and the ADRs before changing design-level behaviour.
* Design decisions need a new ADR in [docs/adr/](docs/adr/) (context, decision, rejected alternatives, consequences).
* ADRs are append-only; supersede with a new ADR instead of rewriting.
* Byte layout changes need a `FORMAT` version bump, a note in `docs/format.md` and a migration plan.

## Benchmarks and results

* Optimise only with a measurement.
* Numbers come only from real runs, recorded in [docs/benchmarks.md](docs/benchmarks.md)
  (what was measured, the bottleneck, the effect, failures included).
* Never fabricate test, benchmark or CI results in docs or commit messages.

## Code style

* Clean and modular: one responsibility per module, small functions, descriptive names.
* Comments explain why.
* `timefs-core` must not depend on FUSE and has no `unsafe`; elsewhere `unsafe` needs a `// SAFETY:` note.
* Typed errors (`thiserror`) in libraries; map to `errno` only in `timefs-fuse`.
* No em dashes or en dashes anywhere (code, comments, docs, commit messages); use commas,
  colons, parentheses or plain hyphens.
