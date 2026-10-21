#!/usr/bin/env bash
# Mount-level benchmark: ext4 vs tmpfs vs FUSE passthrough vs TimeFS.
#
# Usage: BENCH_ARGS="--repeat 3" STORE=pack|loose scripts/bench-mount.sh [workdir]
# The workdir must live on the disk under test (default: ./bench-out, not tmpfs).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="${1:-$ROOT/bench-out}"
BIN="$ROOT/target/release"
RUN="$WORK/mount-run"
PIDS=()

cleanup() {
    for mnt in "$RUN/timefs-mnt" "$RUN/passthrough-mnt"; do
        mountpoint -q "$mnt" 2>/dev/null && fusermount3 -u "$mnt" 2>/dev/null || true
    done
    for pid in "${PIDS[@]:-}"; do
        [ -n "$pid" ] && kill -INT "$pid" 2>/dev/null || true
    done
    wait 2>/dev/null || true
    rm -rf "$RUN"
}
trap cleanup EXIT

wait_mounted() {
    for _ in $(seq 1 50); do
        mountpoint -q "$1" && return 0
        sleep 0.1
    done
    echo "mount of $1 did not appear" >&2
    exit 1
}

cargo build --release -q -p timefs-bench -p timefs-cli --manifest-path "$ROOT/Cargo.toml"
rm -rf "$RUN"
mkdir -p "$RUN/ext4" "$RUN/passthrough-src" "$RUN/passthrough-mnt" "$RUN/timefs-mnt" /dev/shm/timefs-bench

"$BIN/timefs-bench" --work "$WORK" passthrough "$RUN/passthrough-src" "$RUN/passthrough-mnt" &
PIDS+=($!)
wait_mounted "$RUN/passthrough-mnt"

"$BIN/timefs" --repo "$RUN/repo" init --store "${STORE:-pack}" >/dev/null
"$BIN/timefs" --repo "$RUN/repo" mount "$RUN/timefs-mnt" --checkpoint-interval 0 >/dev/null &
PIDS+=($!)
wait_mounted "$RUN/timefs-mnt"

"$BIN/timefs-bench" --work "$WORK" ${BENCH_ARGS:---repeat 3} workloads \
    --target "ext4=$RUN/ext4" \
    --target "tmpfs=/dev/shm/timefs-bench" \
    --target "fuse-passthrough=$RUN/passthrough-mnt" \
    --target "timefs-${STORE:-pack}=$RUN/timefs-mnt"
