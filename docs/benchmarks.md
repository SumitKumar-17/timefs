# Benchmarks

Goal: answer *"how cheaply can a general-purpose filesystem support snapshots, branching
and recovery?"* with measurements. **Numbers in this file are only ever pasted from real
runs, with the commit hash and machine description.** Until a run exists, the result
tables are empty on purpose.

## Questions and metrics

| # | Question | Metric | Compare against |
|---|----------|--------|-----------------|
| Q1 | How fast is a snapshot as the tree grows? | latency vs files (10³…10⁶) | `cp -r`, `cp --reflink` (if available) |
| Q2 | How much space does a snapshot cost? | bytes of new metadata per snapshot, per changed file | full copy |
| Q3 | How fast are branch / restore? | latency | `cp -r`, `git branch` / `git checkout` |
| Q4 | What does dedupe buy on realistic data? | logical bytes ÷ stored bytes | tar, raw size |
| Q5 | Fixed vs content-defined chunks | dedupe ratio on edited files; throughput | each other |
| Q6 | What is the write overhead? | MB/s seq & random read/write, IOPS | host FS direct; **passthrough FUSE baseline** (isolates FUSE cost from TimeFS cost) |
| Q7 | Write amplification of a checkpoint | bytes written ÷ bytes changed | — |
| Q8 | Merge cost | latency vs changed paths | `git merge` on equivalent data |
| Q9 | Loose-object overhead | inodes used, `fsync` cost, read latency | (decides ADR-0003 follow-up) |

## Methodology

* Always report: commit, `rustc` version, kernel, CPU, RAM, storage device and filesystem
  that hosts the repo, FUSE mount options, whether the page cache was dropped.
* Release builds only (`cargo build --release`).
* ≥ 5 repetitions; report median and spread; discard nothing silently.
* Separate **core-level** benchmarks (no FUSE; `cargo bench`, `timefs bench`) from
  **mount-level** benchmarks (`fio`, `scripts/bench-mount.sh`).
* Profile before optimising (`perf`, flamegraph); record the bottleneck and the measured
  effect of every optimisation, including the ones that did not help.

## Results

_No runs recorded yet._
