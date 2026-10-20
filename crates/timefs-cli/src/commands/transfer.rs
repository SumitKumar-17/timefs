//! Moving trees between the host filesystem and the repository.

use timefs_core::hostfs::{export_tree, import_dir};
use timefs_core::Repository;

use super::guard::{exclusive, require_snapshotted};
use super::{author, CmdResult};
use crate::cli::{ExportArgs, ImportArgs};
use crate::format::human_bytes;

/// Import a host directory as a new commit on the current branch. Moving the branch
/// under a live mount would be reverted by its next snapshot, and moving it past a
/// pending checkpoint would orphan that checkpoint, so both are refused.
pub fn import(repo: &Repository, args: ImportArgs) -> CmdResult {
    let _lock = exclusive(repo)?;
    let branch = repo.head_branch()?;
    require_snapshotted(repo, &branch)?;
    let (tree, stats) = import_dir(repo.store().as_ref(), &args.dir, &args.chunker)?;
    let commit = repo.commit_to_branch(&branch, tree, &author(), &args.message)?;
    let store = repo.store().stats();
    println!("[{branch} {}] {}", commit.short(), args.message);
    println!(
        " {} files, {} dirs, {} symlinks, {}",
        stats.files,
        stats.dirs,
        stats.symlinks,
        human_bytes(stats.bytes)
    );
    println!(
        " {} new objects ({}), {} deduplicated",
        store.new_objects,
        human_bytes(store.new_bytes),
        store.dedup_hits
    );
    if stats.skipped > 0 {
        println!(
            " skipped {} special files (sockets, fifos, devices)",
            stats.skipped
        );
    }
    Ok(())
}

pub fn export(repo: &Repository, args: ExportArgs) -> CmdResult {
    let commit = repo.get_commit(&repo.resolve(&args.rev)?)?;
    let stats = export_tree(repo.store().as_ref(), &commit.tree, &args.dest)?;
    println!(
        "Exported {} files, {} dirs, {} symlinks ({}) to {}",
        stats.files,
        stats.dirs,
        stats.symlinks,
        human_bytes(stats.bytes),
        args.dest.display()
    );
    Ok(())
}
