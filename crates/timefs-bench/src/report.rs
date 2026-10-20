//! Timing, statistics and Markdown output.

use std::time::{Duration, Instant};

/// Run `f` and return its result with the elapsed time.
pub fn timed<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let start = Instant::now();
    let value = f();
    (value, start.elapsed())
}

/// Summary of repeated measurements (in the unit the caller chose).
#[derive(Clone, Copy, Debug)]
pub struct Summary {
    pub median: f64,
    pub min: f64,
    pub max: f64,
}

impl Summary {
    pub fn of(samples: &[f64]) -> Summary {
        assert!(!samples.is_empty(), "no samples");
        let mut v = samples.to_vec();
        v.sort_by(|a, b| a.partial_cmp(b).expect("no NaN samples"));
        let mid = v.len() / 2;
        let median = if v.len() % 2 == 1 {
            v[mid]
        } else {
            (v[mid - 1] + v[mid]) / 2.0
        };
        Summary {
            median,
            min: v[0],
            max: v[v.len() - 1],
        }
    }

    /// `median (min..max)` with the given number of decimals.
    pub fn show(&self, decimals: usize) -> String {
        format!(
            "{:.d$} ({:.d$}..{:.d$})",
            self.median,
            self.min,
            self.max,
            d = decimals
        )
    }
}

pub fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// A Markdown table printed as rows are added.
pub struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl Table {
    pub fn new(headers: &[&str]) -> Self {
        Table {
            headers: headers.iter().map(|h| h.to_string()).collect(),
            rows: Vec::new(),
        }
    }

    pub fn row(&mut self, cells: Vec<String>) {
        assert_eq!(cells.len(), self.headers.len(), "row width");
        self.rows.push(cells);
    }

    pub fn print(&self, title: &str) {
        println!("\n### {title}\n");
        println!("| {} |", self.headers.join(" | "));
        println!(
            "|{}|",
            self.headers
                .iter()
                .map(|_| "---")
                .collect::<Vec<_>>()
                .join("|")
        );
        for r in &self.rows {
            println!("| {} |", r.join(" | "));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_median_and_range() {
        let s = Summary::of(&[3.0, 1.0, 2.0]);
        assert_eq!((s.median, s.min, s.max), (2.0, 1.0, 3.0));
        assert_eq!(Summary::of(&[1.0, 2.0, 3.0, 4.0]).median, 2.5);
        assert_eq!(Summary::of(&[5.0]).show(1), "5.0 (5.0..5.0)");
    }

    #[test]
    fn formats_bytes() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
    }
}
