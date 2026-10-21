//! Segment files: `seg-00000001.log`, `seg-00000002.log`, ... in ascending write order.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

/// Start a new segment once the active one would grow beyond this.
pub const DEFAULT_SEGMENT_LIMIT: u64 = 256 * 1024 * 1024;

pub fn path(dir: &Path, id: u32) -> PathBuf {
    dir.join(format!("seg-{id:08}.log"))
}

fn parse_name(name: &str) -> Option<u32> {
    name.strip_prefix("seg-")?
        .strip_suffix(".log")?
        .parse()
        .ok()
}

/// Ids of all segments in `dir`, ascending.
pub fn list(dir: &Path) -> io::Result<Vec<u32>> {
    let mut ids = Vec::new();
    for entry in fs::read_dir(dir)? {
        if let Some(id) = entry?.file_name().to_str().and_then(parse_name) {
            ids.push(id);
        }
    }
    ids.sort_unstable();
    Ok(ids)
}

pub fn open(dir: &Path, id: u32) -> io::Result<File> {
    File::options().read(true).write(true).open(path(dir, id))
}

pub fn create(dir: &Path, id: u32) -> io::Result<File> {
    File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path(dir, id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_sort() {
        let d = tempfile::tempdir().unwrap();
        for id in [10, 2, 1] {
            create(d.path(), id).unwrap();
        }
        fs::write(d.path().join("WRITER"), "").unwrap();
        fs::write(d.path().join("seg-x.log"), "").unwrap();
        assert_eq!(list(d.path()).unwrap(), vec![1, 2, 10]);
        assert_eq!(parse_name("seg-00000007.log"), Some(7));
        assert!(
            create(d.path(), 1).is_err(),
            "segments are never overwritten"
        );
    }
}
