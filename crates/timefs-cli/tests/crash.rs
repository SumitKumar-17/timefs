//! Crash injection: SIGKILL a real `timefs mount` daemon in the middle of a random
//! workload, then check the durability contract from `docs/architecture.md` section 5:
//!
//! * the repository passes `timefs fsck`;
//! * the next mount starts;
//! * the recovered tree is exactly a state the writer passed through since its last
//!   acknowledged sync, so nothing fsynced is missing and nothing is torn or corrupt;
//! * every recovered file is readable (no `EIO`).
//!
//! `TIMEFS_CRASH_ROUNDS` sets the number of rounds (default 3) and `TIMEFS_CRASH_SEED`
//! the first seed. Each round uses a fresh repository and crashes it several times in a
//! row, so recovery from a recovered state is covered too.

// Helper modules live in `tests/crash/`; an inline module keeps cargo from treating
// them as separate test targets.
mod crash {
    pub mod daemon;
    pub mod model;
    pub mod rng;
    pub mod workload;
}

use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use crash::daemon::{fuse_unavailable, is_mounted, timefs, Daemon};
use crash::model::Tree;
use crash::rng::Rng;
use crash::workload::{Outcome, Signals, Writer};

const DEFAULT_ROUNDS: u64 = 3;
const DEFAULT_SEED: u64 = 0x7157_0007;
const CRASHES_PER_ROUND: usize = 3;
/// The crash lands after this many completed operations (or later, see `crash_after`).
const MIN_OPS_BEFORE_CRASH: usize = 3;
const MAX_OPS_BEFORE_CRASH: usize = 200;

#[test]
fn sigkill_during_random_workload_keeps_synced_data() {
    if let Some(why) = fuse_unavailable() {
        return skip(&why);
    }
    let rounds = env_u64("TIMEFS_CRASH_ROUNDS").unwrap_or(DEFAULT_ROUNDS);
    let first_seed = env_u64("TIMEFS_CRASH_SEED").unwrap_or(DEFAULT_SEED);
    for round in 0..rounds {
        let seed = first_seed + round;
        // Alternate the two on-disk layouts so both recovery paths are exercised.
        let store = if round % 2 == 0 { "pack" } else { "loose" };
        eprintln!(
            "crash round {round}: {store} store, seed {seed} (rerun with TIMEFS_CRASH_SEED={seed})"
        );
        if !run_round(seed, store, round == 0) {
            return;
        }
    }
}

/// Returns false when the round was skipped because FUSE turned out to be unusable.
fn run_round(seed: u64, store: &str, may_skip: bool) -> bool {
    let env = RoundEnv::new(store);
    let mut rng = Rng::new(seed);
    let mut state = Tree::default();
    let mut interval = checkpoint_interval(&mut rng);
    let mut daemon = match Daemon::start(&env.repo, &env.mnt, interval) {
        Ok(d) => d,
        // Only the very first mount of the run may fail for environmental reasons;
        // later failures are recovery bugs.
        Err(why) if may_skip => {
            skip(&why);
            return false;
        }
        Err(why) => panic!("seed {seed}: first mount failed: {why}"),
    };

    for crash in 0..CRASHES_PER_ROUND {
        let ctx = format!("seed {seed}, crash {crash}, checkpoint interval {interval}s");
        let outcome = run_until_crash(daemon, &env, rng.next_u64(), state, &mut rng, interval);
        eprintln!(
            "  {ctx}: {} ops, {} syncs, {} allowed states",
            outcome.ops,
            outcome.syncs,
            outcome.candidates.len()
        );
        assert!(!is_mounted(&env.mnt), "{ctx}: dead mount still attached");
        fsck(&env.repo, &ctx);

        interval = checkpoint_interval(&mut rng);
        daemon = Daemon::start(&env.repo, &env.mnt, interval)
            .unwrap_or_else(|why| panic!("{ctx}: mount after crash failed: {why}"));
        state = verify_recovery(&env.mnt, &outcome, &ctx);
    }

    // A clean unmount checkpoints, so the next mount must show exactly the same tree.
    daemon.unmount();
    fsck(&env.repo, &format!("seed {seed}, after clean unmount"));
    let daemon = Daemon::start(&env.repo, &env.mnt, 0).expect("final mount");
    let after = Tree::read_from(&env.mnt).expect("read after clean unmount");
    assert!(
        after == state,
        "seed {seed}: clean unmount lost changes:\n{}",
        after.diff(&state)
    );
    daemon.unmount();
    true
}

/// Mostly no timer (only explicit syncs persist), sometimes a 1 s timer so periodic
/// checkpoints race with the workload as well.
fn checkpoint_interval(rng: &mut Rng) -> u64 {
    if rng.chance(30) {
        1
    } else {
        0
    }
}

/// Run the writer on a thread, SIGKILL the daemon at a seeded moment, and return what
/// the writer recorded.
fn run_until_crash(
    daemon: Daemon,
    env: &RoundEnv,
    writer_seed: u64,
    base: Tree,
    rng: &mut Rng,
    interval: u64,
) -> Outcome {
    let signals = Signals::default();
    let writer = Writer::new(&env.mnt, &env.repo, writer_seed, base, signals.clone());
    let handle = thread::spawn(move || writer.run());

    let target = rng.range(MIN_OPS_BEFORE_CRASH, MAX_OPS_BEFORE_CRASH);
    // With a timer, also let some wall-clock time pass so periodic checkpoints happen
    // before the crash. Timing only moves the crash point; correctness never depends on it.
    let extra_wait = if interval > 0 { rng.below(2_500) } else { 0 };
    crash_after(
        &signals,
        &handle,
        target,
        Duration::from_millis(extra_wait as u64),
    );

    signals
        .killed
        .store(true, std::sync::atomic::Ordering::SeqCst);
    daemon.crash();
    match handle.join() {
        Ok(outcome) => outcome,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

fn crash_after(
    signals: &Signals,
    writer: &thread::JoinHandle<Outcome>,
    ops: usize,
    extra: Duration,
) {
    let poll = Duration::from_micros(200);
    while signals.ops.load(std::sync::atomic::Ordering::SeqCst) < ops {
        if writer.is_finished() {
            return; // the writer panicked; crash now and report its failure
        }
        thread::sleep(poll);
    }
    let mut waited = Duration::ZERO;
    while waited < extra && !writer.is_finished() {
        thread::sleep(poll * 50);
        waited += poll * 50;
    }
}

/// The recovered tree must be readable and equal to one of the allowed states.
fn verify_recovery(mnt: &Path, outcome: &Outcome, ctx: &str) -> Tree {
    let recovered = Tree::read_from(mnt)
        .unwrap_or_else(|e| panic!("{ctx}: reading the recovered tree failed: {e}"));
    let synced = &outcome.candidates[0];
    match outcome.candidates.iter().position(|c| *c == recovered) {
        Some(i) => {
            eprintln!("  {ctx}: recovered allowed state {i}");
            recovered
        }
        None => panic!(
            "{ctx}: recovered tree matches no state since the last sync; \
             difference from the last synced state (left recovered, right synced):\n{}",
            recovered.diff(synced)
        ),
    }
}

fn fsck(repo: &Path, ctx: &str) {
    let out = timefs(repo).arg("fsck").output().expect("run timefs fsck");
    assert!(
        out.status.success(),
        "{ctx}: fsck failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A repository and mountpoint in a temporary directory.
struct RoundEnv {
    _dir: tempfile::TempDir,
    repo: PathBuf,
    mnt: PathBuf,
}

impl RoundEnv {
    fn new(store: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        // Canonical, because the mount table lists canonical paths.
        let root = dir.path().canonicalize().unwrap();
        let repo = root.join("repo");
        let mnt = root.join("mnt");
        std::fs::create_dir(&mnt).unwrap();
        let out = timefs(&repo)
            .args(["init", "--store", store])
            .output()
            .unwrap();
        assert!(out.status.success(), "init failed");
        RoundEnv {
            _dir: dir,
            repo,
            mnt,
        }
    }
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok().map(|v| {
        v.parse()
            .unwrap_or_else(|_| panic!("{name} must be a number"))
    })
}

fn skip(why: &str) {
    if std::env::var("TIMEFS_REQUIRE_FUSE").is_ok_and(|v| v == "1") {
        panic!("FUSE is required but unavailable: {why}");
    }
    eprintln!("SKIPPED: {why}");
}
