//! Core-level scenarios: no FUSE involved, so they isolate the versioning engine's cost.

mod amplification;
mod branch;
mod common;
mod dedupe;
mod fsync;
mod merge;
mod snapshot;
mod store;

use std::error::Error;

use crate::env::Config;

pub type Res = Result<(), Box<dyn Error>>;

type Scenario = fn(&Config) -> Res;

/// Every scenario, in the order a full run executes them. This table is the single source
/// for both the list of names and the dispatch, so the two cannot drift apart.
const SCENARIOS: [(&str, Scenario); 7] = [
    ("snapshot", snapshot::run),
    ("branch", branch::run),
    ("merge", merge::run),
    ("amplification", amplification::run),
    ("store", store::run),
    ("dedupe", dedupe::run),
    ("fsync", fsync::run),
];

pub fn names() -> impl Iterator<Item = &'static str> {
    SCENARIOS.iter().map(|(name, _)| *name)
}

pub fn run(name: &str, cfg: &Config) -> Res {
    match SCENARIOS.iter().find(|(n, _)| *n == name) {
        Some((_, scenario)) => scenario(cfg),
        None => Err(format!(
            "unknown scenario {name:?} (known: {})",
            names().collect::<Vec<_>>().join(", ")
        )
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scenario_is_listed_once() {
        let all: Vec<&str> = names().collect();
        let unique: std::collections::BTreeSet<&str> = all.iter().copied().collect();
        assert_eq!(all.len(), unique.len());
        assert!(all.contains(&"fsync"));
    }
}
