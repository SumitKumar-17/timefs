//! Drives one seeded run: generate steps, apply them to both sides, compare.

use std::collections::VecDeque;
use std::fmt;
use std::io;

use timefs_fuse::control::request;
use timefs_fuse::{MountError, Mounted};

use crate::common::Env;
use crate::support::apply::Side;
use crate::support::ops::{generate, Step};
use crate::support::rng::Rng;
use crate::support::tree::{differences, make_removable, snapshot};

/// Trees are compared this often, in addition to the remount and the end of the run.
const TREE_CHECK_EVERY: usize = 50;
const TRACE_LEN: usize = 25;
const DIFF_LINES: usize = 20;

#[derive(Debug, thiserror::Error)]
pub enum Divergence {
    #[error("{op} returned {timefs} on TimeFS but {oracle} on the host")]
    Outcome {
        op: String,
        timefs: String,
        oracle: String,
    },
    #[error("trees differ ({when}):\n  {}", .lines.join("\n  "))]
    Tree {
        when: &'static str,
        lines: Vec<String>,
    },
    #[error("control request {line:?} failed: {reply}")]
    Control { line: &'static str, reply: String },
    #[error("cannot snapshot the {side} tree: {source}")]
    Walk {
        side: &'static str,
        source: io::Error,
    },
    #[error("unmount failed: {0}")]
    Unmount(#[from] MountError),
    #[error("remount failed although the first mount worked")]
    Remount,
}

/// A divergence with enough context to understand and replay it.
#[derive(Debug)]
pub struct Failure {
    pub step: usize,
    pub error: Divergence,
    pub recent: Vec<String>,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "at step {}: {}", self.step, self.error)?;
        writeln!(f, "last steps:")?;
        self.recent.iter().try_for_each(|s| writeln!(f, "  {s}"))
    }
}

struct Run<'a> {
    env: &'a Env,
    mount: Option<Mounted>,
    timefs: Side,
    oracle: Side,
    recent: VecDeque<String>,
}

/// Apply `ops` random steps derived from `seed`. Halfway through, the filesystem is
/// unmounted and mounted again, and the tree must come back unchanged.
pub fn run(env: &Env, mounted: Mounted, seed: u64, ops: usize) -> Result<(), Failure> {
    let oracle_root = env.dir.path().join("oracle");
    std::fs::create_dir(&oracle_root).expect("create oracle directory");
    let mut run = Run {
        env,
        mount: Some(mounted),
        timefs: Side::new(env.mnt.clone()),
        oracle: Side::new(oracle_root.clone()),
        recent: VecDeque::new(),
    };
    let result = run.steps(seed, ops);
    run.timefs.release_all();
    run.oracle.release_all();
    let unmounted = run.mount.take().map(Mounted::unmount);
    make_removable(&oracle_root);
    result?;
    if let Some(Err(e)) = unmounted {
        return Err(run.fail(ops, e.into()));
    }
    Ok(())
}

impl Run<'_> {
    fn steps(&mut self, seed: u64, ops: usize) -> Result<(), Failure> {
        let mut rng = Rng::new(seed);
        for i in 0..ops {
            let step = generate(&mut rng);
            self.remember(i, &step);
            self.step(&step).map_err(|e| self.fail(i, e))?;
            if i == ops / 2 {
                self.remember(i, &"remount");
                self.remount().map_err(|e| self.fail(i, e))?;
            } else if i % TREE_CHECK_EVERY == 0 {
                self.compare_trees("periodic check")
                    .map_err(|e| self.fail(i, e))?;
            }
        }
        self.compare_trees("end of run")
            .map_err(|e| self.fail(ops, e))
    }

    fn step(&mut self, step: &Step) -> Result<(), Divergence> {
        match step {
            Step::Fs(op) => {
                let (timefs, oracle) = (self.timefs.apply(op), self.oracle.apply(op));
                if timefs != oracle {
                    return Err(Divergence::Outcome {
                        op: format!("{op:?}"),
                        timefs: format!("{timefs:?}"),
                        oracle: format!("{oracle:?}"),
                    });
                }
                Ok(())
            }
            Step::Control(line) => self.control(line),
        }
    }

    /// Snapshots and checkpoints change nothing visible, so only success is checked here;
    /// later comparisons show whether they disturbed the live tree.
    fn control(&self, line: &'static str) -> Result<(), Divergence> {
        let reply = request(&self.env.socket(), line).unwrap_or_else(|e| format!("io error: {e}"));
        if reply.starts_with("ok") {
            Ok(())
        } else {
            Err(Divergence::Control { line, reply })
        }
    }

    fn remount(&mut self) -> Result<(), Divergence> {
        self.compare_trees("before remount")?;
        // Open handles would keep the mount busy; both sides drop theirs to stay in step.
        self.timefs.release_all();
        self.oracle.release_all();
        if let Some(m) = self.mount.take() {
            m.unmount()?;
        }
        self.mount = Some(self.env.mount().ok_or(Divergence::Remount)?);
        self.compare_trees("after remount")
    }

    fn compare_trees(&self, when: &'static str) -> Result<(), Divergence> {
        let walk = |side: &Side, name| {
            snapshot(side.root()).map_err(|source| Divergence::Walk { side: name, source })
        };
        let lines = differences(
            &walk(&self.timefs, "TimeFS")?,
            &walk(&self.oracle, "host")?,
            DIFF_LINES,
        );
        if lines.is_empty() {
            Ok(())
        } else {
            Err(Divergence::Tree { when, lines })
        }
    }

    fn remember(&mut self, i: usize, what: &dyn fmt::Debug) {
        if self.recent.len() == TRACE_LEN {
            self.recent.pop_front();
        }
        self.recent.push_back(format!("{i}: {what:?}"));
    }

    fn fail(&self, step: usize, error: Divergence) -> Failure {
        Failure {
            step,
            error,
            recent: self.recent.iter().cloned().collect(),
        }
    }
}
