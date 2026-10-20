# Testing strategy

| Layer | Technique | Where |
|-------|-----------|-------|
| Codec, hashing, chunking | Unit + property tests (round-trip, canonical-form rejection, chunker covers input exactly) | `timefs-core` |
| Object store | Unit tests, concurrent-put test, corruption detection | `timefs-core` |
| Repository / refs | Unit tests incl. fault injection (leftover temp files, torn refs) | `timefs-core` |
| WorkTree | **Model-based property tests**: random sequences of POSIX operations are applied to both the WorkTree and a trivial in-memory reference model; results must match, including after checkpoint → reload | `timefs-core` |
| Diff / merge | Unit + property tests (merge(a,a)=a; one-sided merges; merge with base = commutative on disjoint changes) | `timefs-core` |
| GC / fsck | Create garbage, verify reachable data survives and garbage goes; corrupt an object, verify detection | `timefs-core` |
| FUSE | End-to-end integration tests that really mount (skipped automatically when `/dev/fuse` is unavailable) and drive the mount with ordinary `std::fs` calls | `timefs-fuse/tests`, `scripts/` |
| Conformance | pjdfstest / fsx / fsstress runs against a mount (manual / scheduled; results recorded in `docs/benchmarks.md` or the roadmap, never invented) | `scripts/` |

Rules:

* A bug fix lands with a regression test.
* Tests that need FUSE must skip (not fail) when FUSE is unavailable, and CI must still
  exercise them on a runner that has `/dev/fuse`.
* No test depends on wall-clock timing for correctness.
