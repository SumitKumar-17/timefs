//! Throughput on one large file: sequential and random, warm and cold.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::time::Instant;

use super::cache::drop_cache;
use super::Metric;
use crate::gen::Rng;

const CHUNK: usize = 1 << 20;
const IO_SIZE: usize = 4096;
const RANDOM_READS: usize = 10_000;
const RANDOM_WRITES: usize = 2_000;

fn mb_per_s(bytes: usize, secs: f64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0) / secs
}

pub fn run(dir: &Path, mib: usize) -> io::Result<Vec<Metric>> {
    let path = dir.join("big.bin");
    let total = mib * CHUNK;
    let mut metrics = Vec::new();

    let data = Rng::new(3).bytes(CHUNK);
    let start = Instant::now();
    {
        let mut f = File::create(&path)?;
        for _ in 0..mib {
            f.write_all(&data)?;
        }
        f.sync_all()?;
    }
    metrics.push((
        "seq write MB/s (incl fsync)".into(),
        mb_per_s(total, start.elapsed().as_secs_f64()),
    ));

    metrics.push(("seq read warm MB/s".into(), read_all(&path, false)?));
    metrics.push(("seq read cold MB/s".into(), read_all(&path, true)?));
    metrics.push((
        "rand read 4K IOPS (cold)".into(),
        random_read(&path, total)?,
    ));
    metrics.push((
        "rand write 4K IOPS (incl fsync)".into(),
        random_write(&path, total)?,
    ));
    Ok(metrics)
}

fn read_all(path: &Path, cold: bool) -> io::Result<f64> {
    let mut f = File::open(path)?;
    if cold {
        drop_cache(&f);
    }
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0;
    let start = Instant::now();
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n;
    }
    Ok(mb_per_s(total, start.elapsed().as_secs_f64()))
}

fn random_read(path: &Path, size: usize) -> io::Result<f64> {
    let f = File::open(path)?;
    drop_cache(&f);
    let mut rng = Rng::new(21);
    let mut buf = [0u8; IO_SIZE];
    let start = Instant::now();
    for _ in 0..RANDOM_READS {
        let off = rng.below((size / IO_SIZE) as u64) * IO_SIZE as u64;
        f.read_exact_at(&mut buf, off)?;
    }
    Ok(RANDOM_READS as f64 / start.elapsed().as_secs_f64())
}

fn random_write(path: &Path, size: usize) -> io::Result<f64> {
    let f = OpenOptions::new().write(true).open(path)?;
    let mut rng = Rng::new(22);
    let block = rng.bytes(IO_SIZE);
    let start = Instant::now();
    for _ in 0..RANDOM_WRITES {
        let off = rng.below((size / IO_SIZE) as u64) * IO_SIZE as u64;
        f.write_all_at(&block, off)?;
    }
    f.sync_all()?;
    Ok(RANDOM_WRITES as f64 / start.elapsed().as_secs_f64())
}
