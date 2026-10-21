//! Space reclamation shared by backends that store one deletable unit per object.

use std::collections::HashSet;
use std::time::{Duration, SystemTime};

use crate::error::Result;
use crate::hash::Hash;
use crate::store::ObjectStore;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub examined: u64,
    pub reachable: u64,
    pub deleted: u64,
    pub bytes_freed: u64,
    /// Unreachable but kept because of the grace period.
    pub kept_recent: u64,
    pub temp_files_removed: u64,
}

/// Delete every object outside `keep` that is older than `grace`, one by one.
pub fn sweep_by_deletion(
    store: &dyn ObjectStore,
    keep: &HashSet<Hash>,
    grace: Duration,
    dry_run: bool,
) -> Result<SweepReport> {
    let mut report = SweepReport::default();
    let now = SystemTime::now();
    for info in store.list()? {
        report.examined += 1;
        if keep.contains(&info.hash) {
            report.reachable += 1;
            continue;
        }
        if now.duration_since(info.modified).unwrap_or_default() < grace {
            report.kept_recent += 1;
            continue;
        }
        if !dry_run {
            store.delete(&info.hash)?;
        }
        report.deleted += 1;
        report.bytes_freed += info.size;
    }
    Ok(report)
}
