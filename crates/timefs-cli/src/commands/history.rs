//! Read-only inspection of history and trees.

use std::io::Write;

use timefs_core::diff::{diff_trees, ChangeKind};
use timefs_core::object::EntryKind;
use timefs_core::store::ObjectStoreExt;
use timefs_core::{treeops, Hash, Repository};

use super::CmdResult;
use crate::cli::{DiffArgs, LogArgs, PathArgs};
use crate::format::utc_timestamp;

fn tree_of(repo: &Repository, rev: &str) -> Result<Hash, Box<dyn std::error::Error>> {
    Ok(repo.get_commit(&repo.resolve(rev)?)?.tree)
}

pub fn log(repo: &Repository, args: LogArgs) -> CmdResult {
    let tip = repo.resolve(&args.rev)?;
    for (hash, commit) in repo.log(&tip, args.limit)? {
        let merge = if commit.parents.len() > 1 {
            " (merge)"
        } else {
            ""
        };
        println!(
            "{} {} {}  {}{merge}",
            hash.short(),
            utc_timestamp(commit.time_ns),
            commit.author,
            commit.message.lines().next().unwrap_or("")
        );
    }
    Ok(())
}

pub fn ls(repo: &Repository, args: PathArgs) -> CmdResult {
    let root = tree_of(repo, &args.rev)?;
    let store = repo.store().as_ref();
    let entry = treeops::lookup_path(store, &root, &args.path)?
        .ok_or_else(|| format!("{:?} not found in {}", args.path, args.rev))?;
    if entry.kind != EntryKind::Dir {
        return Err(format!("{:?} is not a directory", args.path).into());
    }
    for e in store.get_tree(&entry.hash)?.entries() {
        let marker = match e.kind {
            EntryKind::Dir => "/",
            EntryKind::Symlink => "@",
            EntryKind::File => "",
        };
        println!(
            "{:04o} {:>10} {}{marker}",
            e.mode,
            e.size,
            String::from_utf8_lossy(&e.name)
        );
    }
    Ok(())
}

pub fn cat(repo: &Repository, args: PathArgs) -> CmdResult {
    let root = tree_of(repo, &args.rev)?;
    let store = repo.store().as_ref();
    let entry = treeops::lookup_path(store, &root, &args.path)?
        .ok_or_else(|| format!("{:?} not found in {}", args.path, args.rev))?;
    let data = match entry.kind {
        EntryKind::File => treeops::read_file(store, &entry.hash)?,
        EntryKind::Symlink => store.get_blob(&entry.hash)?,
        EntryKind::Dir => return Err(format!("{:?} is a directory", args.path).into()),
    };
    std::io::stdout().write_all(&data)?;
    Ok(())
}

pub fn diff(repo: &Repository, args: DiffArgs) -> CmdResult {
    let (from, to) = (tree_of(repo, &args.from)?, tree_of(repo, &args.to)?);
    for change in diff_trees(repo.store().as_ref(), Some(&from), Some(&to))? {
        let tag = match change.kind {
            ChangeKind::Added => 'A',
            ChangeKind::Deleted => 'D',
            ChangeKind::Modified => 'M',
            ChangeKind::MetaChanged => 'm',
        };
        println!("{tag} {}", change.path);
    }
    Ok(())
}
