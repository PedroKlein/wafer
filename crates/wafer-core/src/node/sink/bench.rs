//! Evaluation benchmark sink with HdrHistogram latency recording.
//!
//! `BenchSink` records end-to-end latency into an HdrHistogram, detects sequence
//! gaps/duplicates via `SequenceTracker`, and monitors hot-swap version boundaries
//! via `HotSwapRecorder`.
//!
//! See docs/decisions/2025-07-12-evaluation-harness-design.md — Session 8 D4, D7.

use std::future::Future;
use std::pin::Pin;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use hdrhistogram::Histogram;

use crate::error::Result;
use crate::node::{Lifecycle, Sink};
use crate::queue::RuntimeEnvelope;

// =============================================================================
// SequenceTracker
// =============================================================================

/// Tracks message sequence numbers to detect gaps (lost messages) and duplicates.
///
/// Validates E-Swap-2: zero loss, zero duplication during hot-swap.
#[derive(Debug)]
pub struct SequenceTracker {
    expected_next: u64,
    gaps: Vec<(u64, u64)>,
    duplicates: u64,
    total_received: u64,
}

impl SequenceTracker {
    #[must_use]
    pub fn new() -> Self {
        Self {
            expected_next: 0,
            gaps: Vec::new(),
            duplicates: 0,
            total_received: 0,
        }
    }

    /// Record a received sequence number.
    pub fn record(&mut self, seq: u64) {
        self.total_received += 1;

        if seq == self.expected_next {
            self.expected_next += 1;
        } else if seq > self.expected_next {
            // Gap detected: missing [expected_next, seq)
            self.gaps.push((self.expected_next, seq - 1));
            self.expected_next = seq + 1;
        } else {
            // seq < expected_next means duplicate or out-of-order
            self.duplicates += 1;
        }
    }

    /// Total number of missing message slots.
    #[must_use]
    pub fn total_gaps(&self) -> u64 {
        self.gaps.iter().map(|(start, end)| end - start + 1).sum()
    }

    /// Whether any gaps exist.
    #[must_use]
    pub fn has_gaps(&self) -> bool {
        !self.gaps.is_empty()
    }

    /// Number of duplicate/out-of-order messages.
    #[must_use]
    pub fn total_duplicates(&self) -> u64 {
        self.duplicates
    }

    /// Total messages received (including warmup, duplicates).
    #[must_use]
    pub fn total_received(&self) -> u64 {
        self.total_received
    }

    /// Gap ranges for reporting.
    #[must_use]
    pub fn gaps(&self) -> &[(u64, u64)] {
        &self.gaps
    }
}

impl Default for SequenceTracker {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// HotSwapRecorder
// =============================================================================

/// Records hot-swap version transitions for pause duration measurement.
///
/// Detects version boundaries from envelope metadata `plugin.version` field.
#[derive(Debug)]
pub struct HotSwapRecorder {
    current_version: Option<String>,
    last_v1_time_ns: Option<u64>,
    first_v2_time_ns: Option<u64>,
    transitions: Vec<SwapTransition>,
}

/// A recorded version transition with pause duration.
#[derive(Debug, Clone)]
pub struct SwapTransition {
    pub from: String,
    pub to: String,
    pub pause_ns: u64,
}

impl HotSwapRecorder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            current_version: None,
            last_v1_time_ns: None,
            first_v2_time_ns: None,
            transitions: Vec::new(),
        }
    }

    /// Record a version observation with its timestamp.
    pub fn record(&mut self, version: &str, timestamp_ns: u64) {
        match &self.current_version {
            None => {
                self.current_version = Some(version.to_owned());
                self.last_v1_time_ns = Some(timestamp_ns);
            }
            Some(current) if current != version => {
                // Version transition detected
                let from = current.clone();
                let pause_ns = self
                    .last_v1_time_ns
                    .map_or(0, |last| timestamp_ns.saturating_sub(last));

                self.first_v2_time_ns = Some(timestamp_ns);
                self.transitions.push(SwapTransition {
                    from,
                    to: version.to_owned(),
                    pause_ns,
                });

                self.current_version = Some(version.to_owned());
                self.last_v1_time_ns = Some(timestamp_ns);
            }
            Some(_) => {
                // Same version — update last seen time
                self.last_v1_time_ns = Some(timestamp_ns);
            }
        }
    }

    /// Pause duration of the most recent transition.
    #[must_use]
    pub fn pause_duration_ns(&self) -> Option<u64> {
        self.transitions.last().map(|t| t.pause_ns)
    }

    /// All recorded transitions.
    #[must_use]
    pub fn transitions(&self) -> &[SwapTransition] {
        &self.transitions
    }
}

impl Default for HotSwapRecorder {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// BenchSinkConfig
// =============================================================================

/// Configuration for the benchmark sink.
#[derive(Debug, Clone)]
pub struct BenchSinkConfig {
    /// Seconds to discard at start (warmup exclusion).
    pub warmup_secs: u64,
    /// Enable sequence gap/duplicate tracking.
    pub track_sequences: bool,
    /// Enable hot-swap version transition recording.
    pub track_hotswap: bool,
}

impl Default for BenchSinkConfig {
    fn default() -> Self {
        Self {
            warmup_secs: 30,
            track_sequences: true,
            track_hotswap: false,
        }
    }
}

impl BenchSinkConfig {
    /// Create with zero warmup and sequence tracking (useful for tests).
    #[must_use]
    pub fn for_test() -> Self {
        Self {
            warmup_secs: 0,
            track_sequences: true,
            track_hotswap: false,
        }
    }
}

// =============================================================================
// BenchSink
// =============================================================================

/// Evaluation-grade measurement sink.
///
/// Records end-to-end latency into HdrHistogram (3 significant digits,
/// 1µs–10s range). Optionally tracks sequence gaps and hot-swap transitions.
pub struct BenchSink {
    id: String,
    config: BenchSinkConfig,
    /// 3 significant digits, range 1_000ns (1µs) to 10_000_000_000ns (10s).
    histogram: Histogram<u64>,
    warmup_until: Option<Instant>,
    sequence_tracker: Option<SequenceTracker>,
    hotswap_recorder: Option<HotSwapRecorder>,
    message_count: u64,
    started: bool,
}

impl BenchSink {
    /// Create a new BenchSink with the given configuration.
    #[must_use]
    pub fn new(config: BenchSinkConfig) -> Self {
        let sequence_tracker = if config.track_sequences {
            Some(SequenceTracker::new())
        } else {
            None
        };
        let hotswap_recorder = if config.track_hotswap {
            Some(HotSwapRecorder::new())
        } else {
            None
        };

        // Range: 1µs (1000ns) to 10s (10_000_000_000ns), 3 significant digits
        let histogram = Histogram::new_with_bounds(1_000, 10_000_000_000, 3)
            .expect("valid histogram bounds");

        Self {
            id: "bench-sink".to_owned(),
            config,
            histogram,
            warmup_until: None,
            sequence_tracker,
            hotswap_recorder,
            message_count: 0,
            started: false,
        }
    }

    /// Create with a custom node ID.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    // --- Result accessors ---

    /// Median latency in nanoseconds.
    #[must_use]
    pub fn p50_ns(&self) -> u64 {
        self.histogram.value_at_quantile(0.50)
    }

    /// 99th percentile latency in nanoseconds.
    #[must_use]
    pub fn p99_ns(&self) -> u64 {
        self.histogram.value_at_quantile(0.99)
    }

    /// 99.9th percentile latency in nanoseconds.
    #[must_use]
    pub fn p999_ns(&self) -> u64 {
        self.histogram.value_at_quantile(0.999)
    }

    /// Minimum recorded latency in nanoseconds.
    #[must_use]
    pub fn min_ns(&self) -> u64 {
        self.histogram.min()
    }

    /// Maximum recorded latency in nanoseconds.
    #[must_use]
    pub fn max_ns(&self) -> u64 {
        self.histogram.max()
    }

    /// Mean latency in nanoseconds.
    #[must_use]
    pub fn mean_ns(&self) -> f64 {
        self.histogram.mean()
    }

    /// Number of values recorded in the histogram (post-warmup).
    #[must_use]
    pub fn recorded_count(&self) -> u64 {
        self.histogram.len()
    }

    /// Total messages received (including warmup).
    #[must_use]
    pub fn message_count(&self) -> u64 {
        self.message_count
    }

    /// Access the sequence tracker (if enabled).
    #[must_use]
    pub fn sequence_tracker(&self) -> Option<&SequenceTracker> {
        self.sequence_tracker.as_ref()
    }

    /// Access the hot-swap recorder (if enabled).
    #[must_use]
    pub fn hotswap_recorder(&self) -> Option<&HotSwapRecorder> {
        self.hotswap_recorder.as_ref()
    }
}

/// Get current wall-clock time in nanoseconds since UNIX epoch.
fn current_time_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

impl Lifecycle for BenchSink {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &'static str {
        "bench-sink"
    }

    fn validate(&self) -> Result<()> {
        Ok(())
    }

    fn init(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }

    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }
}

impl Sink for BenchSink {
    fn collect(
        &mut self,
        envelope: RuntimeEnvelope,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        // Initialize warmup timer on first message
        if !self.started {
            self.started = true;
            if self.config.warmup_secs > 0 {
                self.warmup_until = Some(
                    Instant::now()
                        + std::time::Duration::from_secs(self.config.warmup_secs),
                );
            }
        }

        self.message_count += 1;

        // Skip recording during warmup
        if let Some(until) = self.warmup_until {
            if Instant::now() < until {
                return Box::pin(async { Ok(()) });
            }
        }

        // Extract intended_ns from metadata for latency calculation
        let intended_ns: Option<u64> = envelope
            .header
            .metadata
            .iter()
            .find(|(k, _)| k.as_ref() == "bench.intended_ns")
            .and_then(|(_, v)| v.parse().ok());

        if let Some(intended) = intended_ns {
            let now = current_time_ns();
            let latency_ns = now.saturating_sub(intended);
            // Clamp to histogram range (ignore out-of-range values)
            if latency_ns >= 1_000 {
                let _ = self.histogram.record(latency_ns);
            } else {
                // Sub-microsecond: record as 1µs minimum
                let _ = self.histogram.record(1_000);
            }
        }

        // Sequence tracking
        if let Some(ref mut tracker) = self.sequence_tracker {
            if let Some(seq) = envelope
                .header
                .metadata
                .iter()
                .find(|(k, _)| k.as_ref() == "bench.sequence")
                .and_then(|(_, v)| v.parse::<u64>().ok())
            {
                tracker.record(seq);
            }
        }

        // Hot-swap version tracking
        if let Some(ref mut recorder) = self.hotswap_recorder {
            if let Some((_, version)) = envelope
                .header
                .metadata
                .iter()
                .find(|(k, _)| k.as_ref() == "plugin.version")
            {
                recorder.record(version.as_ref(), current_time_ns());
            }
        }

        Box::pin(async { Ok(()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::RuntimeEnvelope;

    fn make_bench_envelope(seq: u64) -> RuntimeEnvelope {
        let now_ns = current_time_ns();
        // Subtract a known amount so latency is measurable
        let intended_ns = now_ns.saturating_sub(5_000); // 5µs ago
        RuntimeEnvelope::from_string("bench-source", "payload")
            .with_metadata("bench.sequence", seq.to_string())
            .with_metadata("bench.intended_ns", intended_ns.to_string())
    }

    #[tokio::test]
    async fn bench_sink_warmup_exclusion() {
        let config = BenchSinkConfig {
            warmup_secs: 1,
            track_sequences: false,
            track_hotswap: false,
        };
        let mut sink = BenchSink::new(config);
        sink.init().await.unwrap();

        // Send message during warmup
        let env = make_bench_envelope(0);
        sink.collect(env).await.unwrap();

        // With 1s warmup, this message should not be recorded
        assert_eq!(sink.recorded_count(), 0);
        assert_eq!(sink.message_count(), 1);
    }

    #[tokio::test]
    async fn bench_sink_records_after_zero_warmup() {
        let config = BenchSinkConfig::for_test();
        let mut sink = BenchSink::new(config);
        sink.init().await.unwrap();

        for seq in 0..10 {
            let env = make_bench_envelope(seq);
            sink.collect(env).await.unwrap();
        }

        assert_eq!(sink.message_count(), 10);
        assert_eq!(sink.recorded_count(), 10);
        assert!(sink.p50_ns() > 0);
        assert!(sink.p99_ns() >= sink.p50_ns());
    }

    #[tokio::test]
    async fn sequence_tracker_detects_gap() {
        let mut tracker = SequenceTracker::new();
        tracker.record(0);
        tracker.record(1);
        tracker.record(2);
        tracker.record(5); // Gap: 3, 4 missing

        assert!(tracker.has_gaps());
        assert_eq!(tracker.total_gaps(), 2);
        assert_eq!(tracker.gaps(), &[(3, 4)]);
        assert_eq!(tracker.total_received(), 4);
    }

    #[tokio::test]
    async fn sequence_tracker_detects_duplicate() {
        let mut tracker = SequenceTracker::new();
        tracker.record(0);
        tracker.record(1);
        tracker.record(1); // Duplicate

        assert_eq!(tracker.total_duplicates(), 1);
        assert!(!tracker.has_gaps());
    }

    #[tokio::test]
    async fn sequence_tracker_no_gaps() {
        let mut tracker = SequenceTracker::new();
        for i in 0..100 {
            tracker.record(i);
        }

        assert!(!tracker.has_gaps());
        assert_eq!(tracker.total_gaps(), 0);
        assert_eq!(tracker.total_duplicates(), 0);
        assert_eq!(tracker.total_received(), 100);
    }

    #[tokio::test]
    async fn hotswap_recorder_detects_transition() {
        let mut recorder = HotSwapRecorder::new();

        recorder.record("v1.0", 1_000_000);
        recorder.record("v1.0", 2_000_000);
        recorder.record("v2.0", 5_000_000); // Transition: pause = 5M - 2M = 3M ns

        assert_eq!(recorder.transitions().len(), 1);
        assert_eq!(recorder.transitions()[0].from, "v1.0");
        assert_eq!(recorder.transitions()[0].to, "v2.0");
        assert_eq!(recorder.transitions()[0].pause_ns, 3_000_000);
        assert_eq!(recorder.pause_duration_ns(), Some(3_000_000));
    }

    #[tokio::test]
    async fn hotswap_recorder_no_transition() {
        let mut recorder = HotSwapRecorder::new();
        recorder.record("v1.0", 1_000_000);
        recorder.record("v1.0", 2_000_000);

        assert!(recorder.transitions().is_empty());
        assert_eq!(recorder.pause_duration_ns(), None);
    }

    #[tokio::test]
    async fn bench_sink_lifecycle() {
        let mut sink = BenchSink::new(BenchSinkConfig::for_test());
        assert_eq!(sink.id(), "bench-sink");
        assert_eq!(sink.node_type(), "bench-sink");
        sink.validate().unwrap();
        sink.init().await.unwrap();
        sink.close().await.unwrap();
    }

    #[tokio::test]
    async fn bench_sink_custom_id() {
        let sink = BenchSink::new(BenchSinkConfig::for_test()).with_id("my-sink");
        assert_eq!(sink.id(), "my-sink");
    }
}
