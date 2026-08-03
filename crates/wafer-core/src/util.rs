//! Small utilities shared across the crate.

use std::time::Duration;

/// Convert a [`Duration`] to nanoseconds, saturating at `u64::MAX`.
///
/// This is the idiomatic replacement for `d.as_nanos() as u64` — it avoids
/// silent truncation from u128→u64 while expressing that overflow is
/// semantically impossible for elapsed wall-clock durations (max representable
/// is ~584 years).
#[inline]
pub fn duration_ns_saturating(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

/// Convert a [`Duration`] to milliseconds, saturating at `u64::MAX`.
#[inline]
pub fn duration_ms_saturating(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}
