//! Filesystem statistics reported to `statfs(2)`: the host filesystem that holds the repository.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

pub const NAME_MAX: u32 = 255;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatFs {
    pub blocks: u64,
    pub bfree: u64,
    pub bavail: u64,
    pub files: u64,
    pub ffree: u64,
    pub bsize: u32,
    pub frsize: u32,
}

impl Default for StatFs {
    /// Used when the host cannot be queried: a large, mostly empty volume.
    fn default() -> Self {
        StatFs {
            blocks: 1 << 30,
            bfree: 1 << 30,
            bavail: 1 << 30,
            files: 1 << 30,
            ffree: 1 << 30,
            bsize: 4096,
            frsize: 4096,
        }
    }
}

#[allow(clippy::unnecessary_cast)] // statvfs field widths differ across platforms
pub fn host_stats(path: &Path) -> StatFs {
    let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else {
        return StatFs::default();
    };
    // SAFETY: `statvfs` is plain old data for which all-zero is a valid value, and the
    // call only writes into `st` through a valid pointer. `c_path` is NUL terminated and
    // outlives the call.
    let (rc, st) = unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        (libc::statvfs(c_path.as_ptr(), &mut st), st)
    };
    if rc != 0 {
        return StatFs::default();
    }
    StatFs {
        blocks: st.f_blocks as u64,
        bfree: st.f_bfree as u64,
        bavail: st.f_bavail as u64,
        files: st.f_files as u64,
        ffree: st.f_ffree as u64,
        bsize: st.f_bsize as u32,
        frsize: st.f_frsize as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_real_numbers_for_an_existing_path() {
        let s = host_stats(Path::new("/"));
        assert!(s.blocks > 0 && s.bsize > 0);
    }

    #[test]
    fn falls_back_for_a_missing_path() {
        assert_eq!(
            host_stats(Path::new("/definitely/not/here")),
            StatFs::default()
        );
    }
}
