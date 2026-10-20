//! One module per command group; `run` dispatches.

mod branch;
mod history;
mod init;
mod maintenance;
mod transfer;

use std::error::Error;

use timefs_core::Repository;

use crate::cli::{Cli, Command};

pub type CmdResult = Result<(), Box<dyn Error>>;

pub fn run(cli: Cli) -> CmdResult {
    let repo_path = cli.repo;
    if let Command::Init = cli.command {
        return init::run(&repo_path);
    }
    let repo = Repository::open(&repo_path)?;
    match cli.command {
        Command::Init => unreachable!("handled above"),
        Command::Import(args) => transfer::import(&repo, args),
        Command::Export(args) => transfer::export(&repo, args),
        Command::Log(args) => history::log(&repo, args),
        Command::Ls(args) => history::ls(&repo, args),
        Command::Cat(args) => history::cat(&repo, args),
        Command::Diff(args) => history::diff(&repo, args),
        Command::Branch(args) => branch::run(&repo, args),
        Command::Fsck => maintenance::fsck(&repo),
        Command::Stats => maintenance::stats(&repo),
    }
}

/// The commit author recorded for CLI-created commits.
pub fn author() -> String {
    std::env::var("USER").unwrap_or_else(|_| "timefs".to_string())
}
