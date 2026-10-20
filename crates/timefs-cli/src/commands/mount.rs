//! `timefs mount`: run a mount in the foreground until it is unmounted.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use timefs_core::Repository;
use timefs_fuse::MountConfig;

use super::{author, CmdResult};
use crate::cli::MountArgs;

const POLL: Duration = Duration::from_millis(100);

pub fn run(repo: Repository, args: MountArgs) -> CmdResult {
    let interval =
        (args.checkpoint_interval > 0).then(|| Duration::from_secs(args.checkpoint_interval));
    let cfg = MountConfig {
        branch: args.branch,
        checkpoint_interval: interval,
        flush_threshold: args.flush_threshold_mib * 1024 * 1024,
        attr_ttl: Duration::from_millis(args.attr_ttl_ms),
        block_size: args.block_size as usize,
        author: author(),
        allow_other: args.allow_other,
        threads: args.threads,
        ..MountConfig::default()
    };

    let term = Arc::new(AtomicBool::new(false));
    for sig in [SIGINT, SIGTERM, SIGHUP] {
        signal_hook::flag::register(sig, Arc::clone(&term))?;
    }

    let mounted = timefs_fuse::mount(repo, &args.mountpoint, &cfg)?;
    if mounted.recovery.restored_checkpoint {
        println!("Recovered unsaved changes from the last checkpoint");
    }
    if mounted.recovery.ignored_stale_checkpoint {
        println!("Ignored a stale checkpoint (the branch moved since it was written)");
    }
    println!(
        "Mounted at {} (Ctrl-C or `fusermount3 -u` to unmount)",
        args.mountpoint.display()
    );

    while !term.load(Ordering::SeqCst) && !mounted.is_finished() {
        sleep(POLL);
    }
    // Report success only when the final checkpoint really persisted the changes; an
    // error here exits non-zero with the reason.
    if mounted.is_finished() {
        mounted.finish()?;
    } else {
        mounted.unmount()?;
    }
    println!("Unmounted, changes checkpointed");
    Ok(())
}
