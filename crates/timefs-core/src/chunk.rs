//! Splitting file data into blocks.

use std::io::Read;

use crate::error::{Error, Result};

pub const DEFAULT_BLOCK_SIZE: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chunker {
    /// Fixed-size blocks (the last one may be short).
    Fixed { size: usize },
    /// FastCDC content-defined chunking.
    Cdc { min: u32, avg: u32, max: u32 },
}

impl Default for Chunker {
    fn default() -> Self {
        Chunker::Fixed {
            size: DEFAULT_BLOCK_SIZE,
        }
    }
}

impl Chunker {
    /// CDC with the given average size (min = avg/4, max = avg*4).
    pub fn cdc(avg: u32) -> Self {
        Chunker::Cdc {
            min: (avg / 4).max(64),
            avg: avg.max(256),
            max: (avg * 4).max(1024),
        }
    }

    pub fn validate(&self) -> Result<()> {
        match *self {
            Chunker::Fixed { size: 0 } => Err(Error::Invalid("block size must be non-zero".into())),
            Chunker::Cdc { min, avg, max } if !(64 <= min && min <= avg && avg <= max) => Err(
                Error::Invalid("cdc sizes must satisfy 64 <= min <= avg <= max".into()),
            ),
            Chunker::Cdc { avg, .. } if avg < 256 => {
                Err(Error::Invalid("cdc average must be >= 256".into()))
            }
            Chunker::Cdc { max, .. } if max < 1024 => {
                Err(Error::Invalid("cdc max must be >= 1024".into()))
            }
            _ => Ok(()),
        }
    }

    /// Split an in-memory buffer into `(offset, len)` ranges that exactly tile it.
    pub fn split(&self, data: &[u8]) -> Vec<(usize, usize)> {
        match *self {
            Chunker::Fixed { size } => {
                let mut out = Vec::with_capacity(data.len() / size + 1);
                let mut off = 0;
                while off < data.len() {
                    let len = size.min(data.len() - off);
                    out.push((off, len));
                    off += len;
                }
                out
            }
            Chunker::Cdc { min, avg, max } => {
                if data.is_empty() {
                    return Vec::new();
                }
                fastcdc::v2020::FastCDC::new(data, min, avg, max)
                    .map(|c| (c.offset, c.length))
                    .collect()
            }
        }
    }

    /// Stream chunks from a reader, calling `f` once per chunk in order.
    pub fn for_each_chunk<R: Read>(
        &self,
        mut reader: R,
        mut f: impl FnMut(&[u8]) -> Result<()>,
    ) -> Result<()> {
        match *self {
            Chunker::Fixed { size } => {
                let mut buf = vec![0u8; size];
                loop {
                    let n = read_full(&mut reader, &mut buf)?;
                    if n == 0 {
                        return Ok(());
                    }
                    f(&buf[..n])?;
                    if n < size {
                        return Ok(());
                    }
                }
            }
            Chunker::Cdc { min, avg, max } => {
                for chunk in fastcdc::v2020::StreamCDC::new(reader, min, avg, max) {
                    match chunk {
                        Ok(c) => f(&c.data)?,
                        Err(fastcdc::v2020::Error::Empty) => break,
                        Err(fastcdc::v2020::Error::IoError(e)) => return Err(Error::Io(e)),
                        Err(e) => return Err(Error::Invalid(e.to_string())),
                    }
                }
                Ok(())
            }
        }
    }
}

/// Read until `buf` is full or EOF; returns the number of bytes read.
fn read_full<R: Read>(r: &mut R, buf: &mut [u8]) -> Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(Error::Io(e)),
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn tiles(ranges: &[(usize, usize)], total: usize) -> bool {
        let mut off = 0;
        for &(o, l) in ranges {
            if o != off || l == 0 {
                return false;
            }
            off += l;
        }
        off == total
    }

    #[test]
    fn fixed_splits_with_short_tail() {
        let c = Chunker::Fixed { size: 4 };
        assert_eq!(c.split(&[0u8; 10]), vec![(0, 4), (4, 4), (8, 2)]);
        assert!(c.split(&[]).is_empty());
    }

    #[test]
    fn validate_rejects_bad_params() {
        assert!(Chunker::Fixed { size: 0 }.validate().is_err());
        assert!(Chunker::Cdc {
            min: 10,
            avg: 20,
            max: 30
        }
        .validate()
        .is_err());
        assert!(Chunker::cdc(8192).validate().is_ok());
    }

    #[test]
    fn cdc_resynchronises_after_insertion() {
        // pseudo-random data
        let mut x = 0x1234_5678_9abc_def0u64;
        let data: Vec<u8> = (0..400_000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect();
        let c = Chunker::cdc(4096);
        let a: std::collections::HashSet<&[u8]> = c
            .split(&data)
            .iter()
            .map(|&(o, l)| &data[o..o + l])
            .collect();
        let mut edited = data.clone();
        edited.splice(1000..1000, [0xAAu8; 17]);
        let b: Vec<&[u8]> = c
            .split(&edited)
            .iter()
            .map(|&(o, l)| &edited[o..o + l])
            .collect();
        let shared = b.iter().filter(|chunk| a.contains(*chunk)).count();
        // an insertion must leave the large majority of chunks unchanged
        assert!(shared * 10 >= b.len() * 8, "shared {shared} of {}", b.len());
    }

    proptest! {
        #[test]
        fn chunkers_tile_input(data in proptest::collection::vec(any::<u8>(), 0..20_000), size in 1usize..5000) {
            for c in [Chunker::Fixed { size }, Chunker::cdc(1024)] {
                prop_assert!(tiles(&c.split(&data), data.len()));
            }
        }

        #[test]
        fn streaming_matches_in_memory(data in proptest::collection::vec(any::<u8>(), 0..20_000), size in 1usize..5000) {
            for c in [Chunker::Fixed { size }, Chunker::cdc(1024)] {
                let mut streamed: Vec<Vec<u8>> = Vec::new();
                c.for_each_chunk(&data[..], |b| { streamed.push(b.to_vec()); Ok(()) }).unwrap();
                let direct: Vec<Vec<u8>> = c.split(&data).iter().map(|&(o, l)| data[o..o + l].to_vec()).collect();
                prop_assert_eq!(streamed, direct);
            }
        }
    }
}
