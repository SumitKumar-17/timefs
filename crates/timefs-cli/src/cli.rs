//! Command-line definitions (argument parsing only, no behaviour).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use timefs_core::chunk::Chunker;

use crate::chunker_arg::{parse_chunker, parse_size_arg};

#[derive(Parser)]
#[command(
    name = "timefs",
    version,
    about = "A filesystem with built-in time travel"
)]
pub struct Cli {
    /// Repository directory.
    #[arg(
        short,
        long,
        global = true,
        env = "TIMEFS_REPO",
        default_value = ".timefs-repo"
    )]
    pub repo: PathBuf,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Create a new, empty repository.
    Init,
    /// Snapshot a host directory as a new commit on the current branch.
    Import(ImportArgs),
    /// Write the tree of a revision into an empty directory.
    Export(ExportArgs),
    /// Show commit history.
    Log(LogArgs),
    /// List a directory inside a revision.
    Ls(PathArgs),
    /// Print a file inside a revision.
    Cat(PathArgs),
    /// Show what changed between two revisions.
    Diff(DiffArgs),
    /// List, create or delete branches.
    Branch(BranchArgs),
    /// Mount a branch as a filesystem (runs until unmounted).
    Mount(MountArgs),
    /// Snapshot the working tree of a running mount onto its branch.
    Snap(SnapArgs),
    /// Persist the working tree of a running mount without committing to the branch.
    Checkpoint,
    /// Show the state of a running mount.
    Status,
    /// Verify the integrity of every object and ref.
    Fsck,
    /// Show repository size and deduplication statistics.
    Stats,
}

#[derive(Args)]
pub struct ImportArgs {
    /// Directory to snapshot.
    pub dir: PathBuf,
    /// Commit message.
    #[arg(short, long, default_value = "import")]
    pub message: String,
    /// Chunking: `fixed[:SIZE]` or `cdc[:AVG]` (sizes accept K/M suffixes).
    #[arg(long, default_value = "fixed:64K", value_parser = parse_chunker)]
    pub chunker: Chunker,
}

#[derive(Args)]
pub struct ExportArgs {
    pub rev: String,
    pub dest: PathBuf,
}

#[derive(Args)]
pub struct LogArgs {
    #[arg(default_value = "HEAD")]
    pub rev: String,
    /// Maximum number of commits to show.
    #[arg(short = 'n', long, default_value_t = 20)]
    pub limit: usize,
}

#[derive(Args)]
pub struct PathArgs {
    pub rev: String,
    #[arg(default_value = "")]
    pub path: String,
}

#[derive(Args)]
pub struct DiffArgs {
    pub from: String,
    pub to: String,
}

#[derive(Args)]
pub struct BranchArgs {
    /// Branch to create (omit to list branches).
    pub name: Option<String>,
    /// Start point for the new branch.
    #[arg(default_value = "HEAD")]
    pub start: String,
    /// Delete the named branch instead of creating it.
    #[arg(short, long)]
    pub delete: bool,
}

#[derive(Args)]
pub struct MountArgs {
    /// Directory to mount on (must exist and be empty).
    pub mountpoint: PathBuf,
    /// Branch to mount (default: the repository's HEAD branch).
    #[arg(short, long)]
    pub branch: Option<String>,
    /// Seconds between automatic checkpoints (0 disables the timer).
    #[arg(long, default_value_t = 5)]
    pub checkpoint_interval: u64,
    /// Checkpoint inline once this many MiB of unpersisted data accumulate.
    #[arg(long, default_value_t = 256)]
    pub flush_threshold_mib: u64,
    /// How long the kernel may cache attributes, in milliseconds.
    #[arg(long, default_value_t = 1000)]
    pub attr_ttl_ms: u64,
    /// Block size for newly written data (K and M suffixes accepted).
    #[arg(long, default_value = "64K", value_parser = parse_size_arg)]
    pub block_size: u32,
    /// Number of FUSE worker threads.
    #[arg(long, default_value_t = 1)]
    pub threads: usize,
    /// Allow other users to access the mount (needs user_allow_other in /etc/fuse.conf).
    #[arg(long)]
    pub allow_other: bool,
}

#[derive(Args)]
pub struct SnapArgs {
    /// Commit message.
    #[arg(short, long, default_value = "snapshot")]
    pub message: String,
}
