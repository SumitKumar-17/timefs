//! One module per command group; `run` dispatches.

mod branch;
mod guard;
mod history;
mod init;
mod live;
mod maintenance;
mod mount;
mod switch;
mod transfer;

use std::error::Error;

use timefs_core::Repository;

use crate::cli::{Cli, Command};

pub type CmdResult = Result<(), Box<dyn Error>>;

pub fn run(cli: Cli) -> CmdResult {
    let repo_path = cli.repo;
    if let Command::Init(args) = cli.command {
        return init::run(&repo_path, args);
    }
    let repo = Repository::open(&repo_path)?;
    match cli.command {
        Command::Init(_) => unreachable!("handled above"),
        Command::Import(args) => transfer::import(&repo, args),
        Command::Export(args) => transfer::export(&repo, args),
        Command::Log(args) => history::log(&repo, args),
        Command::Ls(args) => history::ls(&repo, args),
        Command::Cat(args) => history::cat(&repo, args),
        Command::Diff(args) => history::diff(&repo, args),
        Command::Branch(args) => branch::run(&repo, args),
        Command::Mount(args) => mount::run(repo, args),
        Command::Snap(args) => live::snap(&repo, args),
        Command::Checkpoint => live::checkpoint(&repo),
        Command::Status => live::status(&repo),
        Command::Merge(args) => switch::merge(&repo, args),
        Command::Checkout(args) => switch::checkout(&repo, args),
        Command::Reset(args) => switch::reset(&repo, args),
        Command::Gc(args) => maintenance::gc(&repo, args),
        Command::Migrate(args) => maintenance::migrate(&repo, args),
        Command::Fsck => maintenance::fsck(&repo),
        Command::Stats => maintenance::stats(&repo),
    }
}

/// The commit author recorded for CLI-created commits.
pub fn author() -> String {
    std::env::var("USER").unwrap_or_else(|_| "timefs".to_string())
}
