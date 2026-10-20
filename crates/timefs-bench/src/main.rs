//! TimeFS benchmark harness. Prints Markdown tables; numbers in docs come from these runs.

mod env;
mod gen;
mod passthrough;
mod report;
mod scenarios;
mod workloads;

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
    /// Run mount-level workloads on each target directory and compare them.
    Workloads {
        /// A `label=directory` pair; repeat for several targets.
        #[arg(long = "target", required = true, value_parser = workloads::parse_target)]
        targets: Vec<workloads::Target>,
        /// Size of the big file in MiB.
        #[arg(long, default_value_t = 128)]
        big_mib: usize,
        /// Number of small files.
        #[arg(long, default_value_t = 3000)]
        small_files: usize,
    },
    /// Mount a loopback passthrough filesystem (the FUSE-only baseline).
    Passthrough {
        source: PathBuf,
        mountpoint: PathBuf,
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
        Command::Workloads {
            targets,
            big_mib,
            small_files,
        } => {
            let sizes = workloads::Sizes {
                big_mib,
                small_files,
            };
            workloads::compare(&targets, cfg.repeat, sizes).map_err(Into::into)
        }
        Command::Passthrough { source, mountpoint } => passthrough::run(&source, &mountpoint),
    };
    if let Err(e) = result {
        eprintln!("timefs-bench: error: {e}");
        std::process::exit(1);
    }
}

fn run_core(cfg: &Config, names: Vec<String>) -> scenarios::Res {
    let names: Vec<String> = if names.is_empty() {
        scenarios::names().map(str::to_string).collect()
    } else {
        names
    };
    for name in names {
        eprintln!("running scenario: {name}");
        scenarios::run(&name, cfg)?;
    }
    Ok(())
}
