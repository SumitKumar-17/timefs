//! A minimal loopback FUSE filesystem over a host directory.
//!
//! It exists only as a baseline: comparing it with a TimeFS mount separates the cost of
//! FUSE itself from the cost of TimeFS's versioning machinery.

mod fs;
mod inodes;

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use fuser::{Config, MountOption};
use signal_hook::consts::{SIGINT, SIGTERM};

use fs::Passthrough;

/// Mount `source` at `mountpoint` and block until interrupted or unmounted.
pub fn run(source: &Path, mountpoint: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let term = Arc::new(AtomicBool::new(false));
    for sig in [SIGINT, SIGTERM] {
        signal_hook::flag::register(sig, Arc::clone(&term))?;
    }
    let mut config = Config::default();
    config.mount_options = vec![
        MountOption::FSName("passthrough".into()),
        MountOption::DefaultPermissions,
    ];
    let session = fuser::spawn_mount(
        Passthrough::new(source.canonicalize()?),
        mountpoint,
        &config,
    )?;
    println!(
        "passthrough of {} mounted at {}",
        source.display(),
        mountpoint.display()
    );
    while !term.load(Ordering::SeqCst) && !session.guard.is_finished() {
        std::thread::sleep(Duration::from_millis(100));
    }
    if !session.guard.is_finished() {
        session.umount_and_join()?;
    }
    Ok(())
}
