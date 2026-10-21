# Benchmarks

Goal: answer *"how cheaply can a general-purpose filesystem support snapshots, branching
and recovery?"* with measurements. **Numbers in this file are only ever pasted from real
runs, with the commit hash and machine description.**

## Questions and metrics

| # | Question | Metric | Compare against |
|---|----------|--------|-----------------|
| Q1 | How fast is a snapshot as the tree grows? | latency vs files (10³…10⁶) | `cp -r`, `cp --reflink` (if available) |
| Q2 | How much space does a snapshot cost? | bytes of new metadata per snapshot, per changed file | full copy |
| Q3 | How fast are branch / restore? | latency | `cp -r`, `git branch` / `git checkout` |
| Q4 | What does dedupe buy on realistic data? | logical bytes ÷ stored bytes | tar, raw size |
| Q5 | Fixed vs content-defined chunks | dedupe ratio on edited files; throughput | each other |
| Q6 | What is the write overhead? | MB/s seq & random read/write, IOPS | host FS direct; **passthrough FUSE baseline** (isolates FUSE cost from TimeFS cost) |
| Q7 | Write amplification of a checkpoint | bytes written ÷ bytes changed | n/a |
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

### Environment

| Item | Value |
|---|---|
| Commit | M6 tables: `4fcdcfb` (mount "before": `903f7ab`); M7 pack vs loose tables: `2f421f6` |
| CPU | Intel Core i5-6300U, 2 cores / 4 threads, 2.4 GHz |
| Storage | Crucial MX300 525 GB SATA SSD, ext4 (`relatime`), repo and scratch on the same disk |
| Kernel / libfuse | Linux 7.2.6 (Arch), libfuse 3.18.3 |
| Toolchain | rustc 1.97.1, `--release` (thin LTO) |
| Page cache | not dropped globally (no root); "cold" reads use `posix_fadvise(DONTNEED)` per file |
| Commands | `timefs-bench --work bench-out --repeat 5 --sizes 1000,10000,100000 core` and `BENCH_ARGS="--repeat 3" scripts/bench-mount.sh` |

Cells show `median (min..max)`. Generated files average 4 KiB (0.5x to 1.5x), random content.

### Answer to the research question (summary)

Numbers from the M7 run (`2f421f6`, pack store, the default; loose store in parentheses)
unless marked M6 (`4fcdcfb`, loose store).

* **Snapshots cost what changed, not what exists.** After editing one file, a snapshot of a
  100,000-file tree took 2.7 ms (loose 9.5 ms) and stored 6 objects (18 KiB).
  `git commit -a` needed 150 to 244 ms for the same edit (it stats every file); `cp -r`
  needed about 4 s and a full copy.
* **Durability costs what the host charges for one fsync.** Application `fsync` on a pack
  mount: 2.0 ms, the same as ext4 itself (2.0 ms) and the FUSE passthrough (2.1 ms).
* **Branching is a ref write at every tree size** (M6: about 4 ms for a loose ref file; with
  pack it is one log record plus one `fdatasync`). Opening any commit and reading a file
  takes 0.1 to 0.2 ms (M6, lazy loading).
* **Merge cost follows changed paths** (M6): 1,000 changed files per side on a 100,000-file
  tree merged in 1.1 ms.
* **Recovery is free at runtime:** one log scan at open. The SIGKILL crash-injection test
  recovered a valid state in every one of its runs on both stores.
* **Storage overhead is near zero with the pack store:** 100,000 files of 4 KiB: 410 MiB on
  disk for 401 MiB stored, 3 host inodes (loose: 1006 MiB, 201,269 inodes).
* **Still not cheap:** the flat extent list of large files (up to 70x write amplification for
  a single 4 KiB write at 4 KiB blocks), and memory for touched inodes. See open bottlenecks.

### Pack store vs loose store (M7, `2f421f6`)

Same machine, same harness: `timefs-bench --repeat 5 --sizes 1000,10000,100000 --store <kind> core snapshot store fsync`.

| 100,000 files | loose | pack | reference |
|---|---|---|---|
| initial import | 45,307 ms | 4,134 ms | `cp -r`: 3,732 to 4,076 ms |
| snapshot after 1 edited file | 9.48 ms | 2.67 ms | `git commit -a`: 150 to 244 ms |
| snapshot after 100 edited files | 122.67 ms | 11.10 ms | |
| disk usage (`du`) | 1006.5 MiB | 409.6 MiB | 400.7 MiB stored |
| host inodes added | 201,269 | 3 | |
| fsck (read and verify everything) | 6,811 ms | 2,552 ms | |
| checkpoint for append + fsync (core) | 16.07 ms | 2.25 ms | |

Mount workloads, `STORE=<kind> BENCH_ARGS="--repeat 3" scripts/bench-mount.sh` (both runs back to back):

| workload | ext4 | fuse-passthrough | timefs-pack | timefs-loose |
|---|---|---|---|---|
| seq write MB/s (incl fsync) | 408.8 | 333.1 | 531.7 | 538.8 |
| seq read warm MB/s | 3531.2 | 2040.7 | 5732.9 | 6559.3 |
| seq read cold MB/s | 386.1 | 2306.0 | 2233.4 | 2080.8 |
| rand read 4K IOPS (cold) | 5093.2 | 60803.1 | 45480.4 | 44919.6 |
| rand write 4K IOPS (incl fsync) | 37734.9 | 22553.1 | 20776.6 | 11159.6 |
| create 4 KiB files /s | 30713.6 | 8773.3 | 14671.1 | 15265.2 |
| stat /s | 445678.0 | 59325.2 | 76493.8 | 58685.3 |
| open+read+close /s | 80879.0 | 16062.9 | 26820.1 | 26193.4 |
| readdir entries /s | 2544307.0 | 821532.7 | 2256736.9 | 1676025.4 |
| rename /s | 44617.4 | 524.8 | 32587.9 | 30691.9 |
| unlink /s | 83208.9 | 21297.4 | 65241.5 | 62476.9 |
| fsync latency ms (append 4 KiB) | 2.0 | 2.1 | 2.0 | 35.3 |

* The loose store's fsync latency varies with the disk's state: 9.4 ms in the M6 run,
  35.3 ms in this one (taken right after the 100,000-file core runs). The pack store does one
  `fdatasync` of one file, so it tracks the host's own fsync cost.
* Read paths are the same code (read cache, kernel page cache), so the stores read alike.

### Core scenarios (no FUSE), M6 run on the loose store

#### Snapshot latency and space (Q1, Q2)

| files | operation | ms median (min..max) | new objects | new bytes |
|---|---|---|---|---|
| 1000 | cp -r (full copy) | 54.0 | n/a | full size |
| 1000 | timefs initial import | 769.7 | 2013 | 4.0 MiB |
| 1000 | timefs snapshot after editing 0 file(s) | 0.00 (0.00..0.00) | 0 | 0 B |
| 1000 | timefs snapshot after editing 1 file(s) | 12.25 (11.69..15.37) | 6 | 12.7 KiB |
| 1000 | timefs snapshot after editing 10 file(s) | 21.92 (21.42..24.03) | 27 | 77.2 KiB |
| 1000 | timefs snapshot after editing 100 file(s) | 72.59 (64.49..85.08) | 193 | 430.0 KiB |
| 1000 | git commit -a after editing 1 file | 11.1 | n/a | n/a |
| 10000 | cp -r (full copy) | 395.1 | n/a | full size |
| 10000 | timefs initial import | 6996.4 | 20103 | 40.1 MiB |
| 10000 | timefs snapshot after editing 0 file(s) | 0.00 (0.00..0.00) | 0 | 0 B |
| 10000 | timefs snapshot after editing 1 file(s) | 12.42 (12.14..13.73) | 6 | 18.4 KiB |
| 10000 | timefs snapshot after editing 10 file(s) | 27.23 (24.17..27.56) | 30 | 109.9 KiB |
| 10000 | timefs snapshot after editing 100 file(s) | 96.52 (90.54..111.88) | 247 | 801.4 KiB |
| 10000 | git commit -a after editing 1 file | 38.1 | n/a | n/a |
| 100000 | cp -r (full copy) | 4574.6 | n/a | full size |
| 100000 | timefs initial import | 37192.4 | 201012 | 400.7 MiB |
| 100000 | timefs snapshot after editing 0 file(s) | 0.00 (0.00..0.00) | 0 | 0 B |
| 100000 | timefs snapshot after editing 1 file(s) | 9.56 (8.94..9.74) | 6 | 18.3 KiB |
| 100000 | timefs snapshot after editing 10 file(s) | 22.43 (21.41..24.07) | 36 | 139.7 KiB |
| 100000 | timefs snapshot after editing 100 file(s) | 117.08 (111.54..125.10) | 289 | 1.1 MiB |
| 100000 | git commit -a after editing 1 file | 176.4 | n/a | n/a |

#### Branch and restore latency (Q3)

| files | operation | ms median (min..max) |
|---|---|---|
| 1000 | create branch | 3.852 (3.704..5.285) |
| 1000 | open commit and read one file | 0.081 (0.079..0.249) |
| 1000 | materialise whole tree (timefs export) | 46.7 (39.2..92.1) |
| 1000 | cp -r of the tree | 48.4 (44.5..48.4) |
| 10000 | create branch | 4.068 (3.794..4.231) |
| 10000 | open commit and read one file | 0.199 (0.180..0.432) |
| 10000 | materialise whole tree (timefs export) | 425.8 (416.8..791.4) |
| 10000 | cp -r of the tree | 402.1 (377.2..431.5) |
| 100000 | create branch | 3.998 (3.436..4.136) |
| 100000 | open commit and read one file | 0.100 (0.093..0.244) |
| 100000 | materialise whole tree (timefs export) | 9375.6 (8125.4..21870.5) |
| 100000 | cp -r of the tree | 4626.4 (4577.8..12645.0) |

#### Merge latency, disjoint edits on both sides (Q8)

| files | changed per side | merge ms median (min..max) |
|---|---|---|
| 1000 | 1 | 0.08 (0.08..0.32) |
| 1000 | 10 | 0.26 (0.25..0.60) |
| 1000 | 100 | 1.38 (0.64..1.65) |
| 10000 | 1 | 0.60 (0.56..1.41) |
| 10000 | 10 | 0.81 (0.64..1.51) |
| 10000 | 100 | 0.64 (0.57..1.54) |
| 10000 | 1000 | 0.64 (0.59..1.49) |
| 100000 | 1 | 0.18 (0.18..0.43) |
| 100000 | 10 | 0.20 (0.18..0.43) |
| 100000 | 100 | 0.41 (0.37..1.36) |
| 100000 | 1000 | 1.13 (1.08..3.15) |

#### Checkpoint write amplification, 32 MiB file (Q7)

| block size | random 4 KiB writes | bytes changed | bytes written by checkpoint | write amplification | checkpoint ms |
|---|---|---|---|---|---|
| 4.0 KiB | 1 | 4.0 KiB | 280.1 KiB | 70.0x | 8.15 |
| 4.0 KiB | 16 | 64.0 KiB | 400.1 KiB | 6.3x | 25.63 |
| 4.0 KiB | 256 | 1.0 MiB | 2.2 MiB | 2.2x | 195.64 |
| 16.0 KiB | 1 | 4.0 KiB | 86.1 KiB | 21.5x | 4.82 |
| 16.0 KiB | 16 | 64.0 KiB | 406.1 KiB | 6.3x | 17.86 |
| 16.0 KiB | 256 | 1.0 MiB | 4.9 MiB | 4.9x | 137.86 |
| 64.0 KiB | 1 | 4.0 KiB | 81.6 KiB | 20.4x | 6.90 |
| 64.0 KiB | 16 | 64.0 KiB | 1.2 MiB | 19.3x | 23.72 |
| 64.0 KiB | 256 | 1.0 MiB | 13.5 MiB | 13.5x | 128.95 |
| 256.0 KiB | 1 | 4.0 KiB | 260.4 KiB | 65.1x | 7.88 |
| 256.0 KiB | 16 | 64.0 KiB | 3.5 MiB | 56.1x | 27.60 |
| 256.0 KiB | 256 | 1.0 MiB | 29.5 MiB | 29.5x | 155.90 |

#### Loose object store overhead (Q9)

| files | logical size | objects | stored bytes | disk usage (du) | inodes added | import ms | sync ms | fsck ms |
|---|---|---|---|---|---|---|---|---|
| 1000 | 3.9 MiB | 2013 | 4.0 MiB | 11.0 MiB | 2270 | 1060 | 0 | 78 |
| 10000 | 39.0 MiB | 20103 | 40.1 MiB | 101.5 MiB | 20360 | 7526 | 0 | 522 |
| 100000 | 390.6 MiB | 201012 | 400.7 MiB | 1006.5 MiB | 201269 | 50649 | 0 | 6869 |

#### Space added per snapshot of a 32 MiB file, by chunker (Q5)

| chunker | initial stored | added per overwrite round (20 x 1 KiB) | added per insert round (20 x 100 B) |
|---|---|---|---|
| fixed 4K | 32.3 MiB | 372.6 KiB | 30.2 MiB |
| fixed 64K | 32.0 MiB | 1.3 MiB | 30.0 MiB |
| cdc avg 4K | 32.2 MiB | 388.6 KiB | 352.4 KiB |
| cdc avg 16K | 32.1 MiB | 575.9 KiB | 544.6 KiB |
| cdc avg 64K | 32.0 MiB | 1.7 MiB | 1.7 MiB |

#### Deduplication across the last commits of a real git history (Q4)

| chunker | revisions | logical bytes (sum) | stored bytes | ratio |
|---|---|---|---|---|
| fixed 4K | 11 | 3.0 MiB | 722.1 KiB | 4.24x |
| fixed 64K | 11 | 3.0 MiB | 790.6 KiB | 3.87x |
| cdc avg 4K | 11 | 3.0 MiB | 689.8 KiB | 4.44x |
| cdc avg 16K | 11 | 3.0 MiB | 748.8 KiB | 4.09x |
| cdc avg 64K | 11 | 3.0 MiB | 790.6 KiB | 3.87x |

* Q5 reading: fixed blocks dedupe in-place overwrites well but lose almost everything on
  insertions (about 30 MiB re-stored per round); content-defined chunks keep insertions cheap
  (350 KiB to 1.7 MiB). The live write path uses fixed blocks (random writes), so CDC is an
  `import` option, as ADR-0005 decided.
* Q4 used the last 11 commits of this repository (small text tree, 3 MiB logical).

#### Checkpoint phase breakdown for append + fsync

| phase | ms median (min..max) |
|---|---|
| persist objects (no sync) | 0.35 (0.21..1.49) |
| create commit object | 0.08 (0.05..0.50) |
| sync objects (fsync batch) | 4.66 (2.58..9.19) |
| write ref (tmp, fsync, rename, fsync dir) | 4.22 (3.48..7.65) |
| total | 9.42 (6.88..17.71) |
| new objects per checkpoint | 4 |

### Mount workloads (FUSE), M6 runs on the loose store

Same Rust workload code on four targets: ext4 directly, tmpfs, a minimal FUSE passthrough
over ext4 (`timefs-bench passthrough`, isolates FUSE overhead), and TimeFS (repo on ext4).
128 MiB big file, 3,000 small files, single-threaded client, 1 FUSE worker thread.

#### Before optimisation (`903f7ab`)

| workload | ext4 | tmpfs | fuse-passthrough | timefs |
|---|---|---|---|---|
| seq write MB/s (incl fsync) | 420.0 (414.8..426.7) | 2240.2 (2065.3..2876.6) | 334.2 (126.5..339.8) | 375.1 (347.2..500.1) |
| seq read warm MB/s | 4435.8 (3852.2..7763.9) | 6173.1 (5294.0..7365.2) | 2115.7 (1859.2..2130.3) | 820.8 (378.9..825.8) |
| seq read cold MB/s | 385.4 (384.8..385.4) | 6709.7 (5333.5..7666.8) | 1843.8 (1619.5..2496.9) | 706.0 (691.7..849.7) |
| rand read 4K IOPS (cold) | 5706.6 (5650.6..5720.6) | 472991.6 (459689.8..489682.3) | 54504.5 (49016.7..55180.8) | 15325.9 (11588.5..16341.0) |
| rand write 4K IOPS (incl fsync) | 37057.0 (35377.9..48004.5) | 496474.4 (495048.5..568225.6) | 20667.0 (15255.3..23644.8) | 9892.9 (3123.3..11410.9) |
| create 4 KiB files /s | 28252.2 (26712.1..30916.3) | 94954.9 (80143.5..109612.3) | 7888.3 (6783.3..8268.9) | 14934.1 (13658.4..16626.1) |
| stat /s | 376531.4 (347000.2..455913.3) | 384346.6 (369560.8..504376.1) | 50224.8 (46782.9..51022.6) | 62122.3 (58396.4..76875.9) |
| open+read+close /s | 85145.4 (72896.9..97602.3) | 97510.8 (88838.1..109463.7) | 14021.2 (13943.3..15737.6) | 18841.3 (17446.8..20203.4) |
| readdir entries /s | 2443617.6 (2167907.3..2906027.8) | 3929453.2 (3202835.6..4162579.2) | 605764.5 (460706.7..640060.9) | 2248159.3 (2219222.2..2785983.5) |
| rename /s | 38896.1 (34045.3..39230.4) | 169672.3 (162938.6..173017.0) | 511.8 (485.2..516.0) | 32221.9 (31543.8..34029.8) |
| unlink /s | 72657.7 (70960.9..97074.4) | 171541.0 (144011.1..216235.7) | 19538.8 (18600.9..20729.6) | 78708.3 (58165.5..82105.2) |
| fsync latency ms (append 4 KiB) | 2.1 (2.1..2.3) | 0.0 (0.0..0.0) | 2.2 (2.0..2.2) | 8.4 (7.8..8.9) |

#### After optimisation (`4fcdcfb`)

| workload | ext4 | tmpfs | fuse-passthrough | timefs |
|---|---|---|---|---|
| seq write MB/s (incl fsync) | 397.7 (130.4..439.8) | 2586.3 (2207.1..3013.1) | 310.7 (310.5..327.2) | 632.8 (448.9..657.4) |
| seq read warm MB/s | 3698.1 (3677.7..9611.8) | 7163.5 (7155.0..7775.0) | 2036.9 (2033.5..2092.1) | 6539.2 (5549.2..8594.7) |
| seq read cold MB/s | 385.6 (385.3..385.8) | 6385.3 (6280.7..7388.6) | 2392.4 (1998.8..2486.8) | 2191.5 (2089.7..2373.5) |
| rand read 4K IOPS (cold) | 4956.5 (4843.6..5765.0) | 459531.8 (453505.0..460042.5) | 63712.5 (49658.8..65768.2) | 54443.0 (52150.5..59268.5) |
| rand write 4K IOPS (incl fsync) | 38003.9 (32484.8..39164.1) | 489916.4 (432250.4..490203.4) | 27199.7 (25203.6..28474.2) | 21053.0 (4476.6..21302.2) |
| create 4 KiB files /s | 29673.1 (20052.8..31235.9) | 98291.4 (94982.6..105417.0) | 8906.9 (8489.6..9344.2) | 17258.9 (15396.0..18469.5) |
| stat /s | 442424.0 (293320.4..464407.3) | 509534.8 (509053.8..512542.6) | 58722.8 (55217.7..63256.7) | 73602.0 (64160.8..83215.3) |
| open+read+close /s | 102195.2 (65690.3..102584.7) | 113534.0 (113227.8..114973.0) | 17371.0 (16589.9..18516.5) | 31800.8 (22400.6..36455.8) |
| readdir entries /s | 2848567.3 (2038454.8..2880286.6) | 4122289.1 (4025376.0..4373184.2) | 730496.1 (626731.3..731862.4) | 2632133.1 (2078636.2..2774068.7) |
| rename /s | 37754.2 (36827.2..40043.4) | 210871.2 (208735.0..213635.4) | 532.2 (525.8..536.1) | 30327.8 (24297.3..42783.1) |
| unlink /s | 89683.9 (55997.9..96641.9) | 221641.2 (181327.3..225018.1) | 22991.4 (22428.7..25451.4) | 77110.2 (66175.9..90872.2) |
| fsync latency ms (append 4 KiB) | 2.1 (1.7..2.1) | 0.0 (0.0..0.0) | 2.1 (2.1..2.2) | 9.4 (9.0..10.1) |

Reading the tables:
* TimeFS writes faster than the passthrough because data stays in memory until a checkpoint;
  the cost moves to `fsync`.
* Metadata operations run from the in-memory tree, so TimeFS beats the passthrough (which
  does real host syscalls) on every metadata workload.
* "ext4 seq read cold" is disk bound (385 MB/s); FUSE targets read from the host page cache
  underneath, so their "cold" is only cold for the FUSE layer.
* The passthrough's rename rate is an artefact of the baseline (it rewrites an in-memory path
  map with a linear scan), not of FUSE.

### Optimisation log

Every change was measured with the scenario named; numbers are medians.

| # | Bottleneck found | Change | Effect | Kept |
|---|---|---|---|---|
| 1 | `store.sync` fsynced every object file one after another (about 2.4 ms each) | fsync in parallel on scoped threads, so ext4 merges them into shared journal commits | import of 1,000 files 4792 ms to 662 ms; 100-file snapshot 471 ms to 70 ms (snapshot scenario) | yes |
| 2 | snapshot synced objects and the commit object in separate rounds | create the commit before one sync | no measurable change (about 18 ms both) | yes (harmless, simpler) |
| 3 | small batches (under 8 paths) stayed sequential | parallel threshold 8 to 2 | 1-file snapshot 18 ms to 9.5 ms | yes |
| 4 | every read opened, read and BLAKE3-verified a whole block | `CachedStore`: two-generation read cache (64 MiB); immutable objects never go stale | warm seq read 821 to 2114 MB/s; cold 4K random read 15.3k to 42.0k IOPS (mount workloads) | yes |
| 5 | three fsync rounds per object sync | one combined parallel round (files, dirs, root) | fsync latency not better (A/B, 3 runs each: 9.5 to 10.9 ms vs 8.9 to 9.9 ms for two rounds) | no, reverted to two rounds |
| 6 | kernel dropped its page cache on every open | `FOPEN_KEEP_CACHE` (all writes go through the mount) | warm seq read 2114 to 6539 MB/s; open+read+close 18.2k to 31.8k /s | yes |
| 7 | loose store: two journal rounds for objects plus two for the ref; one file and inode per object | pack store (ADR-0009): objects and refs in one log, one `fdatasync` per checkpoint | mount fsync 35.3 to 2.0 ms in the same session; 100k-file import 45.3 s to 4.1 s; disk usage 1006 to 410 MiB | yes, default |

### Open bottlenecks (measured, not yet fixed)

* **Flat extent lists:** a 4 KiB write to a 32 MiB file at 4 KiB blocks rewrites a 280 KiB
  file object (Q7); extent pages (ADR-0009 part 2) bound this and lift the 64 GiB file cap.
* **Inode memory:** nodes of every touched path stay in memory for the life of a mount.
* Resolved in M7 by the pack store: fsync latency, small-object disk and inode overhead, and
  the slow initial import (all measured above).
