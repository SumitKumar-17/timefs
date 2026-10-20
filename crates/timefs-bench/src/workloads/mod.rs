//! Mount-level workloads: plain `std::fs` operations run against any directory, so the
//! same code measures ext4, tmpfs, a FUSE passthrough and a TimeFS mount.

mod bigfile;
mod cache;
mod smallfiles;
mod sync;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::report::{Summary, Table};

/// One measured quantity: a name that includes its unit, and the value.
pub type Metric = (String, f64);

#[derive(Clone, Debug)]
pub struct Target {
    pub label: String,
    pub dir: PathBuf,
}

/// Parse `label=dir`.
pub fn parse_target(s: &str) -> Result<Target, String> {
    let (label, dir) = s.split_once('=').ok_or("expected label=directory")?;
    if label.is_empty() || dir.is_empty() {
        return Err("expected label=directory".into());
    }
    Ok(Target {
        label: label.to_string(),
        dir: PathBuf::from(dir),
    })
}

#[derive(Clone, Copy, Debug)]
pub struct Sizes {
    pub big_mib: usize,
    pub small_files: usize,
}

/// Run every workload `repeat` times on every target and print one comparison table.
pub fn compare(targets: &[Target], repeat: usize, sizes: Sizes) -> io::Result<()> {
    let mut names: Vec<String> = Vec::new();
    let mut samples: Vec<Vec<Vec<f64>>> = Vec::new(); // target -> metric -> samples
    for target in targets {
        let mut per_metric: Vec<Vec<f64>> = Vec::new();
        for rep in 0..repeat {
            eprintln!("workloads: {} (run {}/{repeat})", target.label, rep + 1);
            let metrics = run_once(&target.dir, sizes)?;
            for (i, (name, value)) in metrics.into_iter().enumerate() {
                if names.len() <= i {
                    names.push(name);
                }
                if per_metric.len() <= i {
                    per_metric.push(Vec::new());
                }
                per_metric[i].push(value);
            }
        }
        samples.push(per_metric);
    }

    let mut headers = vec!["workload"];
    headers.extend(targets.iter().map(|t| t.label.as_str()));
    let mut table = Table::new(&headers);
    for (i, name) in names.iter().enumerate() {
        let mut row = vec![name.clone()];
        row.extend(
            samples
                .iter()
                .map(|per_metric| Summary::of(&per_metric[i]).show(1)),
        );
        table.row(row);
    }
    table.print("Mount workloads (median, min..max)");
    Ok(())
}

/// One pass of all workloads inside a fresh directory, removed afterwards.
fn run_once(dir: &Path, sizes: Sizes) -> io::Result<Vec<Metric>> {
    let work = dir.join(format!("timefs-bench-{}", std::process::id()));
    if work.exists() {
        fs::remove_dir_all(&work)?;
    }
    fs::create_dir_all(&work)?;
    let result = (|| {
        let mut metrics = bigfile::run(&work, sizes.big_mib)?;
        metrics.extend(smallfiles::run(&work, sizes.small_files)?);
        metrics.extend(sync::run(&work)?);
        Ok(metrics)
    })();
    let _ = fs::remove_dir_all(&work);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_targets() {
        let t = parse_target("ext4=/data/x").unwrap();
        assert_eq!(
            (t.label.as_str(), t.dir.to_str().unwrap()),
            ("ext4", "/data/x")
        );
        assert!(parse_target("nodir").is_err());
        assert!(parse_target("=x").is_err());
    }

    #[test]
    fn workloads_run_on_a_plain_directory() {
        let dir = std::env::temp_dir().join(format!("tfs-wl-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let metrics = run_once(
            &dir,
            Sizes {
                big_mib: 2,
                small_files: 20,
            },
        )
        .unwrap();
        assert!(metrics.len() >= 10);
        assert!(
            metrics.iter().all(|(_, v)| v.is_finite() && *v > 0.0),
            "{metrics:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
