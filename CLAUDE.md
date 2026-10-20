@AGENTS.md

# Claude Code notes

* Follow `AGENTS.md`; it is the single source of project conventions.
* Style: no em dashes or en dashes in anything you write (code, comments, docs, commits). Keep code modular and clean.
* Commits: **do not add `Co-Authored-By` or "Generated with" trailers** (repo owner's explicit rule).
* Work autonomously: implement, test, benchmark and document; do not stop at scaffolding.
* After editing code run `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`
  and `cargo test --workspace` before committing.
* Only claim a test/benchmark/CI result you actually observed in this session.
* The mount tests touch real mounts: always unmount (`fusermount3 -u`) before finishing a task.
* Working dir for temp experiments: use a scratch dir outside the repo (e.g. `mktemp -d`), never commit repos/mounts.
