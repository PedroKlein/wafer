//! Process RSS memory sampling for evaluation experiments.
//!
//! Samples resident set size at 1Hz for memory scaling measurements (E-Perf-3/6).
//! Platform-specific: Linux reads /proc/self/statm, macOS uses `ps`.
//!
//! See docs/decisions/2025-07-12-evaluation-harness-design.md — Session 8 D14.

use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

/// Samples process RSS at 1Hz until cancellation.
#[derive(Debug)]
pub struct MemoryRecorder {
    samples: Vec<(u64, u64)>, // (elapsed_ms, rss_bytes)
    start: Option<Instant>,
}

impl MemoryRecorder {
    /// Create a new recorder (does not start sampling).
    #[must_use]
    pub fn new() -> Self {
        Self {
            samples: Vec::new(),
            start: None,
        }
    }

    /// Run the sampling loop at 1Hz until the cancellation token fires.
    pub async fn sample_loop(&mut self, cancel: CancellationToken) {
        self.start = Some(Instant::now());
        let mut ticker = tokio::time::interval(Duration::from_secs(1));

        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = ticker.tick() => {
                    if let Some(rss) = read_rss_bytes() {
                        let elapsed_ms = self.start.map_or(0, |s| s.elapsed().as_millis() as u64);
                        self.samples.push((elapsed_ms, rss));
                    }
                }
            }
        }
    }

    /// Collected samples as (elapsed_ms, rss_bytes) pairs.
    #[must_use]
    pub fn samples(&self) -> &[(u64, u64)] {
        &self.samples
    }

    /// Average RSS across all samples.
    #[must_use]
    pub fn avg_rss_bytes(&self) -> u64 {
        if self.samples.is_empty() {
            return 0;
        }
        let sum: u64 = self.samples.iter().map(|(_, rss)| rss).sum();
        sum / self.samples.len() as u64
    }

    /// Peak RSS observed.
    #[must_use]
    pub fn peak_rss_bytes(&self) -> u64 {
        self.samples.iter().map(|(_, rss)| *rss).max().unwrap_or(0)
    }

    /// Export as CSV string: "elapsed_ms,rss_bytes\n..."
    #[must_use]
    pub fn to_csv(&self) -> String {
        let mut out = String::from("elapsed_ms,rss_bytes\n");
        for (elapsed, rss) in &self.samples {
            out.push_str(&format!("{elapsed},{rss}\n"));
        }
        out
    }
}

impl Default for MemoryRecorder {
    fn default() -> Self {
        Self::new()
    }
}

/// Read current process RSS in bytes. Platform-specific.
#[cfg(target_os = "linux")]
pub fn read_rss_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let resident_pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    // Standard page size on Linux
    Some(resident_pages * 4096)
}

#[cfg(target_os = "macos")]
pub fn read_rss_bytes() -> Option<u64> {
    use std::process::Command;
    let pid = std::process::id();
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let rss_kb: u64 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .ok()?;
    Some(rss_kb * 1024)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn read_rss_bytes() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_rss_returns_some() {
        let rss = read_rss_bytes();
        assert!(rss.is_some(), "should be able to read RSS on this platform");
        assert!(rss.unwrap() > 0, "RSS should be non-zero for a running process");
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
