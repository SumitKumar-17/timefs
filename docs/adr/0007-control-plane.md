# ADR-0007: Unix-socket control plane + offline repo commands

**Status:** Accepted (implemented)

## Context
* The live working tree is in the daemon's memory, so `snap`, `checkpoint` and `status` must run in the daemon.
* Other commands only need the repository.

## Decision
* The daemon listens on `<repo>/timefs.sock` with a newline-delimited text protocol: `snap [message]`, `checkpoint`, `status`.
* The CLI uses the socket when a daemon runs; `snap` falls back to an offline snapshot when none is running.
* `merge`, `checkout`, `reset` and `gc` take the repo `flock` and refuse with "the repository is mounted; unmount it first" if a daemon holds it.
* `log`, `ls`, `cat`, `diff`, `branch`, `fsck`, `stats` operate directly on the repository.

## Alternatives
* **Virtual control files in the mount (`/.timefs/ctl`):** convenient, but mixes data and control; maybe later for read-only introspection.
* **ioctl on the mount:** the CLI must be inside the mount; awkward.
* **D-Bus or gRPC:** heavy dependencies for a handful of commands.

## Consequences
* A stale socket file must be detected (connect refused, then remove).
* Live `merge`/`checkout` on a mounted tree is future work, not v1.
