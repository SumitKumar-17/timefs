//! Commands that talk to a running mount over its control socket.

use timefs_core::Repository;
use timefs_fuse::control::request;

use super::CmdResult;
use crate::cli::SnapArgs;

pub fn snap(repo: &Repository, args: SnapArgs) -> CmdResult {
    let message = args.message.replace('\n', " ");
    send(repo, &format!("snap {message}"))
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
