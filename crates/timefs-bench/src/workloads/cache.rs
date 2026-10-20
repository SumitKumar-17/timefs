//! Dropping the page cache for one file, so reads hit the filesystem again.

use std::fs::File;
use std::os::fd::AsRawFd;

/// Ask the kernel to discard cached pages of `file` (best effort; the result is ignored).
pub fn drop_cache(file: &File) {
    // SAFETY: `posix_fadvise` only reads its integer arguments and the descriptor is valid
    // for the duration of the call because `file` is borrowed.
    unsafe {
        libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED);
    }
}
