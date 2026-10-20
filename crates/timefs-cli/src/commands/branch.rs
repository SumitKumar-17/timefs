use timefs_core::Repository;

use super::guard::exclusive;
use super::CmdResult;
use crate::cli::BranchArgs;

pub fn run(repo: &Repository, args: BranchArgs) -> CmdResult {
    match (args.name, args.delete) {
        (None, _) => list(repo),
        (Some(name), true) => {
            // The branch may be mounted with `--branch`; deleting it under the mount
            // would let the next snapshot silently recreate it.
            let _lock = exclusive(repo)?;
            repo.delete_branch(&name)?;
            println!("Deleted branch {name}");
            Ok(())
        }
        (Some(name), false) => {
            let start = repo.resolve(&args.start)?;
            repo.create_branch(&name, &start)?;
            println!("Created branch {name} at {}", start.short());
            Ok(())
        }
    }
}

fn list(repo: &Repository) -> CmdResult {
    let current = repo.head_branch()?;
    let branches = repo.branches()?;
    if branches.is_empty() {
        println!("* {current} (no commits yet)");
    }
    for (name, tip) in branches {
        let mark = if name == current { '*' } else { ' ' };
        println!("{mark} {name} {}", tip.short());
    }
    Ok(())
}
