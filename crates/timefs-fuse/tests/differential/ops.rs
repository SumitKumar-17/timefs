//! The operation vocabulary and its random generator.
//!
//! Paths come from a deliberately tiny namespace (three names, depth up to three) so that
//! operations keep colliding: renames land on existing targets, creates hit directories,
//! removals hit non-empty directories and symlinks point at things that come and go.
//! Rarely a component has the maximum name length, or one byte more, to check that the
//! limit and its error code match the host.

use crate::support::rng::Rng;

const NAMES: [&str; 3] = ["a", "b", "c"];
const MAX_DEPTH: u64 = 3;
/// Linux `NAME_MAX`.
const NAME_MAX: usize = 255;
/// Number of long-lived open handles; they model open-unlink-read sequences.
pub const SLOTS: usize = 3;

/// A filesystem operation, applied identically to TimeFS and to the host oracle.
#[derive(Clone, Debug)]
pub enum Op {
    Write {
        path: String,
        offset: u64,
        len: usize,
        fill: u64,
    },
    Read {
        path: String,
        offset: u64,
        len: usize,
    },
    Append {
        path: String,
        len: usize,
        fill: u64,
    },
    /// Open with `O_TRUNC`.
    Truncate {
        path: String,
    },
    SetLen {
        path: String,
        len: u64,
    },
    Mkdir {
        path: String,
        mode: u32,
    },
    Rmdir {
        path: String,
    },
    Unlink {
        path: String,
    },
    Rename {
        from: String,
        to: String,
    },
    Symlink {
        target: String,
        path: String,
    },
    Readlink {
        path: String,
    },
    Chmod {
        path: String,
        mode: u32,
    },
    Fsync {
        path: String,
    },
    Stat {
        path: String,
    },
    ReadDir {
        path: String,
    },
    Hold {
        slot: usize,
        path: String,
    },
    ReadHeld {
        slot: usize,
    },
    WriteHeld {
        slot: usize,
        offset: u64,
        len: usize,
        fill: u64,
    },
    Release {
        slot: usize,
    },
}

/// One step of a run: a compared filesystem operation or a TimeFS-only control request.
#[derive(Clone, Debug)]
pub enum Step {
    Fs(Op),
    Control(&'static str),
}

pub fn generate(rng: &mut Rng) -> Step {
    if rng.one_in(60) {
        return Step::Control(rng.pick(&["snap stress", "checkpoint"]));
    }
    Step::Fs(random_op(rng))
}

fn random_op(rng: &mut Rng) -> Op {
    let path = random_path(rng);
    match rng.below(23) {
        0..=4 => Op::Write {
            path,
            offset: offset(rng),
            len: length(rng),
            fill: fill(rng),
        },
        5 | 6 => Op::Read {
            path,
            offset: offset(rng),
            len: length(rng),
        },
        7 => Op::Append {
            path,
            len: length(rng),
            fill: fill(rng),
        },
        8 => Op::Truncate { path },
        9 => Op::SetLen {
            path,
            len: offset(rng),
        },
        10 => Op::Mkdir {
            path,
            mode: mode(rng),
        },
        11 => Op::Rmdir { path },
        12 => Op::Unlink { path },
        13 | 14 => Op::Rename {
            from: path,
            to: random_path(rng),
        },
        15 => Op::Symlink {
            target: random_path(rng),
            path,
        },
        16 => Op::Readlink { path },
        17 => Op::Chmod {
            path,
            mode: mode(rng),
        },
        18 => Op::Fsync { path },
        19 => {
            if rng.one_in(2) {
                Op::Stat { path }
            } else {
                Op::ReadDir { path }
            }
        }
        // Handle operations get extra weight so that held files often get unlinked.
        _ => handle_op(rng, path),
    }
}

fn handle_op(rng: &mut Rng, path: String) -> Op {
    let slot = rng.below(SLOTS as u64) as usize;
    match rng.below(4) {
        0 => Op::Hold { slot, path },
        1 => Op::ReadHeld { slot },
        2 => Op::WriteHeld {
            slot,
            offset: offset(rng),
            len: length(rng),
            fill: fill(rng),
        },
        _ => Op::Release { slot },
    }
}

/// Shallow paths are favoured so that the deeper ones usually have an existing parent.
fn random_path(rng: &mut Rng) -> String {
    let depth = match rng.below(10) {
        0..=4 => 1,
        5..=7 => 2,
        _ => MAX_DEPTH,
    };
    let parts: Vec<String> = (0..depth).map(|_| component(rng)).collect();
    parts.join("/")
}

fn component(rng: &mut Rng) -> String {
    match rng.below(80) {
        0 => "m".repeat(NAME_MAX),
        1 => "n".repeat(NAME_MAX + 1),
        _ => rng.pick(&NAMES).to_string(),
    }
}

/// Mostly small offsets (same block), some across 64 KiB blocks, a few far past EOF so
/// that holes appear.
fn offset(rng: &mut Rng) -> u64 {
    match rng.below(10) {
        0..=4 => rng.below(8192),
        5..=7 => rng.below(300_000),
        8 => rng.below(2_000_000),
        _ => 4_000_000 + rng.below(1 << 20),
    }
}

fn length(rng: &mut Rng) -> usize {
    let len = match rng.below(4) {
        0 => rng.below(16),
        1 => rng.below(4096),
        2 => rng.below(70_000),
        _ => rng.below(300_000),
    };
    len as usize + 1
}

fn fill(rng: &mut Rng) -> u64 {
    if rng.one_in(8) {
        0
    } else {
        rng.next_u64() | 1
    }
}

/// Owner read and execute are always kept so the comparison walk can read every file and
/// enter every directory; owner write is dropped sometimes to exercise `EACCES`.
fn mode(rng: &mut Rng) -> u32 {
    let owner_write = if rng.one_in(4) { 0 } else { 0o200 };
    0o500 | owner_write | rng.below(0o100) as u32
}
