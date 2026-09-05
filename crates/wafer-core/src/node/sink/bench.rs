//! Evaluation benchmark sink with HdrHistogram latency recording.
//!
//! `BenchSink` records end-to-end latency into an HdrHistogram, detects sequence
//! gaps/duplicates via `SequenceTracker`, and monitors hot-swap version boundaries
//! via `HotSwapRecorder`.
//!
//! See docs/rfcs/RFC-008-evaluation-harness.md — Session 8 D4, D7, D9.

use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hdrhistogram::Histogram;
use hdrhistogram::serialization::V2Serializer;
use hdrhistogram::serialization::interval_log::IntervalLogWriterBuilder;

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
    first_expected: Option<u64>,
    expected_next: u64,
    gaps: Vec<(u64, u64)>,
    duplicates: u64,
    total_received: u64,
}

impl SequenceTracker {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            first_expected: Some(0),
            expected_next: 0,
            gaps: Vec::new(),
            duplicates: 0,
            total_received: 0,
        }
    }

    #[must_use]
    const fn from_first_observed() -> Self {
        Self {
            first_expected: None,
            expected_next: 0,
            gaps: Vec::new(),
            duplicates: 0,
            total_received: 0,
        }
    }

    const fn anchor_at(&mut self, expected: u64) {
        if self.first_expected.is_none() {
            self.first_expected = Some(expected);
            self.expected_next = expected;
        }
    }

    /// Record a received sequence number. Returns whether it was a duplicate.
    pub fn record(&mut self, seq: u64) -> bool {
        self.anchor_at(seq);
        self.total_received = self.total_received.saturating_add(1);

        match seq.cmp(&self.expected_next) {
            std::cmp::Ordering::Equal => {
                self.expected_next = self.expected_next.saturating_add(1);
                false
            }
            std::cmp::Ordering::Greater => {
                self.gaps.push((self.expected_next, seq.saturating_sub(1)));
                self.expected_next = seq.saturating_add(1);
                false
            }
            std::cmp::Ordering::Less => {
                self.duplicates = self.duplicates.saturating_add(1);
                true
            }
        }
    }

    /// Number of sequence positions spanned since tracking began.
    #[must_use]
    pub fn total_expected(&self) -> u64 {
        self.first_expected.map_or(0, |first| self.expected_next.saturating_sub(first))
    }

    /// Total number of missing message slots.
    #[must_use]
    pub fn total_gaps(&self) -> u64 {
        self.gaps.iter().map(|(start, end)| end.saturating_sub(*start).saturating_add(1)).sum()
    }

    /// Whether any gaps exist.
    #[must_use]
    pub fn has_gaps(&self) -> bool {
        !self.gaps.is_empty()
    }

    /// Number of duplicate/out-of-order messages.
    #[must_use]
    pub const fn total_duplicates(&self) -> u64 {
        self.duplicates
    }

    /// Total messages received (including warmup, duplicates).
    #[must_use]
    pub const fn total_received(&self) -> u64 {
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
    pub const fn new() -> Self {
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
                let pause_ns =
                    self.last_v1_time_ns.map_or(0, |last| timestamp_ns.saturating_sub(last));

                self.first_v2_time_ns = Some(timestamp_ns);
                self.transitions.push(SwapTransition { from, to: version.to_owned(), pause_ns });

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

    /// Timestamp of the LAST v1 observation before the most recent transition.
    ///
    /// Combined with [`first_v2_ns`](Self::first_v2_ns), lets E-Swap-1 compute
    /// the pause window as `first_v2 - last_v1`.
    #[must_use]
    pub fn last_v1_ns(&self) -> Option<u64> {
        // `last_v1_time_ns` is refreshed on every observation of the current
        // version. When a transition occurs, `record()` snapshots the OLD
        // `last_v1_time_ns` into `SwapTransition.pause_ns` and then updates it
        // to the new-version timestamp. The most recent transition's
        // `pause_ns = first_v2 - last_v1_before_transition`, so we reconstruct
        // `last_v1` from `first_v2 - pause_ns`.
        match (self.first_v2_time_ns, self.transitions.last()) {
            (Some(first_v2), Some(t)) => Some(first_v2.saturating_sub(t.pause_ns)),
            _ => None,
        }
    }

    /// Timestamp of the FIRST v2 observation (the message where the version
    /// transition was first observed).
    #[must_use]
    pub const fn first_v2_ns(&self) -> Option<u64> {
        self.first_v2_time_ns
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
    /// Output directory for auto-export on close(). None = no auto-export.
    pub output_dir: Option<PathBuf>,
}

impl Default for BenchSinkConfig {
    fn default() -> Self {
        Self { warmup_secs: 30, track_sequences: true, track_hotswap: false, output_dir: None }
    }
}

impl BenchSinkConfig {
    /// Create with zero warmup and sequence tracking (useful for tests).
    #[must_use]
    pub const fn for_test() -> Self {
        Self { warmup_secs: 0, track_sequences: true, track_hotswap: false, output_dir: None }
    }

    /// Set output directory for auto-export on close.
    #[must_use]
    pub fn with_output_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.output_dir = Some(dir.into());
        self
    }
}

// =============================================================================
// ThroughputSample
// =============================================================================

/// A single throughput measurement for one time bucket (1s resolution).
#[derive(Debug, Clone)]
pub struct ThroughputSample {
    /// Seconds since measurement start (after warmup).
    pub elapsed_secs: f64,
    /// Messages received in this bucket.
    pub msg_count: u64,
    /// Bytes received in this bucket.
    pub bytes: u64,
}

const BURST_BUCKET_WIDTH_NS: u64 = 100_000_000;
const BURST_BUCKET_COUNT: usize = 1_200;

#[derive(Debug, Clone, Copy, Default)]
struct BurstBucket {
    received_unique: u64,
    received_events: u64,
    duplicates: u64,
}

// =============================================================================
// BenchSink
// =============================================================================

/// Evaluation-grade measurement sink.
///
/// Records end-to-end latency into HdrHistogram (3 significant digits,
/// 1µs–10s range). Optionally tracks sequence gaps and hot-swap transitions.
/// Supports periodic throughput sampling (1s buckets) and export to HDR/CSV.
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
    /// Wall-clock start time for interval log.
    start_wall_time: Option<SystemTime>,
    /// Throughput tracking: time measurement started (after warmup).
    measurement_start: Option<Instant>,
    /// Current bucket start time.
    current_bucket_start: Option<Instant>,
    /// Messages in current bucket.
    bucket_msg_count: u64,
    /// Bytes in current bucket.
    bucket_bytes: u64,
    /// Completed throughput samples.
    throughput_samples: Vec<ThroughputSample>,
    burst_buckets: Option<Box<[BurstBucket]>>,
    burst_phase_received: [u64; 3],
}

impl BenchSink {
    /// Create a new BenchSink with the given configuration.
    ///
    /// # Panics
    ///
    /// Panics if the internal histogram cannot be created (compile-time constant bounds; unreachable in practice).
    #[must_use]
    #[expect(
        clippy::expect_used,
        reason = "Histogram bounds are compile-time constants (1µs–10s, 3 sig figs); cannot fail"
    )]
    pub fn new(config: BenchSinkConfig) -> Self {
        let sequence_tracker = if config.track_sequences {
            Some(if config.warmup_secs == 0 {
                SequenceTracker::new()
            } else {
                SequenceTracker::from_first_observed()
            })
        } else {
            None
        };
        let hotswap_recorder =
            if config.track_hotswap { Some(HotSwapRecorder::new()) } else { None };

        // Range: 1µs (1000ns) to 10s (10_000_000_000ns), 3 significant digits
        let histogram =
            Histogram::new_with_bounds(1_000, 10_000_000_000, 3).expect("valid histogram bounds");

        Self {
            id: "bench-sink".to_owned(),
            config,
            histogram,
            warmup_until: None,
            sequence_tracker,
            hotswap_recorder,
            message_count: 0,
            started: false,
            start_wall_time: None,
            measurement_start: None,
            current_bucket_start: None,
            bucket_msg_count: 0,
            bucket_bytes: 0,
            throughput_samples: Vec::new(),
            burst_buckets: None,
            burst_phase_received: [0; 3],
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
    pub const fn message_count(&self) -> u64 {
        self.message_count
    }

    /// Access the sequence tracker (if enabled).
    #[must_use]
    pub const fn sequence_tracker(&self) -> Option<&SequenceTracker> {
        self.sequence_tracker.as_ref()
    }

    /// Access the hot-swap recorder (if enabled).
    #[must_use]
    pub const fn hotswap_recorder(&self) -> Option<&HotSwapRecorder> {
        self.hotswap_recorder.as_ref()
    }

    /// Access throughput samples collected during measurement.
    #[must_use]
    pub fn throughput_samples(&self) -> &[ThroughputSample] {
        &self.throughput_samples
    }

    /// Access the raw histogram.
    #[must_use]
    pub const fn histogram(&self) -> &Histogram<u64> {
        &self.histogram
    }

    // --- Export methods (D9 recording output) ---

    /// Serialize histogram to HdrHistogram interval log format.
    ///
    /// Format compatible with HdrHistogram tooling and the Python `hdr_loader.py`.
    /// Single interval spanning the entire measurement period.
    ///
    /// # Panics
    ///
    /// Panics if in-memory histogram serialization fails (unreachable: buffers are always valid).
    #[expect(
        clippy::expect_used,
        reason = "HdrHistogram writer/serialization operates on in-memory buffers; UTF-8 guaranteed from ASCII content"
    )]
    pub fn to_hdr_log(&self) -> String {
        let mut buf = Vec::new();
        let mut serializer = V2Serializer::new();

        let start_time = self.start_wall_time.unwrap_or(UNIX_EPOCH);

        let mut writer_builder = IntervalLogWriterBuilder::new();
        writer_builder
            .with_start_time(start_time)
            .with_base_time(start_time)
            .add_comment("WAFER BenchSink latency histogram (nanoseconds)")
            .add_comment(&format!("Total messages: {}", self.message_count))
            .add_comment(&format!("Recorded values: {}", self.histogram.len()))
            .add_comment(&format!("Warmup: {}s", self.config.warmup_secs));

        let mut log_writer =
            writer_builder.begin_log_with(&mut buf, &mut serializer).expect("begin interval log");

        // Write as a single interval spanning the full measurement
        let duration = self.measurement_start.map_or(Duration::ZERO, |start| start.elapsed());

        log_writer
            .write_histogram(
                &self.histogram,
                Duration::ZERO,
                duration,
                hdrhistogram::serialization::interval_log::Tag::new("latency_ns"),
            )
            .expect("write histogram");

        String::from_utf8(buf).expect("valid UTF-8 from interval log")
    }

    /// Generate throughput CSV content.
    ///
    /// Format: `elapsed_secs,msg_count,bytes`
    /// One row per 1-second bucket.
    #[expect(
        clippy::expect_used,
        reason = "std::fmt::Write for String is infallible — cannot panic"
    )]
    pub fn throughput_csv(&self) -> String {
        use std::fmt::Write as _;
        let mut csv = String::from("elapsed_secs,msg_count,bytes\n");
        for sample in &self.throughput_samples {
            writeln!(csv, "{:.3},{},{}", sample.elapsed_secs, sample.msg_count, sample.bytes)
                .expect("String write is infallible");
        }
        csv
    }

    /// Export all measurement data to a directory.
    ///
    /// Creates:
    /// - `latency.hdr` — HdrHistogram interval log
    /// - `throughput.csv` — periodic throughput samples
    /// - `measurement-window.json` — exact post-warmup wall-clock bounds
    /// - `sequence.csv` — gap and duplicate accounting (only when the
    ///   sink was constructed with `track_sequences = true`)
    /// - `swap_timeline.json` — per-transition timeline for hot-swap
    ///   experiments (only when `track_hotswap = true` and at least one
    ///   transition has been observed)
    ///
    /// # Errors
    /// Returns IO errors from directory creation or file writing.
    pub fn export_to_dir(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;

        // Write latency.hdr
        let hdr_content = self.to_hdr_log();
        let mut hdr_file = std::fs::File::create(dir.join("latency.hdr"))?;
        hdr_file.write_all(hdr_content.as_bytes())?;

        // Write throughput.csv
        let csv_content = self.throughput_csv();
        let mut csv_file = std::fs::File::create(dir.join("throughput.csv"))?;
        csv_file.write_all(csv_content.as_bytes())?;

        if let Some(buckets) = &self.burst_buckets {
            let rows = buckets
                .iter()
                .enumerate()
                .map(|(index, bucket)| {
                    let start_offset_ns = u64::try_from(index)
                        .unwrap_or(u64::MAX)
                        .saturating_mul(BURST_BUCKET_WIDTH_NS);
                    serde_json::json!({
                        "start_offset_ns": start_offset_ns,
                        "end_offset_ns": start_offset_ns.saturating_add(BURST_BUCKET_WIDTH_NS),
                        "received_unique": bucket.received_unique,
                        "received_events": bucket.received_events,
                        "duplicates": bucket.duplicates,
                        "rate_msg_s": bucket.received_unique.saturating_mul(10),
                    })
                })
                .collect::<Vec<_>>();
            let received_unique = buckets.iter().map(|bucket| bucket.received_unique).sum::<u64>();
            let received_events = buckets.iter().map(|bucket| bucket.received_events).sum::<u64>();
            let duplicates = buckets.iter().map(|bucket| bucket.duplicates).sum::<u64>();
            let value = serde_json::json!({
                "schema_version": 1,
                "clock": "monotonic",
                "bucket_width_ns": BURST_BUCKET_WIDTH_NS,
                "coverage_start_offset_ns": 0,
                "coverage_end_offset_ns": 120_000_000_000_u64,
                "received_unique": received_unique,
                "received_events": received_events,
                "duplicates": duplicates,
                "phase_received_messages": self.burst_phase_received,
                "buckets": rows,
            });
            std::fs::write(
                dir.join("throughput-buckets.json"),
                format!("{}\n", serde_json::to_string_pretty(&value)?),
            )?;
        }

        if let Some(started) = self.start_wall_time {
            let started_ns =
                started.duration_since(UNIX_EPOCH).map_or(0, crate::util::duration_ns_saturating);
            let finished_ns = current_time_ns();
            let mut window_file = std::fs::File::create(dir.join("measurement-window.json"))?;
            writeln!(window_file, "{{\"started_ns\":{started_ns},\"finished_ns\":{finished_ns}}}")?;
        }

        // Write sequence.csv when the sink was configured to track sequences.
        // Absence of the file signals "not tracked" — the P1.1 result contract
        // treats sequence.csv as conditional-on-configuration.
        if let Some(tracker) = &self.sequence_tracker {
            let total_expected = tracker.total_expected();
            let mut seq_file = std::fs::File::create(dir.join("sequence.csv"))?;
            writeln!(
                seq_file,
                "total_expected,total_received,gap_ranges,gap_msgs,duplicates_count"
            )?;
            writeln!(
                seq_file,
                "{},{},{},{},{}",
                total_expected,
                tracker.total_received(),
                tracker.gaps().len(),
                tracker.total_gaps(),
                tracker.total_duplicates(),
            )?;
        }

        // Write swap_timeline.json when the hot-swap recorder observed a
        // transition. Even a single-transition dataset is worth emitting so
        // downstream analysis notebooks can compute pause statistics without
        // scraping stdout.
        if let Some(recorder) = &self.hotswap_recorder
            && !recorder.transitions().is_empty()
        {
            let mut swap_file = std::fs::File::create(dir.join("swap_timeline.json"))?;
            let transitions_json: Vec<String> = recorder
                .transitions()
                .iter()
                .map(|t| {
                    format!(
                        "{{\"from\":\"{}\",\"to\":\"{}\",\"pause_ns\":{}}}",
                        t.from.replace('"', "\\\""),
                        t.to.replace('"', "\\\""),
                        t.pause_ns,
                    )
                })
                .collect();
            writeln!(
                swap_file,
                "{{\"transitions\":[{}],\"first_v2_ns\":{},\"last_v1_ns\":{}}}",
                transitions_json.join(","),
                recorder.first_v2_ns().map_or_else(|| String::from("null"), |v| v.to_string()),
                recorder.last_v1_ns().map_or_else(|| String::from("null"), |v| v.to_string()),
            )?;
        }

        Ok(())
    }

    fn record_sequence(&mut self, envelope: &RuntimeEnvelope) -> bool {
        let Some(tracker) = &mut self.sequence_tracker else { return false };
        let Some(seq) = envelope
            .header
            .metadata
            .iter()
            .find(|(key, _)| key.as_ref() == "bench.sequence")
            .and_then(|(_, value)| value.parse::<u64>().ok())
        else {
            return false;
        };
        if let Some(start) = envelope
            .header
            .metadata
            .iter()
            .find(|(key, _)| key.as_ref() == "bench.measurement_start_seq")
            .and_then(|(_, value)| value.parse::<u64>().ok())
        {
            tracker.anchor_at(start);
        }
        tracker.record(seq)
    }

    fn record_burst_bucket(&mut self, envelope: &RuntimeEnvelope, now: Instant, duplicate: bool) {
        let Some((_, phase)) =
            envelope.header.metadata.iter().find(|(key, _)| key.as_ref() == "bench.phase")
        else {
            return;
        };
        let buckets = self.burst_buckets.get_or_insert_with(|| {
            vec![BurstBucket::default(); BURST_BUCKET_COUNT].into_boxed_slice()
        });
        if let Some(start) = self.measurement_start {
            let elapsed_ns = crate::util::duration_ns_saturating(now.duration_since(start));
            let index =
                usize::try_from(elapsed_ns / BURST_BUCKET_WIDTH_NS).unwrap_or(BURST_BUCKET_COUNT);
            if let Some(bucket) = buckets.get_mut(index) {
                bucket.received_events = bucket.received_events.saturating_add(1);
                if duplicate {
                    bucket.duplicates = bucket.duplicates.saturating_add(1);
                } else {
                    bucket.received_unique = bucket.received_unique.saturating_add(1);
                }
            }
        }
        if let Some(count) = ["before", "burst", "after"]
            .iter()
            .position(|candidate| candidate == &phase.as_ref())
            .and_then(|index| self.burst_phase_received.get_mut(index))
        {
            *count = count.saturating_add(1);
        }
    }

    /// Flush current throughput bucket if ≥1s has elapsed.
    fn flush_bucket_if_needed(&mut self, now: Instant) {
        let Some(bucket_start) = self.current_bucket_start else { return };
        let elapsed = now.duration_since(bucket_start);

        if elapsed >= Duration::from_secs(1) {
            let elapsed_since_measurement =
                self.measurement_start.map_or(0.0, |s| now.duration_since(s).as_secs_f64());

            self.throughput_samples.push(ThroughputSample {
                elapsed_secs: elapsed_since_measurement,
                msg_count: self.bucket_msg_count,
                bytes: self.bucket_bytes,
            });

            // Reset bucket
            self.current_bucket_start = Some(now);
            self.bucket_msg_count = 0;
            self.bucket_bytes = 0;
        }
    }
}

/// Get current wall-clock time in nanoseconds since UNIX epoch.
fn current_time_ns() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, crate::util::duration_ns_saturating)
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
        // Flush final throughput bucket
        if self.bucket_msg_count > 0 {
            if let Some(measurement_start) = self.measurement_start {
                let now = Instant::now();
                let elapsed_since_measurement = now.duration_since(measurement_start).as_secs_f64();
                self.throughput_samples.push(ThroughputSample {
                    elapsed_secs: elapsed_since_measurement,
                    msg_count: self.bucket_msg_count,
                    bytes: self.bucket_bytes,
                });
                self.bucket_msg_count = 0;
                self.bucket_bytes = 0;
            }
        }

        // Auto-export if output_dir configured
        if let Some(ref dir) = self.config.output_dir {
            if let Err(e) = self.export_to_dir(dir) {
                tracing::error!("BenchSink export to {:?} failed: {}", dir, e);
            } else {
                tracing::info!(
                    "BenchSink exported results to {:?} (latency.hdr + throughput.csv)",
                    dir
                );
            }
        }

        Box::pin(async { Ok(()) })
    }
}

#[expect(
    clippy::let_underscore_must_use,
    reason = "histogram record errors on out-of-range values: silently dropping is correct for latency sampling"
)]
impl Sink for BenchSink {
    fn collect(
        &mut self,
        envelope: RuntimeEnvelope,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        // Initialize warmup timer on first message
        if !self.started {
            self.started = true;
            if self.config.warmup_secs > 0 {
                #[expect(
                    clippy::arithmetic_side_effects,
                    reason = "Instant + Duration cannot overflow for realistic warmup values"
                )]
                let deadline =
                    Instant::now() + std::time::Duration::from_secs(self.config.warmup_secs);
                self.warmup_until = Some(deadline);
            }
        }

        self.message_count = self.message_count.saturating_add(1);

        // BenchSource marks the exact warmup population. Other sources retain
        // the wall-clock fallback because they do not know the benchmark phase.
        let warmup_marker = envelope
            .header
            .metadata
            .iter()
            .find(|(key, _)| key.as_ref() == "bench.warmup")
            .and_then(|(_, value)| value.parse::<bool>().ok());
        let in_warmup = warmup_marker
            .unwrap_or_else(|| self.warmup_until.is_some_and(|until| Instant::now() < until));
        if in_warmup {
            return Box::pin(async { Ok(()) });
        }

        // Initialize measurement tracking on first post-warmup message
        let now = Instant::now();
        if self.measurement_start.is_none() {
            self.measurement_start = Some(now);
            self.current_bucket_start = Some(now);
            self.start_wall_time = Some(SystemTime::now());
        }

        let duplicate = self.record_sequence(&envelope);
        self.record_burst_bucket(&envelope, now, duplicate);

        // Throughput tracking
        let payload_len = crate::util::usize_as_u64(envelope.payload.len());
        self.bucket_msg_count = self.bucket_msg_count.saturating_add(1);
        self.bucket_bytes = self.bucket_bytes.saturating_add(payload_len);
        self.flush_bucket_if_needed(now);

        // Extract intended_ns from metadata for latency calculation
        let intended_ns: Option<u64> = envelope
            .header
            .metadata
            .iter()
            .find(|(k, _)| k.as_ref() == "bench.intended_ns")
            .and_then(|(_, v)| v.parse().ok());

        if let Some(intended) = intended_ns {
            let now_ns = current_time_ns();
            let latency_ns = now_ns.saturating_sub(intended);
            // Clamp to histogram range (ignore out-of-range values)
            if latency_ns >= 1_000 {
                let _ = self.histogram.record(latency_ns);
            } else {
                // Sub-microsecond: record as 1µs minimum
                let _ = self.histogram.record(1_000);
            }
        }

        // Hot-swap version tracking
        if let Some(ref mut recorder) = self.hotswap_recorder {
            if let Some((_, version)) =
                envelope.header.metadata.iter().find(|(k, _)| k.as_ref() == "plugin.version")
            {
                recorder.record(version.as_ref(), current_time_ns());
            }
        }

        Box::pin(async { Ok(()) })
    }
}

#[cfg(test)]
#[expect(
    clippy::let_underscore_must_use,
    reason = "test code: fire-and-forget histogram records and fs cleanup"
)]
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
            track_sequences: true,
            track_hotswap: false,
            output_dir: None,
        };
        let mut sink = BenchSink::new(config);
        sink.init().await.unwrap();

        // Send message during warmup
        let env = make_bench_envelope(0);
        sink.collect(env).await.unwrap();

        // With 1s warmup, this message should not be recorded
        assert_eq!(sink.recorded_count(), 0);
        assert_eq!(sink.message_count(), 1);
        let tracker = sink.sequence_tracker().unwrap();
        assert_eq!(tracker.total_received(), 0);
        assert!(!tracker.has_gaps());

        let tmp_dir =
            std::env::temp_dir().join(format!("wafer-warmup-window-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp_dir);
        sink.export_to_dir(&tmp_dir).unwrap();
        assert!(!tmp_dir.join("measurement-window.json").exists());

        tokio::time::sleep(Duration::from_millis(1_010)).await;
        let before_measurement_ns = current_time_ns();
        sink.collect(make_bench_envelope(1)).await.unwrap();
        let tracker = sink.sequence_tracker().unwrap();
        assert_eq!(tracker.total_expected(), 1);
        assert_eq!(tracker.total_received(), 1);
        assert!(!tracker.has_gaps());
        sink.export_to_dir(&tmp_dir).unwrap();

        let sequence = std::fs::read_to_string(tmp_dir.join("sequence.csv")).unwrap();
        assert!(sequence.contains("1,1,0,0,0"));

        let window: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(tmp_dir.join("measurement-window.json")).unwrap(),
        )
        .unwrap();
        assert!(window["started_ns"].as_u64().unwrap() >= before_measurement_ns);
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[tokio::test]
    async fn bench_sink_uses_source_warmup_marker_for_sequence_population() {
        let config = BenchSinkConfig {
            warmup_secs: 30,
            track_sequences: true,
            track_hotswap: false,
            output_dir: None,
        };
        let mut sink = BenchSink::new(config);
        sink.init().await.unwrap();

        sink.collect(make_bench_envelope(29_999).with_metadata("bench.warmup", "true"))
            .await
            .unwrap();
        sink.collect(make_bench_envelope(30_000).with_metadata("bench.warmup", "false"))
            .await
            .unwrap();
        sink.collect(make_bench_envelope(30_001).with_metadata("bench.warmup", "false"))
            .await
            .unwrap();

        let tracker = sink.sequence_tracker().unwrap();
        assert_eq!(tracker.total_expected(), 2);
        assert_eq!(tracker.total_received(), 2);
        assert!(!tracker.has_gaps());
        assert_eq!(sink.recorded_count(), 2);
    }

    #[tokio::test]
    async fn bench_sink_detects_missing_first_measurement_message() {
        let config = BenchSinkConfig {
            warmup_secs: 30,
            track_sequences: true,
            track_hotswap: false,
            output_dir: None,
        };
        let mut sink = BenchSink::new(config);
        sink.init().await.unwrap();

        sink.collect(
            make_bench_envelope(30_001)
                .with_metadata("bench.warmup", "false")
                .with_metadata("bench.measurement_start_seq", "30000"),
        )
        .await
        .unwrap();

        let tracker = sink.sequence_tracker().unwrap();
        assert_eq!(tracker.total_expected(), 2);
        assert_eq!(tracker.total_received(), 1);
        assert_eq!(tracker.total_gaps(), 1);
        assert_eq!(tracker.gaps(), &[(30_000, 30_000)]);
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

    #[test]
    fn sequence_tracker_can_anchor_at_first_observed_value() {
        let mut tracker = SequenceTracker::from_first_observed();
        tracker.record(30_001);
        tracker.record(30_002);

        assert_eq!(tracker.total_expected(), 2);
        assert_eq!(tracker.total_received(), 2);
        assert!(!tracker.has_gaps());
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

    /// P0.5 AC3: BenchSink's HotSwapRecorder detects the v1 → v2 transition
    /// via envelope metadata `plugin.version`, and both `first_v2_ns()` and
    /// `last_v1_ns()` are accessible and ordered.
    #[tokio::test]
    async fn swap_boundary_detection() {
        use crate::queue::RuntimeEnvelope;

        let config = BenchSinkConfig {
            warmup_secs: 0,
            track_sequences: false,
            track_hotswap: true,
            output_dir: None,
        };
        let mut sink = BenchSink::new(config);
        sink.init().await.unwrap();

        // Twenty v1 messages, then twenty v2 messages. The version stamp is
        // envelope metadata `plugin.version`, exactly what pass-through-v1
        // and pass-through-v2 emit at runtime.
        for seq in 0..20 {
            let env = RuntimeEnvelope::from_string("bench-source", "payload")
                .with_metadata("bench.sequence", seq.to_string())
                .with_metadata("plugin.version", "1.0.0");
            sink.collect(env).await.unwrap();
        }
        // Give the recorder a monotonic gap between phases so first_v2_ns
        // is strictly greater than last_v1_ns even at sub-ns clock resolution.
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        for seq in 20..40 {
            let env = RuntimeEnvelope::from_string("bench-source", "payload")
                .with_metadata("bench.sequence", seq.to_string())
                .with_metadata("plugin.version", "2.0.0");
            sink.collect(env).await.unwrap();
        }

        let recorder = sink.hotswap_recorder().expect("track_hotswap=true set on config");
        assert_eq!(recorder.transitions().len(), 1, "exactly one v1→v2 transition");
        let last_v1 =
            recorder.last_v1_ns().expect("last_v1_ns must be populated after a transition");
        let first_v2 =
            recorder.first_v2_ns().expect("first_v2_ns must be populated after a transition");
        assert!(
            first_v2 > last_v1,
            "first_v2_ns ({first_v2}) must strictly exceed last_v1_ns ({last_v1})"
        );
        // Pause duration matches (first_v2 - last_v1) definition.
        let pause = recorder.pause_duration_ns().unwrap();
        assert_eq!(pause, first_v2 - last_v1, "pause = first_v2 - last_v1");
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

    // --- Export tests ---

    #[tokio::test]
    async fn to_hdr_log_produces_valid_format() {
        let config = BenchSinkConfig::for_test();
        let mut sink = BenchSink::new(config);
        sink.init().await.unwrap();

        for seq in 0..50 {
            let env = make_bench_envelope(seq);
            sink.collect(env).await.unwrap();
        }

        let hdr_log = sink.to_hdr_log();

        // Should contain required HdrHistogram interval log markers
        assert!(hdr_log.contains("#[StartTime"), "missing StartTime header");
        assert!(hdr_log.contains("#[BaseTime"), "missing BaseTime header");
        assert!(hdr_log.contains("WAFER BenchSink latency histogram"), "missing comment");
        assert!(hdr_log.contains("Total messages: 50"), "missing total messages");
        assert!(hdr_log.contains("Recorded values: 50"), "missing recorded count");
        // Should contain at least one encoded histogram line (Tag:start:duration:...)
        assert!(
            hdr_log.lines().any(|l| l.starts_with("Tag=latency_ns,")),
            "missing histogram data line"
        );
    }

    #[tokio::test]
    async fn to_hdr_log_empty_histogram() {
        let config = BenchSinkConfig::for_test();
        let sink = BenchSink::new(config);

        // No messages → should still produce valid (empty) log
        let hdr_log = sink.to_hdr_log();
        assert!(hdr_log.contains("#[StartTime"));
        assert!(hdr_log.contains("Recorded values: 0"));
    }

    #[tokio::test]
    async fn throughput_csv_header_present() {
        let config = BenchSinkConfig::for_test();
        let mut sink = BenchSink::new(config);
        sink.init().await.unwrap();

        // Send a few messages (not enough time for a bucket flush)
        for seq in 0..5 {
            let env = make_bench_envelope(seq);
            sink.collect(env).await.unwrap();
        }

        let csv = sink.throughput_csv();
        assert!(csv.starts_with("elapsed_secs,msg_count,bytes\n"));
    }

    #[tokio::test]
    async fn burst_metadata_writes_bounded_100ms_sink_buckets() {
        let mut sink = BenchSink::new(BenchSinkConfig::for_test());
        sink.init().await.unwrap();
        for (sequence, phase) in [(0, "before"), (1, "burst"), (2, "after")] {
            let envelope = make_bench_envelope(sequence)
                .with_metadata("bench.measurement_start_seq", "0")
                .with_metadata("bench.warmup", "false")
                .with_metadata("bench.phase", phase);
            sink.collect(envelope).await.unwrap();
        }

        let dir = std::env::temp_dir().join(format!("wafer-burst-buckets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        sink.export_to_dir(&dir).unwrap();
        let value: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("throughput-buckets.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(value["bucket_width_ns"], 100_000_000);
        assert_eq!(value["buckets"].as_array().unwrap().len(), 1_200);
        assert_eq!(value["received_events"], 3);
        assert_eq!(value["received_unique"], 3);
        assert_eq!(value["duplicates"], 0);
        assert_eq!(value["phase_received_messages"], serde_json::json!([1, 1, 1]));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn throughput_sampling_flushes_on_close() {
        let config = BenchSinkConfig::for_test();
        let mut sink = BenchSink::new(config);
        sink.init().await.unwrap();

        for seq in 0..10 {
            let env = make_bench_envelope(seq);
            sink.collect(env).await.unwrap();
        }

        // Before close — bucket is pending (not flushed because <1s elapsed)
        assert!(sink.throughput_samples().is_empty());

        // Close flushes the final bucket
        sink.close().await.unwrap();

        assert_eq!(sink.throughput_samples().len(), 1);
        assert_eq!(sink.throughput_samples()[0].msg_count, 10);
        // Each "payload" is 7 bytes
        assert_eq!(sink.throughput_samples()[0].bytes, 7 * 10);
    }

    #[tokio::test]
    async fn export_to_dir_creates_files() {
        let config = BenchSinkConfig::for_test();
        let mut sink = BenchSink::new(config);
        sink.init().await.unwrap();

        let before_measurement_ns = current_time_ns();
        for seq in 0..20 {
            let env = make_bench_envelope(seq);
            sink.collect(env).await.unwrap();
        }
        sink.close().await.unwrap();

        let tmp_dir = std::env::temp_dir().join(format!("wafer-bench-test-{}", std::process::id()));
        // Clean up from prior runs
        let _ = std::fs::remove_dir_all(&tmp_dir);

        sink.export_to_dir(&tmp_dir).unwrap();

        // Verify files exist and have content
        let hdr_path = tmp_dir.join("latency.hdr");
        let csv_path = tmp_dir.join("throughput.csv");
        let window_path = tmp_dir.join("measurement-window.json");

        assert!(hdr_path.exists(), "latency.hdr not created");
        assert!(csv_path.exists(), "throughput.csv not created");
        assert!(window_path.exists(), "measurement-window.json not created");

        let window: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(window_path).unwrap()).unwrap();
        let started_ns = window["started_ns"].as_u64().unwrap();
        let finished_ns = window["finished_ns"].as_u64().unwrap();
        assert!(started_ns >= before_measurement_ns);
        assert!(finished_ns > started_ns);
        assert!(finished_ns <= current_time_ns());

        let hdr_content = std::fs::read_to_string(&hdr_path).unwrap();
        assert!(hdr_content.contains("#[StartTime"));
        assert!(hdr_content.contains("Recorded values: 20"));

        let csv_content = std::fs::read_to_string(&csv_path).unwrap();
        assert!(csv_content.starts_with("elapsed_secs,msg_count,bytes\n"));
        // Should have at least the final flush bucket
        assert!(csv_content.lines().count() >= 2); // header + at least 1 data row

        // Cleanup
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[tokio::test]
    async fn auto_export_on_close_with_output_dir() {
        let tmp_dir =
            std::env::temp_dir().join(format!("wafer-bench-autoexport-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp_dir);

        let config = BenchSinkConfig::for_test().with_output_dir(&tmp_dir);
        let mut sink = BenchSink::new(config);
        sink.init().await.unwrap();

        for seq in 0..15 {
            let env = make_bench_envelope(seq);
            sink.collect(env).await.unwrap();
        }

        // close() should automatically export
        sink.close().await.unwrap();

        assert!(tmp_dir.join("latency.hdr").exists(), "auto-export failed: latency.hdr missing");
        assert!(
            tmp_dir.join("throughput.csv").exists(),
            "auto-export failed: throughput.csv missing"
        );
        assert!(
            tmp_dir.join("measurement-window.json").exists(),
            "auto-export failed: measurement-window.json missing"
        );

        let hdr = std::fs::read_to_string(tmp_dir.join("latency.hdr")).unwrap();
        assert!(hdr.contains("Recorded values: 15"));

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[tokio::test]
    async fn throughput_csv_format_correctness() {
        let config = BenchSinkConfig::for_test();
        let mut sink = BenchSink::new(config);
        sink.init().await.unwrap();

        for seq in 0..5 {
            let env = make_bench_envelope(seq);
            sink.collect(env).await.unwrap();
        }
        sink.close().await.unwrap();

        let csv = sink.throughput_csv();
        let lines: Vec<&str> = csv.lines().collect();

        // Header
        assert_eq!(lines[0], "elapsed_secs,msg_count,bytes");

        // Data rows are parseable
        for line in &lines[1..] {
            let parts: Vec<&str> = line.split(',').collect();
            assert_eq!(parts.len(), 3, "CSV row should have 3 columns: {line}");
            parts[0].parse::<f64>().expect("elapsed_secs should be f64");
            parts[1].parse::<u64>().expect("msg_count should be u64");
            parts[2].parse::<u64>().expect("bytes should be u64");
        }
    }
}
