//! TimeFS benchmark harness. Prints Markdown tables; numbers in docs come from these runs.

mod env;
mod gen;
mod report;
mod scenarios;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use env::Config;

#[derive(Parser)]
#[command(name = "timefs-bench", about = "Benchmarks for TimeFS")]
struct Cli {
    /// Scratch directory (use the disk under test, not tmpfs).
    #[arg(long, default_value = "bench-out")]
    work: PathBuf,
    /// Repetitions per measurement.
    #[arg(long, default_value_t = 5)]
    repeat: usize,
    /// Comma separated file counts for the scaling scenarios.
    #[arg(long, value_delimiter = ',', default_values_t = [1_000usize, 10_000, 100_000])]
    sizes: Vec<usize>,
    /// Mean size of generated files in bytes.
    #[arg(long, default_value_t = 4096)]
    file_size: usize,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run core-level scenarios (no FUSE): snapshot, branch, merge, amplification, store, dedupe.
    Core {
        /// Scenario names; empty runs all.
        names: Vec<String>,
    },
}

fn main() {
    let cli = Cli::parse();
    let cfg = Config {
        work: cli.work,
        repeat: cli.repeat.max(1),
        sizes: cli.sizes,
        file_size: cli.file_size,
    };
    if let Err(e) = std::fs::create_dir_all(&cfg.work) {
        eprintln!("timefs-bench: cannot create {}: {e}", cfg.work.display());
        std::process::exit(1);
    }
    let result = match cli.command {
        Command::Core { names } => run_core(&cfg, names),
    };
    if let Err(e) = result {
        eprintln!("timefs-bench: error: {e}");
        std::process::exit(1);
    }
}

fn run_core(cfg: &Config, names: Vec<String>) -> scenarios::Res {
    let names: Vec<String> = if names.is_empty() {
        scenarios::NAMES.iter().map(|s| s.to_string()).collect()
    } else {
        names
    };
    for name in names {
        eprintln!("running scenario: {name}");
        scenarios::run(&name, cfg)?;
    }
    Ok(())
}
