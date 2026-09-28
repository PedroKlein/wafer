//! Small utilities shared across the crate.

use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
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
