//! Parsing of the `--chunker` option.

use timefs_core::chunk::{Chunker, DEFAULT_BLOCK_SIZE};

const MIN_CDC_AVG: u32 = 256;

/// `fixed`, `fixed:64K`, `cdc`, `cdc:16K`. Sizes accept `K` and `M` suffixes.
pub fn parse_chunker(s: &str) -> Result<Chunker, String> {
    let (kind, size) = match s.split_once(':') {
        Some((k, v)) => (k, Some(parse_size(v)?)),
        None => (s, None),
    };
    let chunker = match kind {
        "fixed" => Chunker::Fixed {
            size: size.map_or(DEFAULT_BLOCK_SIZE, |v| v as usize),
        },
        "cdc" => {
            let avg = size.unwrap_or(16 * 1024);
            if avg < MIN_CDC_AVG {
                return Err(format!("cdc average must be at least {MIN_CDC_AVG} bytes"));
            }
            Chunker::cdc(avg)
        }
        other => return Err(format!("unknown chunker {other:?} (expected fixed or cdc)")),
    };
    chunker.validate().map_err(|e| e.to_string())?;
    Ok(chunker)
}

fn parse_size(s: &str) -> Result<u32, String> {
    let (digits, factor) = match s.chars().last() {
        Some('K' | 'k') => (&s[..s.len() - 1], 1024),
        Some('M' | 'm') => (&s[..s.len() - 1], 1024 * 1024),
        _ => (s, 1),
    };
    digits
        .parse::<u32>()
        .ok()
        .and_then(|n| n.checked_mul(factor))
        .ok_or_else(|| format!("invalid size {s:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixed_and_cdc() {
        assert_eq!(parse_chunker("fixed").unwrap(), Chunker::default());
        assert_eq!(
            parse_chunker("fixed:4K").unwrap(),
            Chunker::Fixed { size: 4096 }
        );
        assert_eq!(parse_chunker("cdc:8K").unwrap(), Chunker::cdc(8192));
        assert_eq!(
            parse_chunker("fixed:1M").unwrap(),
            Chunker::Fixed { size: 1 << 20 }
        );
    }

    #[test]
    fn rejects_garbage() {
        for bad in [
            "",
            "zip",
            "fixed:",
            "fixed:0",
            "fixed:abc",
            "cdc:10",
            "fixed:99999999M",
        ] {
            assert!(parse_chunker(bad).is_err(), "{bad:?}");
        }
    }
}
