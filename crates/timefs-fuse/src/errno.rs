//! Translation of working tree errors to kernel errno values.

use fuser::Errno;
use timefs_core::worktree::FsError;
use timefs_core::Error;

pub fn errno_of(e: &FsError) -> Errno {
    match e {
        FsError::NotFound => Errno::ENOENT,
        FsError::Exists => Errno::EEXIST,
        FsError::NotDir => Errno::ENOTDIR,
        FsError::IsDir => Errno::EISDIR,
        FsError::NotEmpty => Errno::ENOTEMPTY,
        FsError::NameTooLong => Errno::ENAMETOOLONG,
        FsError::InvalidArgument(_) => Errno::EINVAL,
        FsError::Unsupported(_) => Errno::ENOTSUP,
        FsError::Store(inner) => store_errno(inner),
    }
}

/// Storage failures surface as `EIO`, except genuine OS errors such as `ENOSPC`.
pub fn store_errno(e: &Error) -> Errno {
    match e {
        Error::Io(io) => io.raw_os_error().map_or(Errno::EIO, Errno::from_i32),
        _ => Errno::EIO,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_each_variant() {
        assert_eq!(errno_of(&FsError::NotFound), Errno::ENOENT);
        assert_eq!(errno_of(&FsError::NotEmpty), Errno::ENOTEMPTY);
        assert_eq!(errno_of(&FsError::InvalidArgument("x")), Errno::EINVAL);
        let corrupt = FsError::Store(Error::corrupt("bad"));
        assert_eq!(errno_of(&corrupt), Errno::EIO);
        let full = FsError::Store(Error::Io(std::io::Error::from_raw_os_error(libc::ENOSPC)));
        assert_eq!(errno_of(&full), Errno::ENOSPC);
    }
}
