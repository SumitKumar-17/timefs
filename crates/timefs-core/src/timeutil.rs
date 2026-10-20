//! Wall clock helpers (nanoseconds since the Unix epoch, the unit used in objects).

use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ns() -> i64 {
    system_time_to_ns(SystemTime::now())
}

pub fn system_time_to_ns(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_nanos()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_nanos()).unwrap_or(i64::MAX),
    }
}

pub fn ns_to_system_time(ns: i64) -> SystemTime {
    if ns >= 0 {
        UNIX_EPOCH + std::time::Duration::from_nanos(ns as u64)
    } else {
        UNIX_EPOCH - std::time::Duration::from_nanos(ns.unsigned_abs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_around_the_epoch() {
        for ns in [-1_500_000_000i64, 0, 1, 1_700_000_000_123_456_789] {
            assert_eq!(system_time_to_ns(ns_to_system_time(ns)), ns);
        }
    }
}
