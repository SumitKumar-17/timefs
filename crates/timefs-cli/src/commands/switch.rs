//! Commands that move branches: merge, checkout, reset.

use timefs_core::merge::{merge_commits, Conflict, MergeOutcome, Resolve};
use timefs_core::Repository;

use super::guard::{exclusive, require_snapshotted};
use super::{author, CmdResult};
use crate::cli::{CheckoutArgs, MergeArgs, ResetArgs, ResolveArg};

impl From<ResolveArg> for Resolve {
    fn from(r: ResolveArg) -> Self {
        match r {
            ResolveArg::Ours => Resolve::Ours,
            ResolveArg::Theirs => Resolve::Theirs,
        }
    }
}

pub fn merge(repo: &Repository, args: MergeArgs) -> CmdResult {
    let _lock = exclusive(repo)?;
    let branch = repo.head_branch()?;
    require_snapshotted(repo, &branch)?;
    let ours = repo
        .branch_tip(&branch)?
        .ok_or("the current branch has no commits yet")?;
    let theirs = repo.resolve(&args.rev)?;
    let message = args
        .message
        .unwrap_or_else(|| format!("Merge {} into {branch}", args.rev));
    let resolve = args.resolve.map_or(Resolve::Abort, Resolve::from);

    match merge_commits(repo, &ours, &theirs, resolve, &author(), &message)? {
        MergeOutcome::UpToDate => println!("Already up to date."),
        MergeOutcome::FastForward { tip } => {
            repo.set_branch_tip(&branch, &tip)?;
            println!("Fast-forward {branch} to {}", tip.short());
        }
        MergeOutcome::Merged { commit, resolved } => {
            repo.set_branch_tip(&branch, &commit)?;
            println!("Merged into {branch} as {}", commit.short());
            print_conflicts("resolved", &resolved);
        }
        MergeOutcome::Conflicts(conflicts) => {
            print_conflicts("conflict", &conflicts);
            return Err(format!(
                "merge aborted with {} conflict(s); nothing changed (use --resolve ours|theirs)",
                conflicts.len()
            )
            .into());
        }
    }
    Ok(())
}

pub fn checkout(repo: &Repository, args: CheckoutArgs) -> CmdResult {
    let _lock = exclusive(repo)?;
    require_snapshotted(repo, &repo.head_branch()?)?;
    if repo.branch_tip(&args.branch)?.is_none() {
        return Err(format!("branch {:?} does not exist", args.branch).into());
    }
    repo.set_head_branch(&args.branch)?;
    println!("Switched to branch {}", args.branch);
    Ok(())
}

pub fn reset(repo: &Repository, args: ResetArgs) -> CmdResult {
    let _lock = exclusive(repo)?;
    let branch = repo.head_branch()?;
    require_snapshotted(repo, &branch)?;
    let target = repo.resolve(&args.rev)?;
    repo.get_commit(&target)?;
    repo.set_branch_tip(&branch, &target)?;
    println!("Branch {branch} is now at {}", target.short());
    Ok(())
}

fn print_conflicts(label: &str, conflicts: &[Conflict]) {
    for c in conflicts {
        println!("{label}: {} ({:?})", c.path, c.kind);
    }
}
