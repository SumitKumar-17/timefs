//! Core-level scenarios: no FUSE involved, so they isolate the versioning engine's cost.

mod amplification;
mod branch;
mod common;
mod dedupe;
mod merge;
mod snapshot;
mod store;

use std::error::Error;

use crate::env::Config;

pub type Res = Result<(), Box<dyn Error>>;

/// Names accepted on the command line, in the order `all` runs them.
pub const NAMES: [&str; 6] = [
    "snapshot",
    "branch",
    "merge",
    "amplification",
    "store",
    "dedupe",
];

pub fn run(name: &str, cfg: &Config) -> Res {
    match name {
        "snapshot" => snapshot::run(cfg),
        "branch" => branch::run(cfg),
        "merge" => merge::run(cfg),
        "amplification" => amplification::run(cfg),
        "store" => store::run(cfg),
        "dedupe" => dedupe::run(cfg),
        other => Err(format!("unknown scenario {other:?}").into()),
    }
}
