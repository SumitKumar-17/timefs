//! fsync latency: what an application waits for when it demands durability.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;
use std::time::Instant;

use super::Metric;
use crate::report::Summary;

const ROUNDS: usize = 50;

pub fn run(dir: &Path) -> io::Result<Vec<Metric>> {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("journal"))?;
    let block = [0xC3u8; 4096];
    let mut latencies = Vec::with_capacity(ROUNDS);
    for _ in 0..ROUNDS {
        f.write_all(&block)?;
        let start = Instant::now();
        f.sync_all()?;
        latencies.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    Ok(vec![(
        "fsync latency ms (append 4 KiB)".into(),
        Summary::of(&latencies).median,
    )])
}
