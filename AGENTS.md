# AGENTS.md — guide for contributors and coding agents

TimeFS is a versioning userspace filesystem written in Rust. Read `docs/architecture.md`
and the ADRs in `docs/adr/` before changing design-level behaviour.

## Repository map

* `crates/timefs-core` — all logic; **must not depend on FUSE**. Modules: `hash`, `codec`,
  `object`, `store`, `chunk`, `refs`, `repo`, `worktree`, `diff`, `merge`, `gc`, `fsck`.
* `crates/timefs-fuse` — thin `fuser` adapter + control socket. No versioning logic here.
* `crates/timefs-cli` — the `timefs` binary (clap).
* `docs/` — design docs; `docs/roadmap.md` is the progress log.

## Commands

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release
```

FUSE integration tests need `/dev/fuse` and `fusermount3`; they skip when unavailable
unless `TIMEFS_REQUIRE_FUSE=1` is set (CI sets it). Run them with `--test-threads=1`.
If a test dies and leaves a mount: `fusermount3 -u <mountpoint>`.

## Engineering rules

1. **Correctness before speed.** Optimise only with a measurement; record it in
   `docs/benchmarks.md` (what was measured, the bottleneck, the effect — including failures).
2. **Never fabricate results.** Test, benchmark and CI outcomes in docs/commit messages must come from actual runs.
3. **Format stability.** Object encodings are canonical and hashed. Changing a byte layout
   requires a `FORMAT` version bump, a note in `docs/format.md`, and a migration plan.
4. **Write ordering invariant.** Objects are durable before any ref references them; refs
   change only via temp-file → fsync → rename → fsync(dir). Never weaken this.
5. **No `unsafe`** in `timefs-core`. `unsafe` elsewhere needs a `// SAFETY:` justification.
6. **No panics on untrusted bytes.** Decoding corrupt objects returns an error.
7. **Errors:** typed errors (`thiserror`) in libraries; map to `errno` only in `timefs-fuse`.
8. **Docs move with code.** A change to behaviour updates `docs/architecture.md`/`format.md`
   (and adds an ADR for design decisions) in the same commit. Update `docs/roadmap.md` status.
9. **Tests with every change.** Bug fixes get a regression test. WorkTree changes must keep the model-based property test passing.
10. Keep dependencies few and justified; prefer std.

## Git conventions

* Small, meaningful commits; imperative subject ≤ 72 chars (`core: add loose object store`).
  Prefix with the area: `core:`, `fuse:`, `cli:`, `docs:`, `ci:`, `bench:`, `test:`, `build:`.
* Body explains *why*, not just what. Mention measured results only if they were measured.
* **No `Co-Authored-By` trailers** or other attribution trailers in commits.
* Keep `main` green: fmt, clippy (warnings denied) and tests pass before pushing.

## Definition of done for a change

Code + tests + docs updated, `cargo fmt`, `clippy -D warnings` and `cargo test --workspace`
clean, roadmap/progress log updated if a milestone moved.
