## Summary

* What changed and why.

## Checklist

* [ ] `cargo fmt --all`
* [ ] `cargo clippy --workspace --all-targets -- -D warnings`
* [ ] `cargo test --workspace` passes, including FUSE tests (`TIMEFS_REQUIRE_FUSE=1`, `--test-threads=1`)
* [ ] New or updated tests (bug fixes include a regression test)
* [ ] Docs updated (`docs/architecture.md`, `docs/format.md` as needed)
* [ ] `docs/roadmap.md` updated if a milestone moved
* [ ] ADR added in `docs/adr/` if a design decision changed
* [ ] Benchmarks re-run and recorded in `docs/benchmarks.md` if performance claims are made
* [ ] No em dashes or en dashes in code, comments, docs or commit messages
* [ ] Commits use area prefixes and carry no attribution trailers
