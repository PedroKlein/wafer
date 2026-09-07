//! Process RSS memory sampling for evaluation experiments.
//!
//! Samples resident set size at 1Hz for memory scaling measurements (E-Perf-3/6).
//! Uses the `memory-stats` crate for uniform cross-platform behavior (macOS +
//! Linux) without subprocess overhead.
//!
//! See docs/rfcs/RFC-008-evaluation-harness.md — Session 8 D14.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio_util::sync::CancellationToken;

/// Samples process RSS at 1Hz until cancellation.
#[derive(Debug)]
pub struct MemoryRecorder {
    samples: Vec<(u64, u64)>, // (elapsed_ms, rss_bytes)
    start: Option<Instant>,
    start_unix_epoch_ns: Option<u64>,
}

impl MemoryRecorder {
    /// Create a new recorder (does not start sampling).
    #[must_use]
    pub const fn new() -> Self {
        Self { samples: Vec::new(), start: None, start_unix_epoch_ns: None }
    }

    /// Run the sampling loop at 1Hz until the cancellation token fires.
    pub async fn sample_loop(&mut self, cancel: CancellationToken) {
        self.start = Some(Instant::now());
        self.start_unix_epoch_ns = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, crate::util::duration_ns_saturating),
        );
        let mut ticker = tokio::time::interval(Duration::from_secs(1));

        loop {
            tokio::select! {
                () = cancel.cancelled() => {
                    self.sample();
                    break;
                },
                _ = ticker.tick() => {
                    self.sample();
                }
            }
        }
    }

    fn sample(&mut self) {
        if let Some(rss) = read_rss_bytes() {
            let elapsed_ms =
                self.start.map_or(0, |start| crate::util::duration_ms_saturating(start.elapsed()));
            self.samples.push((elapsed_ms, rss));
        }
    }

    #[must_use]
    pub const fn start_unix_epoch_ns(&self) -> Option<u64> {
        self.start_unix_epoch_ns
    }

    /// Collected samples as (elapsed_ms, rss_bytes) pairs.
    #[must_use]
    pub fn samples(&self) -> &[(u64, u64)] {
        &self.samples
    }

    /// Average RSS across all samples.
    #[must_use]
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "division by zero guarded by is_empty() check above"
    )]
    pub fn avg_rss_bytes(&self) -> u64 {
        if self.samples.is_empty() {
            return 0;
        }
        let sum: u64 = self.samples.iter().map(|(_, rss)| rss).sum();
        sum / crate::util::usize_as_u64(self.samples.len())
    }

    /// Peak RSS observed.
    #[must_use]
    pub fn peak_rss_bytes(&self) -> u64 {
        self.samples.iter().map(|(_, rss)| *rss).max().unwrap_or(0)
    }

    /// Export as CSV string: "elapsed_ms,rss_bytes\n..."
    #[must_use]
    #[expect(
        clippy::expect_used,
        reason = "std::fmt::Write for String is infallible — cannot panic"
    )]
    pub fn to_csv(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::from("elapsed_ms,rss_bytes\n");
        for (elapsed, rss) in &self.samples {
            writeln!(out, "{elapsed},{rss}").expect("String write is infallible");
        }
        out
    }
}

impl Default for MemoryRecorder {
    fn default() -> Self {
        Self::new()
    }
}

/// Read current process RSS in bytes using the `memory-stats` crate.
///
/// Cross-platform (macOS + Linux) without subprocess overhead. Returns
/// `None` only when the underlying OS API is unavailable (e.g., WASI).
pub fn read_rss_bytes() -> Option<u64> {
    memory_stats::memory_stats().map(|s| crate::util::usize_as_u64(s.physical_mem))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_stats_source() {
        // AC1: read_rss_bytes uses `memory_stats` crate on macOS + Linux.
        let rss = read_rss_bytes();
        assert!(rss.is_some(), "memory-stats crate should return Some on this platform");
        let rss = rss.unwrap();
        assert!(rss > 0, "RSS should be non-zero for a running process");
        // Sanity: a Rust test process should use at least 1 MB
        assert!(rss > 1_000_000, "RSS {rss} bytes is suspiciously low for a Rust test process");
    }

    #[test]
    fn read_rss_returns_some() {
        let rss = read_rss_bytes();
        assert!(rss.is_some(), "should be able to read RSS on this platform");
        assert!(rss.unwrap() > 0, "RSS should be non-zero for a running process");
    }

    #[test]
    fn read_rss_bytes_within_1hz_overhead_budget() {
        // L-3 (2026-08-02): thesis-hardening T4 AC6 promises <0.1% overhead
        // from the 1 Hz sampler. That reduces to: read_rss_bytes must cost
        // <1 ms per call, since 1 ms out of a 1 s sampling interval is
        // exactly 0.1%. We measure the per-call cost here and gate the AC.
        //
        // Warm-up: the first call can pull the memory_stats OS bridge into
        // hot cache paths. Discard it before timing.
        let _ = read_rss_bytes();

        let start = std::time::Instant::now();
        for _ in 0..200_u32 {
            let rss = read_rss_bytes();
            std::hint::black_box(rss);
        }
        let per_call = start.elapsed() / 200;

        // AC boundary: 1 ms per call = 0.1% overhead at 1 Hz.
        // Real measurement on M1: ~500 ns per call (0.00005%). We give
        // a very generous 1 ms budget so slow CI machines don't flake.
        assert!(
            per_call < std::time::Duration::from_millis(1),
            "read_rss_bytes averaged {per_call:?} per call, above the 1 ms AC boundary. \
             At 1 Hz sampling this would exceed the <0.1% overhead budget."
        );
    }

    #[tokio::test]
    async fn sample_loop_collects_samples() {
        let cancel = CancellationToken::new();
        let mut recorder = MemoryRecorder::new();

        let cancel_clone = cancel.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(2500)).await;
            cancel_clone.cancel();
        });

        recorder.sample_loop(cancel).await;
        handle.await.unwrap();

        // Should have collected at least 2 samples in 2.5 seconds
        assert!(
            recorder.samples().len() >= 2,
            "expected >=2 samples, got {}",
            recorder.samples().len()
        );
    }

    #[test]
    fn csv_format() {
        let mut recorder = MemoryRecorder::new();
        recorder.samples = vec![(0, 1024), (1000, 2048), (2000, 1536)];

        let csv = recorder.to_csv();
        assert!(csv.starts_with("elapsed_ms,rss_bytes\n"));
        assert!(csv.contains("0,1024\n"));
        assert!(csv.contains("1000,2048\n"));
        assert!(csv.contains("2000,1536\n"));
    }

    #[test]
    fn avg_and_peak() {
        let mut recorder = MemoryRecorder::new();
        recorder.samples = vec![(0, 1000), (1000, 2000), (2000, 3000)];

        assert_eq!(recorder.avg_rss_bytes(), 2000);
        assert_eq!(recorder.peak_rss_bytes(), 3000);
    }

    #[test]
    fn empty_recorder() {
        let recorder = MemoryRecorder::new();
        assert_eq!(recorder.avg_rss_bytes(), 0);
        assert_eq!(recorder.peak_rss_bytes(), 0);
        assert_eq!(recorder.to_csv(), "elapsed_ms,rss_bytes\n");
    }
}
