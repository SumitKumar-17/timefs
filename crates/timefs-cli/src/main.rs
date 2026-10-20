//! The `timefs` command-line tool.

mod chunker_arg;
mod cli;
mod commands;
mod format;

use clap::Parser;

fn main() {
    let cli = cli::Cli::parse();
    if let Err(e) = commands::run(cli) {
        eprintln!("timefs: error: {e}");
        std::process::exit(1);
    }
}
