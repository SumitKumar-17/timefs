//! Many small files: the metadata-heavy side of a filesystem.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::Metric;

const FILE_SIZE: usize = 4096;

fn per_second(count: usize, start: Instant) -> f64 {
    count as f64 / start.elapsed().as_secs_f64()
}

pub fn run(dir: &Path, count: usize) -> io::Result<Vec<Metric>> {
    let root = dir.join("small");
    fs::create_dir(&root)?;
    let paths: Vec<PathBuf> = (0..count).map(|i| root.join(format!("f{i:06}"))).collect();
    let payload = vec![0x5Au8; FILE_SIZE];
    let mut metrics = Vec::new();

    let start = Instant::now();
    for p in &paths {
        File::create(p)?.write_all(&payload)?;
    }
    metrics.push(("create 4 KiB files /s".into(), per_second(count, start)));

    let start = Instant::now();
    for p in &paths {
        fs::metadata(p)?;
    }
    metrics.push(("stat /s".into(), per_second(count, start)));

    let start = Instant::now();
    let mut buf = Vec::with_capacity(FILE_SIZE);
    for p in &paths {
        buf.clear();
        File::open(p)?.read_to_end(&mut buf)?;
    }
    metrics.push(("open+read+close /s".into(), per_second(count, start)));

    let start = Instant::now();
    let listed = fs::read_dir(&root)?.count();
    assert_eq!(listed, count, "readdir lost entries");
    metrics.push(("readdir entries /s".into(), per_second(count, start)));

    let start = Instant::now();
    let renamed: Vec<PathBuf> = paths.iter().map(|p| p.with_extension("r")).collect();
    for (from, to) in paths.iter().zip(&renamed) {
        fs::rename(from, to)?;
    }
    metrics.push(("rename /s".into(), per_second(count, start)));

    let start = Instant::now();
    for p in &renamed {
        fs::remove_file(p)?;
    }
    metrics.push(("unlink /s".into(), per_second(count, start)));
    Ok(metrics)
}
