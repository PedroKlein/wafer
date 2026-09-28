//! Small utilities shared across the crate.

use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

/// Nanoseconds since the Unix epoch, read from the monotonic clock.
///
/// The wall clock is read once per process and every later value adds
/// monotonic elapsed time to it, so benchmark timestamps taken in this process
/// can be subtracted from each other even if the wall clock steps mid-run.
pub(crate) fn monotonic_unix_ns() -> u64 {
    let (origin, origin_unix_ns) = *MONOTONIC_ANCHOR;
    origin_unix_ns.saturating_add(duration_ns_saturating(origin.elapsed()))
}

/// How far the wall clock has stepped away from [`monotonic_unix_ns`] since
/// its anchor was taken. NTP rate corrections move both clocks together, so
/// anything beyond a few microseconds is a step.
pub(crate) fn wall_clock_step_ns() -> i64 {
    let wall_ns = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, duration_ns_saturating);
    let step_ns = i128::from(wall_ns).saturating_sub(i128::from(monotonic_unix_ns()));
    i64::try_from(step_ns).unwrap_or(i64::MAX)
}

static MONOTONIC_ANCHOR: LazyLock<(Instant, u64)> = LazyLock::new(|| {
    let unix_ns = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, duration_ns_saturating);
    (Instant::now(), unix_ns)
});

/// Convert a [`Duration`] to milliseconds, saturating at `u64::MAX`.
#[inline]
pub fn duration_ms_saturating(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Lossless `usize` → `u64` conversion.
///
/// On 64-bit targets (all WAFER targets: aarch64, x86-64) this is a no-op.
/// On hypothetical 32-bit targets it widens safely. Exists to satisfy
/// `clippy::as_conversions` without per-site `#[expect]` annotations.
#[inline]
#[expect(
    clippy::as_conversions,
    reason = "usize→u64 is lossless on 64-bit (WAFER targets); From<usize> for u64 is not in std"
)]
pub const fn usize_as_u64(n: usize) -> u64 {
    n as u64
}

/// Write `bytes` to a temporary file beside `path`, sync it, then rename it
/// over `path`, so an interrupted run never leaves a truncated artifact.
///
/// # Errors
/// Any create, write, sync or rename failure, or a `path` without a file name.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let name = path.file_name().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "artifact path has no file name")
    })?;
    let mut temporary_name = OsString::from(".");
    temporary_name.push(name);
    temporary_name.push(".tmp");
    let temporary = path.with_file_name(temporary_name);
    let result = std::fs::File::create(&temporary)
        .and_then(|mut file| file.write_all(bytes).and_then(|()| file.sync_all()))
        .and_then(|()| std::fs::rename(&temporary, path));
    if result.is_err() {
        drop(std::fs::remove_file(&temporary));
    }
    result
}
