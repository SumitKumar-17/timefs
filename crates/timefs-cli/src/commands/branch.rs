use timefs_core::{Error, Hash, Repository};
use timefs_fuse::control::request;

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
            match repo.create_branch(&name, &start) {
                Ok(()) => {
                    println!("Created branch {name} at {}", start.short());
                    Ok(())
                }
                // A pack repository has one writer: while it is mounted, the mount must
                // write the ref, so ask it over the control socket.
                Err(Error::Locked) => create_through_mount(repo, &name, &start),
                Err(e) => Err(e.into()),
            }
        }
    }
}

fn create_through_mount(repo: &Repository, name: &str, start: &Hash) -> CmdResult {
    let reply = request(
        &repo.control_socket_path(),
        &format!("branch {name} {start}"),
    )
    .map_err(|e| format!("the repository is locked and no mount answered ({e})"))?;
    match reply.strip_prefix("ok ") {
        Some(rest) => {
            print!("{}", capitalise(rest));
            Ok(())
        }
        None => Err(reply
            .trim_end()
            .trim_start_matches("err ")
            .to_string()
            .into()),
    }
}

fn capitalise(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
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
