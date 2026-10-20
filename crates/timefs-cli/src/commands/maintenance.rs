//! Integrity checking and statistics.

use std::time::Duration;

use timefs_core::fsck::fsck as run_fsck;
use timefs_core::gc::{gc as run_gc, GcOptions};
use timefs_core::{treeops, Repository};

use super::guard::exclusive;
use super::CmdResult;
use crate::cli::GcArgs;
use crate::format::human_bytes;

pub fn fsck(repo: &Repository) -> CmdResult {
    let report = run_fsck(repo)?;
    println!(
        "Checked {} objects ({})",
        report.objects_checked,
        human_bytes(report.bytes_checked)
    );
    for (hash, why) in &report.corrupt {
        println!("corrupt: {hash}: {why}");
    }
    for (hash, by) in &report.missing {
        println!("missing: {hash} (referenced by {by})");
    }
    for bad in &report.bad_refs {
        println!("bad ref: {bad}");
    }
    if report.unreachable > 0 {
        println!("{} unreachable objects (reclaimable)", report.unreachable);
    }
    if report.is_clean() {
        println!("Repository is healthy");
        Ok(())
    } else {
        Err("repository has errors".into())
    }
}

pub fn stats(repo: &Repository) -> CmdResult {
    let objects = repo.store().list()?;
    let stored: u64 = objects.iter().map(|o| o.size).sum();
    println!("objects:  {}", objects.len());
    println!("stored:   {}", human_bytes(stored));
    println!("branches: {}", repo.branches()?.len());

    if let Ok(head) = repo.resolve("HEAD") {
        let tree = repo.get_commit(&head)?.tree;
        let mut logical = 0u64;
        treeops::walk(repo.store().as_ref(), &tree, &mut |_, e| {
            logical += e.size;
            Ok(())
        })?;
        println!("HEAD logical size: {}", human_bytes(logical));
        if stored > 0 {
            println!(
                "HEAD logical / stored: {:.2}x",
                logical as f64 / stored as f64
            );
        }
    }
    Ok(())
}

pub fn gc(repo: &Repository, args: GcArgs) -> CmdResult {
    let lock = exclusive(repo)?;
    let opts = GcOptions {
        grace: Duration::from_secs(args.grace_secs),
        dry_run: args.dry_run,
    };
    let r = run_gc(repo, &lock, opts)?;
    let verb = if args.dry_run {
        "Would delete"
    } else {
        "Deleted"
    };
    println!("Examined {} objects, {} reachable", r.examined, r.reachable);
    println!(
        "{verb} {} unreachable objects ({})",
        r.deleted,
        human_bytes(r.bytes_freed)
    );
    if r.kept_recent > 0 {
        println!(
            "Kept {} recent unreachable objects (grace period)",
            r.kept_recent
        );
    }
    if r.temp_files_removed > 0 {
        println!("Removed {} stale temporary files", r.temp_files_removed);
    }
    Ok(())
}
