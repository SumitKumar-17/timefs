# ADR-0007: Unix-socket control plane + offline repo commands

**Status:** Accepted

## Context
The live working tree lives in the daemon's memory, so some operations (`snap`, `status`)
must be executed by the daemon. Others (`log`, `diff`, `stats`, `fsck`, `gc`) only need the repository.

## Decision
The daemon listens on `<repo>/timefs.sock` with a tiny newline-delimited text protocol
(`snap <message>`, `status`, `checkpoint`, `branch <name>`). The CLI uses the socket when a
daemon is running and operates on the repository directly otherwise. Operations that must
exclude a live working tree (`gc`, `merge`, `checkout`, repair) take the repo `flock` and
refuse with a clear message if a daemon holds it.

## Alternatives
* **Virtual control files inside the mount** (`/.timefs/ctl`) — convenient for shell
  users, but pollutes the namespace and mixes data and control; may be added later for read-only introspection.
* **ioctl on the mount** — needs the CLI to be inside the mount; awkward.
* **D-Bus/gRPC** — heavy dependencies for a handful of commands.

## Consequences
* A stale socket file must be detected (connect-refused → remove).
* Live `merge`/`checkout` against a mounted tree is a roadmap item, not v1.
