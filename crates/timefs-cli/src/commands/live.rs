//! Commands that talk to a running mount over its control socket.

use timefs_core::Repository;
use timefs_fuse::control::request;

use super::guard::exclusive;
use super::{author, CmdResult};
use crate::cli::SnapArgs;

/// Snapshot through the running mount, or, when nothing is mounted, promote the saved
/// checkpoint of the current branch.
pub fn snap(repo: &Repository, args: SnapArgs) -> CmdResult {
    let message = args.message.replace('\n', " ");
    match request(&repo.control_socket_path(), &format!("snap {message}")) {
        Ok(reply) => report(&reply),
        Err(e) if is_not_running(&e) => snap_offline(repo, &message),
        Err(e) => Err(e.into()),
    }
}

fn is_not_running(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
    )
}

fn snap_offline(repo: &Repository, message: &str) -> CmdResult {
    let _lock = exclusive(repo)?;
    let branch = repo.head_branch()?;
    match repo.promote_checkpoint(&branch, &author(), message)? {
        Some(commit) => println!("created {commit}"),
        None => println!("nothing to snapshot"),
    }
    Ok(())
}

pub fn checkpoint(repo: &Repository) -> CmdResult {
    send(repo, "checkpoint")
}

pub fn status(repo: &Repository) -> CmdResult {
    send(repo, "status")
}

fn send(repo: &Repository, line: &str) -> CmdResult {
    let reply = request(&repo.control_socket_path(), line).map_err(|e| {
        format!("no running mount found for this repository ({e}); start one with `timefs mount`")
    })?;
    report(&reply)
}

/// Print a successful reply, or turn an `err ...` reply into an error.
fn report(reply: &str) -> CmdResult {
    match reply.strip_prefix("ok") {
        Some(rest) => {
            print!("{}", rest.trim_start());
            Ok(())
        }
        None => Err(reply
            .trim_end()
            .trim_start_matches("err ")
            .to_string()
            .into()),
    }
}
