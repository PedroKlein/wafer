//! Measurement recorder: [`HdrHistogram`] + sequence tracker + artifact writer.
//!
//! Records end-to-end latency for MQTT messages emitted by `wafer-loadgen publish`
//! (the JSON payload embeds `ts` = intended-publish-time in nanoseconds, `seq` =
//! monotonic sequence). The subscriber (`sub.rs`) drives this recorder for every
//! received message and flushes three artifacts on graceful exit:
//!
//! - `latency.hdr` — raw [`HdrHistogram`] V2-serialised bytes. The first three
//!   bytes are the V2 cookie prefix `1c 84 93` (the low byte encodes counter
//!   word-size and varies across runs). `xxd latency.hdr | head -1` shows
//!   the cookie at file offset 0.
//! - `sequence.csv` — one row per gap or duplicate (long form, ADF/pandas ready).
//! - `subscriber-metadata.json` — broker/topic identity + percentile snapshot +
//!   git SHA + exit reason.
//!
//! The latency reference is the payload's `ts` field, NOT the subscriber's receive
//! time. This is deliberate: sampling receive-time-vs-receive-time would erase
//! coordinated omission (Gil Tene 2012). If the pipeline pauses, the publisher's
//! `ts` continues to advance at wall-clock rate; the recorded latency reflects
//! the true queueing delay.
//!
//! See docs/rfcs/RFC-008-evaluation-harness.md — Session 8 D4 / D9.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use hdrhistogram::Histogram;
use hdrhistogram::serialization::{Serializer, V2Serializer};
use serde::{Deserialize, Serialize};

pub(crate) const EVENT_BUCKET_WIDTH_NS: u64 = 100_000_000;
pub(crate) const EVENT_BUCKET_COUNT: usize = 200;
pub(crate) const EVENT_COVERAGE_START_NS: i64 = -10_000_000_000;
pub(crate) const EVENT_COVERAGE_END_NS: i64 = 10_000_000_000;
pub(crate) const EVENT_ALIGNMENT_TOLERANCE_NS: i64 = 10_000_000;
const EVENT_SAMPLE_CAPACITY: usize = 65_536;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PublisherTimingReceipt {
    pub schema_version: u8,
    pub measurement_clock: String,
    pub alignment_clock: String,
    pub measurement_started_unix_epoch_ns: u64,
    pub event_unix_epoch_ns: u64,
    pub event_offset_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ActionTimingReceipt {
    pub schema_version: u8,
    pub strategy: String,
    pub timestamp_clock: String,
    pub timestamp_clock_purpose: String,
    pub scheduling_clock: String,
    pub duration_clock: String,
    pub measurement_start_timestamp_ns: u64,
    pub scheduled_event_offset_ns: u64,
    pub scheduled_event_timestamp_ns: u64,
    pub event_timestamp_ns: u64,
    pub event_offset_from_measurement_start_ns: u64,
    pub alignment_error_ns: i64,
    pub alignment_tolerance_ns: i64,
    pub action_start_timestamp_ns: u64,
    pub action_end_timestamp_ns: u64,
    pub action_end_offset_ns: u64,
    pub action_start_monotonic_ns: u64,
    pub action_end_monotonic_ns: u64,
    pub action_duration_ns: u64,
}

#[derive(Debug, Clone, Copy)]
struct EventSample {
    receive_unix_epoch_ns: u64,
    duplicate: bool,
}

#[derive(Debug, Clone, Serialize)]
struct ThroughputBucket {
    start_offset_ns: i64,
    end_offset_ns: i64,
    received_unique: u64,
    received_events: u64,
    duplicates: u64,
    rate_msg_s: f64,
}

pub(crate) struct EventBucketRecorder {
    measurement_start_timestamp_ns: u64,
    scheduled_event_timestamp_ns: u64,
    samples: Vec<EventSample>,
}

impl EventBucketRecorder {
    pub(crate) fn new(timing: &PublisherTimingReceipt) -> anyhow::Result<Self> {
        if timing.schema_version != 1
            || timing.measurement_clock != "monotonic"
            || timing.alignment_clock != "unix-epoch"
            || timing.event_offset_ns != 60_000_000_000
            || timing.event_unix_epoch_ns
                != timing.measurement_started_unix_epoch_ns.saturating_add(timing.event_offset_ns)
        {
            anyhow::bail!("publisher timing receipt does not declare the frozen t=60 boundary");
        }
        Ok(Self {
            measurement_start_timestamp_ns: timing.measurement_started_unix_epoch_ns,
            scheduled_event_timestamp_ns: timing.event_unix_epoch_ns,
            samples: Vec::with_capacity(EVENT_SAMPLE_CAPACITY),
        })
    }

    pub(crate) fn record(
        &mut self,
        receive_unix_epoch_ns: u64,
        duplicate: bool,
    ) -> anyhow::Result<()> {
        let capture_margin_ns = EVENT_ALIGNMENT_TOLERANCE_NS.unsigned_abs();
        let capture_start = self
            .scheduled_event_timestamp_ns
            .saturating_sub(EVENT_COVERAGE_START_NS.unsigned_abs())
            .saturating_sub(capture_margin_ns);
        let capture_end = self
            .scheduled_event_timestamp_ns
            .saturating_add(EVENT_COVERAGE_END_NS.unsigned_abs())
            .saturating_add(capture_margin_ns);
        if !(capture_start..capture_end).contains(&receive_unix_epoch_ns) {
            return Ok(());
        }
        if self.samples.len() == EVENT_SAMPLE_CAPACITY {
            anyhow::bail!(
                "E-Swap-3 bounded event sample capacity exceeded ({EVENT_SAMPLE_CAPACITY})"
            );
        }
        self.samples.push(EventSample { receive_unix_epoch_ns, duplicate });
        Ok(())
    }

    pub(crate) fn write(&self, path: &Path, action: &ActionTimingReceipt) -> anyhow::Result<()> {
        self.validate_action_timing(action)?;
        let mut received_unique = [0_u64; EVENT_BUCKET_COUNT];
        let mut received_events = [0_u64; EVENT_BUCKET_COUNT];
        let mut duplicates = [0_u64; EVENT_BUCKET_COUNT];
        for sample in &self.samples {
            let offset = signed_offset(sample.receive_unix_epoch_ns, action.event_timestamp_ns);
            if !(EVENT_COVERAGE_START_NS..EVENT_COVERAGE_END_NS).contains(&offset) {
                continue;
            }
            let shifted = offset.saturating_sub(EVENT_COVERAGE_START_NS);
            let index = usize::try_from(
                shifted
                    .checked_div(i64::try_from(EVENT_BUCKET_WIDTH_NS).unwrap_or(i64::MAX))
                    .unwrap_or(i64::MAX),
            )
            .unwrap_or(EVENT_BUCKET_COUNT);
            let (Some(events), Some(unique), Some(duplicate_count)) = (
                received_events.get_mut(index),
                received_unique.get_mut(index),
                duplicates.get_mut(index),
            ) else {
                continue;
            };
            *events = events.saturating_add(1);
            if sample.duplicate {
                *duplicate_count = duplicate_count.saturating_add(1);
            } else {
                *unique = unique.saturating_add(1);
            }
        }
        let buckets = received_unique
            .iter()
            .zip(&received_events)
            .zip(&duplicates)
            .enumerate()
            .map(|(index, ((received_unique, received_events), duplicates))| {
                let start_offset_ns = EVENT_COVERAGE_START_NS.saturating_add(
                    i64::try_from(index)
                        .unwrap_or(i64::MAX)
                        .saturating_mul(i64::try_from(EVENT_BUCKET_WIDTH_NS).unwrap_or(i64::MAX)),
                );
                ThroughputBucket {
                    start_offset_ns,
                    end_offset_ns: start_offset_ns
                        .saturating_add(i64::try_from(EVENT_BUCKET_WIDTH_NS).unwrap_or(i64::MAX)),
                    received_unique: *received_unique,
                    received_events: *received_events,
                    duplicates: *duplicates,
                    rate_msg_s: f64::from(u32::try_from(*received_unique).unwrap_or(u32::MAX))
                        * 10.0,
                }
            })
            .collect::<Vec<_>>();
        let artifact = serde_json::json!({
            "schema_version": 1,
            "clock": "unix-epoch",
            "clock_purpose": "cross-process-alignment",
            "measurement_start_timestamp_ns": self.measurement_start_timestamp_ns,
            "scheduled_event_timestamp_ns": self.scheduled_event_timestamp_ns,
            "scheduled_event_offset_ns": action.scheduled_event_offset_ns,
            "event_timestamp_ns": action.event_timestamp_ns,
            "event_offset_from_measurement_start_ns": action.event_offset_from_measurement_start_ns,
            "alignment_error_ns": action.alignment_error_ns,
            "alignment_tolerance_ns": action.alignment_tolerance_ns,
            "bucket_width_ns": EVENT_BUCKET_WIDTH_NS,
            "coverage_start_offset_ns": EVENT_COVERAGE_START_NS,
            "coverage_end_offset_ns": EVENT_COVERAGE_END_NS,
            "received_unique": received_unique.iter().sum::<u64>(),
            "received_events": received_events.iter().sum::<u64>(),
            "duplicates": duplicates.iter().sum::<u64>(),
            "buckets": buckets,
        });
        fs::write(path, format!("{}\n", serde_json::to_string_pretty(&artifact)?))?;
        Ok(())
    }

    fn validate_action_timing(&self, action: &ActionTimingReceipt) -> anyhow::Result<()> {
        let expected_offset = action
            .event_timestamp_ns
            .checked_sub(self.measurement_start_timestamp_ns)
            .ok_or_else(|| anyhow::anyhow!("E-Swap-3 event precedes measurement start"))?;
        let expected_error =
            signed_offset(action.event_timestamp_ns, self.scheduled_event_timestamp_ns);
        if action.schema_version != 1
            || action.timestamp_clock != "unix-epoch"
            || action.timestamp_clock_purpose != "cross-process-alignment"
            || action.scheduling_clock != "monotonic"
            || action.duration_clock != "monotonic"
            || action.measurement_start_timestamp_ns != self.measurement_start_timestamp_ns
            || action.scheduled_event_offset_ns != 60_000_000_000
            || action.scheduled_event_timestamp_ns != self.scheduled_event_timestamp_ns
            || action.action_start_timestamp_ns != action.event_timestamp_ns
            || action.event_offset_from_measurement_start_ns != expected_offset
            || action.alignment_error_ns != expected_error
            || action.alignment_tolerance_ns != EVENT_ALIGNMENT_TOLERANCE_NS
            || action.alignment_error_ns.unsigned_abs()
                > EVENT_ALIGNMENT_TOLERANCE_NS.unsigned_abs()
            || action.action_end_timestamp_ns < action.action_start_timestamp_ns
            || action.action_end_offset_ns
                != action.action_end_timestamp_ns.saturating_sub(action.action_start_timestamp_ns)
            || action.action_end_monotonic_ns < action.action_start_monotonic_ns
            || action.action_duration_ns
                != action.action_end_monotonic_ns.saturating_sub(action.action_start_monotonic_ns)
        {
            anyhow::bail!("action timing receipt violates the frozen E-Swap-3 boundary");
        }
        Ok(())
    }
}

fn signed_offset(timestamp_ns: u64, reference_ns: u64) -> i64 {
    if timestamp_ns >= reference_ns {
        i64::try_from(timestamp_ns.saturating_sub(reference_ns)).unwrap_or(i64::MAX)
    } else {
        i64::try_from(reference_ns.saturating_sub(timestamp_ns))
            .unwrap_or(i64::MAX)
            .checked_neg()
            .unwrap_or(i64::MIN)
    }
}

/// Result of a single `LatencyRecorder::record_json` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordOutcome {
    /// Message parsed and its latency+sequence were recorded.
    Recorded { latency_ns: u64, seq: u64 },
    /// Payload could not be parsed as JSON with `ts` and `seq` fields.
    ParseError,
    /// `ts` is in the future (clock skew or non-monotonic publisher). Not recorded.
    NegativeLatency,
    /// Message belongs to an excluded sequence range, such as warmup traffic.
    IgnoredSequence { seq: u64 },
}

/// Tracks message sequence numbers to detect gaps (lost messages) and duplicates.
///
/// Standalone copy of `wafer_core::node::sink::SequenceTracker` — duplicated so
/// `wafer-loadgen` does not need to depend on the wasmtime-heavy runtime crate.
#[derive(Debug, Default)]
pub struct SequenceTracker {
    expected_next: u64,
    gaps: Vec<(u64, u64)>,
    duplicates: Vec<u64>,
    total_received: u64,
    total_gaps: u64,
    total_duplicates: u64,
    max_examples: Option<usize>,
    examples_truncated: bool,
}

impl SequenceTracker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub(crate) fn with_max_examples(max_examples: usize) -> Self {
        Self { max_examples: Some(max_examples), ..Self::default() }
    }

    fn can_store_example(&self, count: usize) -> bool {
        self.max_examples.is_none_or(|limit| count < limit)
    }

    /// Record a received sequence number.
    pub fn record(&mut self, seq: u64) -> bool {
        self.total_received = self.total_received.saturating_add(1);
        match seq.cmp(&self.expected_next) {
            std::cmp::Ordering::Equal => {
                self.expected_next = self.expected_next.saturating_add(1);
                false
            }
            std::cmp::Ordering::Greater => {
                // seq > expected_next, so subtraction cannot underflow
                #[expect(
                    clippy::arithmetic_side_effects,
                    reason = "seq > expected_next checked by match arm; seq - 1 safe because seq > 0 (seq > expected_next >= 0)"
                )]
                {
                    self.total_gaps =
                        self.total_gaps.saturating_add(seq.saturating_sub(self.expected_next));
                    if self.can_store_example(self.gaps.len()) {
                        self.gaps.push((self.expected_next, seq - 1));
                    } else {
                        self.examples_truncated = true;
                    }
                    self.expected_next = seq + 1;
                }
                false
            }
            std::cmp::Ordering::Less => {
                self.total_duplicates = self.total_duplicates.saturating_add(1);
                if self.can_store_example(self.duplicates.len()) {
                    self.duplicates.push(seq);
                } else {
                    self.examples_truncated = true;
                }
                true
            }
        }
    }

    #[must_use]
    pub const fn total_gaps(&self) -> u64 {
        self.total_gaps
    }

    #[must_use]
    pub const fn total_duplicates(&self) -> u64 {
        self.total_duplicates
    }

    #[must_use]
    pub const fn total_received(&self) -> u64 {
        self.total_received
    }

    #[must_use]
    pub fn gaps(&self) -> &[(u64, u64)] {
        &self.gaps
    }

    #[must_use]
    pub fn duplicates(&self) -> &[u64] {
        &self.duplicates
    }
}

/// Snapshot of a completed subscriber run for post-hoc analysis.
///
/// Sequence-tracker report exposed via `SubscriberMetadata` for the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequenceReport {
    pub total_received: u64,
    pub total_gaps: u64,
    pub total_duplicates: u64,
    pub gap_ranges: Vec<(u64, u64)>,
    pub duplicate_seqs: Vec<u64>,
    #[serde(default)]
    pub examples_truncated: bool,
}

impl From<&SequenceTracker> for SequenceReport {
    fn from(t: &SequenceTracker) -> Self {
        Self {
            total_received: t.total_received(),
            total_gaps: t.total_gaps(),
            total_duplicates: t.total_duplicates(),
            gap_ranges: t.gaps().to_vec(),
            duplicate_seqs: t.duplicates().to_vec(),
            examples_truncated: t.examples_truncated,
        }
    }
}

/// Manifest written alongside `latency.hdr` and `sequence.csv` on exit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriberMetadata {
    /// Broker connection string (host:port), for reproducibility.
    pub broker: String,
    pub topic: String,
    /// Wall-clock start (ns since UNIX epoch).
    pub started_at_ns: u64,
    pub ended_at_ns: u64,
    /// Why the subscriber stopped.
    pub exit_reason: String,
    /// Git SHA of the wafer tree that built this binary, if the environment
    /// injected it (via `WAFER_GIT_SHA` at run time or build-time `env!`).
    pub git_sha: Option<String>,
    /// Host tag as passed by the eval scripts (P1.1 result contract).
    pub host_tag: Option<String>,
    /// Exclusive upper sequence bound for the measured population.
    #[serde(default)]
    pub sequence_end_exclusive: Option<u64>,
    /// Parsed warmup messages excluded because they were outside the measured range.
    #[serde(default)]
    pub ignored_sequence_count: u64,
    /// Parsed measured messages outside the publisher's declared sequence range.
    #[serde(default)]
    pub unexpected_sequence_count: u64,

    // --- Measurement summary (also fully preserved in latency.hdr) ---
    pub total_recorded: u64,
    pub total_messages: u64,
    pub parse_errors: u64,
    pub negative_latency_count: u64,
    pub latency_min_ns: u64,
    pub latency_max_ns: u64,
    pub latency_mean_ns: f64,
    pub latency_p50_ns: u64,
    pub latency_p95_ns: u64,
    pub latency_p99_ns: u64,
    pub latency_p999_ns: u64,
    pub histogram_lowest_ns: u64,
    pub histogram_highest_ns: u64,
    pub histogram_sig_digits: u8,

    // --- Sequence report ---
    pub sequence: SequenceReport,
}

/// End-to-end latency recorder for the `wafer-loadgen subscribe` command.
///
/// Bounded `1_000` ns (1 µs) → `10_000_000_000` ns (10 s) at 3 significant digits.
/// This is the same envelope as `BenchSink` for cross-experiment comparability.
pub struct LatencyRecorder {
    histogram: Histogram<u64>,
    sequence: SequenceTracker,
    total_messages: u64,
    parse_errors: u64,
    negative_latency: u64,
    ignored_sequences: u64,
    last_record_duplicate: bool,
}

impl LatencyRecorder {
    /// Range: 1 µs to 10 s, 3 significant digits.
    pub const LOWEST_NS: u64 = 1_000;
    pub const HIGHEST_NS: u64 = 10_000_000_000;
    pub const SIG_DIGITS: u8 = 3;

    #[must_use]
    pub fn new() -> Self {
        Self::with_sequence_example_limit(None)
    }

    /// # Panics
    /// Never in practice. The histogram parameters are compile-time constants.
    #[must_use]
    #[expect(
        clippy::expect_used,
        reason = "HdrHistogram bounds are compile-time constants proven valid by unit tests"
    )]
    pub(crate) fn with_sequence_example_limit(max_examples: Option<usize>) -> Self {
        let histogram =
            Histogram::<u64>::new_with_bounds(Self::LOWEST_NS, Self::HIGHEST_NS, Self::SIG_DIGITS)
                .expect("HdrHistogram bounds are compile-time constants and known valid");
        Self {
            histogram,
            sequence: max_examples
                .map_or_else(SequenceTracker::new, SequenceTracker::with_max_examples),
            total_messages: 0,
            parse_errors: 0,
            negative_latency: 0,
            ignored_sequences: 0,
            last_record_duplicate: false,
        }
    }

    /// Parse a JSON payload (with `ts` = intended-publish-ns and `seq` = u64)
    /// and record the observed latency against `receive_ns`.
    pub fn record_json(&mut self, payload: &[u8], receive_ns: u64) -> RecordOutcome {
        self.record_json_with_sequence_end(payload, receive_ns, None)
    }

    /// Record only messages whose sequence is below `sequence_end_exclusive`.
    pub(crate) fn record_json_before(
        &mut self,
        payload: &[u8],
        receive_ns: u64,
        sequence_end_exclusive: u64,
    ) -> RecordOutcome {
        self.record_json_with_sequence_end(payload, receive_ns, Some(sequence_end_exclusive))
    }

    fn record_json_with_sequence_end(
        &mut self,
        payload: &[u8],
        receive_ns: u64,
        sequence_end_exclusive: Option<u64>,
    ) -> RecordOutcome {
        let parsed: serde_json::Result<serde_json::Value> = serde_json::from_slice(payload);
        let Ok(json) = parsed else {
            self.total_messages = self.total_messages.saturating_add(1);
            self.parse_errors = self.parse_errors.saturating_add(1);
            return RecordOutcome::ParseError;
        };
        let ts = json.get("ts").and_then(serde_json::Value::as_u64);
        let seq = json.get("seq").and_then(serde_json::Value::as_u64);
        let (Some(ts), Some(seq)) = (ts, seq) else {
            self.total_messages = self.total_messages.saturating_add(1);
            self.parse_errors = self.parse_errors.saturating_add(1);
            return RecordOutcome::ParseError;
        };
        if sequence_end_exclusive.is_some_and(|end| seq >= end) {
            self.ignored_sequences = self.ignored_sequences.saturating_add(1);
            return RecordOutcome::IgnoredSequence { seq };
        }
        self.total_messages = self.total_messages.saturating_add(1);
        self.record(ts, receive_ns, seq)
    }

    /// Record directly from timestamps. Public so tests can inject deterministic
    /// latencies without JSON round-tripping.
    pub fn record(&mut self, intended_publish_ns: u64, receive_ns: u64, seq: u64) -> RecordOutcome {
        if receive_ns < intended_publish_ns {
            self.negative_latency = self.negative_latency.saturating_add(1);
            return RecordOutcome::NegativeLatency;
        }
        // receive_ns >= intended_publish_ns checked above
        #[expect(
            clippy::arithmetic_side_effects,
            reason = "subtraction safe: receive_ns >= intended_publish_ns guarded by the if-check above"
        )]
        let latency_ns = receive_ns - intended_publish_ns;
        // Clamp below-histogram-floor values to the floor rather than dropping.
        // Sub-microsecond latencies are physically impossible over MQTT, but a
        // clock-skew payload could show one. Clamping preserves the count.
        let recorded = latency_ns.clamp(Self::LOWEST_NS, Self::HIGHEST_NS);
        // hdrhistogram returns Err only if the value is above the ceiling; we
        // already clamped so this can only fail on internal invariants.
        let _r = self.histogram.record(recorded);
        self.last_record_duplicate = self.sequence.record(seq);
        RecordOutcome::Recorded { latency_ns, seq }
    }

    #[must_use]
    pub fn total_recorded(&self) -> u64 {
        self.histogram.len()
    }

    #[must_use]
    pub const fn total_messages(&self) -> u64 {
        self.total_messages
    }

    #[must_use]
    pub const fn parse_errors(&self) -> u64 {
        self.parse_errors
    }

    #[must_use]
    pub(crate) const fn ignored_sequences(&self) -> u64 {
        self.ignored_sequences
    }

    pub(crate) const fn last_record_duplicate(&self) -> bool {
        self.last_record_duplicate
    }

    #[must_use]
    pub const fn negative_latency(&self) -> u64 {
        self.negative_latency
    }

    #[must_use]
    pub fn p50_ns(&self) -> u64 {
        self.histogram.value_at_quantile(0.50)
    }
    #[must_use]
    pub fn p95_ns(&self) -> u64 {
        self.histogram.value_at_quantile(0.95)
    }
    #[must_use]
    pub fn p99_ns(&self) -> u64 {
        self.histogram.value_at_quantile(0.99)
    }
    #[must_use]
    pub fn p999_ns(&self) -> u64 {
        self.histogram.value_at_quantile(0.999)
    }
    #[must_use]
    pub fn min_ns(&self) -> u64 {
        self.histogram.min()
    }
    #[must_use]
    pub fn max_ns(&self) -> u64 {
        self.histogram.max()
    }
    #[must_use]
    pub fn mean_ns(&self) -> f64 {
        self.histogram.mean()
    }

    #[must_use]
    pub const fn sequence(&self) -> &SequenceTracker {
        &self.sequence
    }

    /// Serialize the histogram to raw V2 bytes.
    ///
    /// The first three bytes are the [`HdrHistogram`] V2 cookie prefix
    /// `1c 84 93` (the fourth byte encodes the counter word-size chosen by
    /// hdrhistogram-rs and varies across runs). `xxd` will show the cookie at
    /// offset 0. Analysis notebooks decode this via
    /// `hdrhistogram::serialization::Deserializer` or the Python
    /// `hdrh.histogram.HdrHistogram.decode` API.
    ///
    /// # Errors
    /// Returns an error if the V2 serializer fails (in practice: never, unless
    /// the buffer runs out of memory).
    pub fn serialize_v2(&self) -> Result<Vec<u8>, hdrhistogram::serialization::V2SerializeError> {
        let mut buf = Vec::with_capacity(1024);
        let mut serializer = V2Serializer::new();
        serializer.serialize(&self.histogram, &mut buf)?;
        Ok(buf)
    }

    /// Emit the per-event long-form CSV for gap/duplicate analysis.
    ///
    /// Columns: `event_type,seq_start,seq_end,count`. If no gaps or duplicates
    /// were observed, only the header row is emitted (still ADF/pandas-parseable).
    #[must_use]
    pub fn sequence_csv(&self) -> String {
        use std::fmt::Write as _;
        let mut csv = String::from("event_type,seq_start,seq_end,count\n");
        for (start, end) in self.sequence.gaps() {
            let count = end.saturating_sub(*start).saturating_add(1);
            // writeln! into a String is infallible
            #[expect(clippy::unwrap_used, reason = "fmt::Write for String cannot fail")]
            writeln!(csv, "gap,{start},{end},{count}").unwrap();
        }
        for seq in self.sequence.duplicates() {
            #[expect(clippy::unwrap_used, reason = "fmt::Write for String cannot fail")]
            writeln!(csv, "duplicate,{seq},{seq},1").unwrap();
        }
        csv
    }

    /// Write `latency.hdr`, `sequence.csv`, and `subscriber-metadata.json` to `dir`.
    ///
    /// # Errors
    /// - Directory creation failure.
    /// - Any file write failure.
    /// - [`HdrHistogram`] V2 serialization failure (unreachable in practice).
    pub fn write_artifacts(
        &self,
        dir: &Path,
        mut metadata: SubscriberMetadata,
    ) -> anyhow::Result<()> {
        fs::create_dir_all(dir)?;

        // Fill measurement-derived fields.
        metadata.total_recorded = self.total_recorded();
        metadata.total_messages = self.total_messages;
        metadata.parse_errors = self.parse_errors;
        metadata.negative_latency_count = self.negative_latency;
        metadata.ignored_sequence_count = self.ignored_sequences;
        metadata.latency_min_ns = self.min_ns();
        metadata.latency_max_ns = self.max_ns();
        metadata.latency_mean_ns = self.mean_ns();
        metadata.latency_p50_ns = self.p50_ns();
        metadata.latency_p95_ns = self.p95_ns();
        metadata.latency_p99_ns = self.p99_ns();
        metadata.latency_p999_ns = self.p999_ns();
        metadata.histogram_lowest_ns = Self::LOWEST_NS;
        metadata.histogram_highest_ns = Self::HIGHEST_NS;
        metadata.histogram_sig_digits = Self::SIG_DIGITS;
        metadata.sequence = SequenceReport::from(&self.sequence);

        // latency.hdr — raw V2 bytes.
        let hdr_bytes =
            self.serialize_v2().map_err(|e| anyhow::anyhow!("hdr V2 serialize: {e:?}"))?;
        let mut hdr_file = fs::File::create(dir.join("latency.hdr"))?;
        hdr_file.write_all(&hdr_bytes)?;

        // sequence.csv
        let mut csv_file = fs::File::create(dir.join("sequence.csv"))?;
        csv_file.write_all(self.sequence_csv().as_bytes())?;

        // subscriber-metadata.json
        let json = serde_json::to_string_pretty(&metadata)?;
        let mut meta_file = fs::File::create(dir.join("subscriber-metadata.json"))?;
        meta_file.write_all(json.as_bytes())?;
        meta_file.write_all(b"\n")?;

        Ok(())
    }
}

impl Default for LatencyRecorder {
    fn default() -> Self {
        Self::new()
    }
}

/// Wall-clock time in ns since UNIX epoch. Returned by `SystemTime::now()` on
/// every OS we target; falls back to 0 on the theoretical pre-1970 clock case.
#[must_use]
pub fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

// =============================================================================
// Unit tests — TDD anchor for the P0.1 acceptance criteria.
// =============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    /// AC1: 10 ms injected latency shows up in the p99 percentile within
    /// [`HdrHistogram`] quantization tolerance (0.1% at 3 sig digits).
    #[test]
    fn record_ten_ms_latency_lands_in_p99_within_hdrhistogram_tolerance() {
        let mut rec = LatencyRecorder::new();
        let intended = 1_000_000_000; // Arbitrary base timestamp.
        let receive = intended + 10_000_000; // +10 ms
        for seq in 0..500 {
            let outcome = rec.record(intended, receive, seq);
            assert!(matches!(outcome, RecordOutcome::Recorded { .. }));
        }

        // 3 sig digits @ 10 ms → bucket ~10 µs wide. Allow 0.5% band = 50 µs.
        let target = 10_000_000_u64;
        let tol = 50_000_u64;
        assert!(
            rec.p99_ns().abs_diff(target) <= tol,
            "p99 {} not within ±{tol} of {target}",
            rec.p99_ns()
        );
        assert!(rec.p50_ns().abs_diff(target) <= tol);
        assert_eq!(rec.total_recorded(), 500);
    }

    #[test]
    fn record_json_parses_publisher_payload_shape() {
        let intended = 42_000_000_000_u64;
        let receive = intended + 250_000; // 250 µs
        let payload = format!(
            r#"{{"ts":{intended},"seq":7,"device_id":"bench","temperature":72.5,"pad":"xxx"}}"#
        );
        let mut rec = LatencyRecorder::new();
        let outcome = rec.record_json(payload.as_bytes(), receive);
        assert_eq!(outcome, RecordOutcome::Recorded { latency_ns: 250_000, seq: 7 });
        assert_eq!(rec.total_recorded(), 1);
        assert_eq!(rec.parse_errors(), 0);
    }

    #[test]
    fn record_json_before_sequence_ignores_delayed_warmup_messages() {
        let intended = 42_000_000_000_u64;
        let receive = intended + 250_000;
        let stale = format!(r#"{{"ts":{intended},"seq":60000}}"#);
        let measured = format!(r#"{{"ts":{intended},"seq":0}}"#);
        let mut rec = LatencyRecorder::new();

        assert_eq!(
            rec.record_json_before(stale.as_bytes(), receive, 60_000),
            RecordOutcome::IgnoredSequence { seq: 60_000 }
        );
        assert_eq!(
            rec.record_json_before(measured.as_bytes(), receive, 60_000),
            RecordOutcome::Recorded { latency_ns: 250_000, seq: 0 }
        );
        assert_eq!(rec.ignored_sequences(), 1);
        assert_eq!(rec.total_messages(), 1);
        assert_eq!(rec.total_recorded(), 1);
        assert_eq!(rec.sequence().total_gaps(), 0);
    }

    #[test]
    fn record_json_flags_malformed_input() {
        let mut rec = LatencyRecorder::new();
        assert_eq!(rec.record_json(b"not-json", 1), RecordOutcome::ParseError);
        assert_eq!(rec.record_json(b"{\"ts\":123}", 1), RecordOutcome::ParseError); // no seq
        assert_eq!(rec.record_json(b"{\"seq\":1}", 1), RecordOutcome::ParseError); // no ts
        assert_eq!(rec.parse_errors(), 3);
        assert_eq!(rec.total_recorded(), 0);
    }

    #[test]
    fn record_negative_latency_is_flagged_not_recorded() {
        let mut rec = LatencyRecorder::new();
        let outcome = rec.record(2_000, 1_000, 0);
        assert_eq!(outcome, RecordOutcome::NegativeLatency);
        assert_eq!(rec.total_recorded(), 0);
        assert_eq!(rec.negative_latency(), 1);
    }

    #[test]
    fn record_clamps_sub_microsecond_to_floor() {
        let mut rec = LatencyRecorder::new();
        let outcome = rec.record(1_000_000_000, 1_000_000_500, 0); // 500 ns
        // Outcome reports the RAW latency (500 ns) — clamping is a storage
        // detail. Both facts matter: caller sees truth, histogram stays bounded.
        assert!(matches!(outcome, RecordOutcome::Recorded { latency_ns: 500, .. }));
        assert_eq!(rec.total_recorded(), 1, "sub-microsecond values must not be dropped");
        assert_eq!(rec.negative_latency(), 0);
    }

    #[test]
    fn sequence_tracker_detects_gap_and_duplicate() {
        let mut t = SequenceTracker::new();
        for s in [0_u64, 1, 2, 5, 5, 6] {
            t.record(s);
        }
        assert_eq!(t.total_received(), 6);
        assert_eq!(t.total_gaps(), 2); // 3 and 4
        assert_eq!(t.gaps(), &[(3, 4)]);
        assert_eq!(t.total_duplicates(), 1); // second 5
        assert_eq!(t.duplicates(), &[5]);
    }

    /// AC2: `latency.hdr` begins with the [`HdrHistogram`] V2 cookie prefix. The
    /// V2 cookie's low byte encodes counter word-size (hdrhistogram-rs picks
    /// the smallest counter that fits), so only the upper 3 bytes are
    /// stable — but those 3 bytes are what makes `xxd latency.hdr | head -1`
    /// visually identifiable as [`HdrHistogram`] output.
    #[test]
    fn serialize_v2_starts_with_hdr_magic_cookie() {
        let mut rec = LatencyRecorder::new();
        for i in 0..100 {
            rec.record(0, 10_000_000, i);
        }
        let bytes = rec.serialize_v2().unwrap();
        assert_eq!(
            &bytes[..3],
            &[0x1c, 0x84, 0x93],
            "expected HdrHistogram V2 cookie prefix, got {:02x?}",
            &bytes[..8.min(bytes.len())]
        );
    }

    #[test]
    fn sequence_examples_are_bounded_without_losing_totals() {
        let mut tracker = SequenceTracker::with_max_examples(1_024);
        for seq in (1..=2_050).step_by(2) {
            tracker.record(seq);
            tracker.record(seq);
        }
        let report = SequenceReport::from(&tracker);
        assert_eq!(report.total_gaps, 1_025);
        assert_eq!(report.total_duplicates, 1_025);
        assert_eq!(report.gap_ranges.len(), 1_024);
        assert_eq!(report.duplicate_seqs.len(), 1_024);
        assert!(report.examples_truncated);
    }

    #[test]
    fn sequence_csv_shape_is_adf_ready() {
        let mut rec = LatencyRecorder::new();
        for s in [0_u64, 1, 3, 3] {
            rec.record(0, 1_000_000, s);
        }
        let csv = rec.sequence_csv();
        // Header first, then one row per gap and one per duplicate.
        assert!(csv.starts_with("event_type,seq_start,seq_end,count\n"));
        let rows: Vec<&str> = csv.lines().skip(1).collect();
        // Ensure every row has exactly 4 comma-separated columns.
        for row in &rows {
            assert_eq!(row.split(',').count(), 4, "malformed CSV row: {row}");
        }
        // Content assertions on the concrete rows.
        assert!(rows.contains(&"gap,2,2,1"));
        assert!(rows.contains(&"duplicate,3,3,1"));
    }

    fn action_timing(
        timing: &PublisherTimingReceipt,
        alignment_error_ns: i64,
    ) -> ActionTimingReceipt {
        let event_timestamp_ns =
            timing.event_unix_epoch_ns.checked_add_signed(alignment_error_ns).unwrap();
        ActionTimingReceipt {
            schema_version: 1,
            strategy: "wafer-hotswap".into(),
            timestamp_clock: "unix-epoch".into(),
            timestamp_clock_purpose: "cross-process-alignment".into(),
            scheduling_clock: "monotonic".into(),
            duration_clock: "monotonic".into(),
            measurement_start_timestamp_ns: timing.measurement_started_unix_epoch_ns,
            scheduled_event_offset_ns: timing.event_offset_ns,
            scheduled_event_timestamp_ns: timing.event_unix_epoch_ns,
            event_timestamp_ns,
            event_offset_from_measurement_start_ns: event_timestamp_ns
                .saturating_sub(timing.measurement_started_unix_epoch_ns),
            alignment_error_ns,
            alignment_tolerance_ns: EVENT_ALIGNMENT_TOLERANCE_NS,
            action_start_timestamp_ns: event_timestamp_ns,
            action_end_timestamp_ns: event_timestamp_ns.saturating_add(200_000_000),
            action_end_offset_ns: 200_000_000,
            action_start_monotonic_ns: 5_000_000_000,
            action_end_monotonic_ns: 5_199_000_000,
            action_duration_ns: 199_000_000,
        }
    }

    #[test]
    fn event_buckets_are_bounded_contiguous_and_reconcile_duplicates() {
        let timing = PublisherTimingReceipt {
            schema_version: 1,
            measurement_clock: "monotonic".into(),
            alignment_clock: "unix-epoch".into(),
            measurement_started_unix_epoch_ns: 1_000_000_000_000,
            event_unix_epoch_ns: 1_060_000_000_000,
            event_offset_ns: 60_000_000_000,
        };
        let action = action_timing(&timing, 5_000_000);
        let mut buckets = EventBucketRecorder::new(&timing).unwrap();
        buckets.record(action.event_timestamp_ns - 10_000_000_000, false).unwrap();
        buckets.record(action.event_timestamp_ns - 9_950_000_000, true).unwrap();
        buckets.record(action.event_timestamp_ns + 9_999_999_999, false).unwrap();
        buckets.record(action.event_timestamp_ns + 10_000_000_000, false).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("throughput-buckets.json");
        buckets.write(&path, &action).unwrap();

        let artifact: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let rows = artifact["buckets"].as_array().unwrap();
        assert_eq!(rows.len(), 200);
        assert_eq!(artifact["received_events"], 3);
        assert_eq!(artifact["received_unique"], 2);
        assert_eq!(artifact["duplicates"], 1);
        assert_eq!(rows[0]["start_offset_ns"], -10_000_000_000_i64);
        assert_eq!(rows[0]["end_offset_ns"], -9_900_000_000_i64);
        assert_eq!(rows[0]["received_events"], 2);
        assert_eq!(rows[0]["received_unique"], 1);
        assert_eq!(rows[0]["duplicates"], 1);
        assert_eq!(rows[0]["rate_msg_s"], 10.0);
        assert_eq!(rows[199]["received_unique"], 1);
        assert!(fs::metadata(path).unwrap().len() < 64 * 1024);
    }

    #[test]
    #[expect(clippy::print_stderr, reason = "test emits the requested local throughput receipt")]
    fn event_bucket_capture_is_source_bound_and_artifact_bounded() {
        let timing = PublisherTimingReceipt {
            schema_version: 1,
            measurement_clock: "monotonic".into(),
            alignment_clock: "unix-epoch".into(),
            measurement_started_unix_epoch_ns: 1_000_000_000_000,
            event_unix_epoch_ns: 1_060_000_000_000,
            event_offset_ns: 60_000_000_000,
        };
        let action = action_timing(&timing, 0);
        let mut buckets = EventBucketRecorder::new(&timing).unwrap();
        let started = std::time::Instant::now();
        for index in 0..20_000_u64 {
            buckets
                .record(timing.event_unix_epoch_ns - 10_000_000_000 + index * 1_000_000, false)
                .unwrap();
        }
        let elapsed = started.elapsed();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("throughput-buckets.json");
        buckets.write(&path, &action).unwrap();
        let size = fs::metadata(&path).unwrap().len();
        eprintln!(
            "swap3_local_evidence messages=20000 elapsed_ns={} throughput_msg_s={:.0} artifact_bytes={size}",
            elapsed.as_nanos(),
            20_000.0 / elapsed.as_secs_f64()
        );
        assert!(size < 64 * 1024);
    }

    #[test]
    fn event_buckets_are_rebinned_against_actual_action_start() {
        let timing = PublisherTimingReceipt {
            schema_version: 1,
            measurement_clock: "monotonic".into(),
            alignment_clock: "unix-epoch".into(),
            measurement_started_unix_epoch_ns: 1_000_000_000_000,
            event_unix_epoch_ns: 1_060_000_000_000,
            event_offset_ns: 60_000_000_000,
        };
        let action = action_timing(&timing, 5_000_000);
        let mut buckets = EventBucketRecorder::new(&timing).unwrap();
        buckets.record(timing.event_unix_epoch_ns + 1_000_000, false).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("throughput-buckets.json");
        buckets.write(&path, &action).unwrap();
        let artifact: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let rows = artifact["buckets"].as_array().unwrap();
        assert_eq!(rows[99]["received_unique"], 1);
        assert_eq!(rows[100]["received_unique"], 0);
    }

    #[test]
    fn event_bucket_capture_fails_on_fixed_capacity_overflow() {
        let timing = PublisherTimingReceipt {
            schema_version: 1,
            measurement_clock: "monotonic".into(),
            alignment_clock: "unix-epoch".into(),
            measurement_started_unix_epoch_ns: 1_000_000_000_000,
            event_unix_epoch_ns: 1_060_000_000_000,
            event_offset_ns: 60_000_000_000,
        };
        let mut buckets = EventBucketRecorder::new(&timing).unwrap();
        for _ in 0..EVENT_SAMPLE_CAPACITY {
            buckets.record(timing.event_unix_epoch_ns, false).unwrap();
        }
        assert!(buckets.record(timing.event_unix_epoch_ns, false).is_err());
    }

    #[test]
    fn event_buckets_reject_alignment_outside_tolerance() {
        let timing = PublisherTimingReceipt {
            schema_version: 1,
            measurement_clock: "monotonic".into(),
            alignment_clock: "unix-epoch".into(),
            measurement_started_unix_epoch_ns: 1_000_000_000_000,
            event_unix_epoch_ns: 1_060_000_000_000,
            event_offset_ns: 60_000_000_000,
        };
        let buckets = EventBucketRecorder::new(&timing).unwrap();
        let action = action_timing(&timing, EVENT_ALIGNMENT_TOLERANCE_NS + 1);
        let dir = tempfile::tempdir().unwrap();
        assert!(buckets.write(&dir.path().join("throughput-buckets.json"), &action).is_err());
    }

    #[test]
    fn event_buckets_reject_non_frozen_timing_receipt() {
        let timing = PublisherTimingReceipt {
            schema_version: 1,
            measurement_clock: "monotonic".into(),
            alignment_clock: "unix-epoch".into(),
            measurement_started_unix_epoch_ns: 1,
            event_unix_epoch_ns: 2,
            event_offset_ns: 1,
        };
        assert!(EventBucketRecorder::new(&timing).is_err());
    }

    #[test]
    fn write_artifacts_produces_three_expected_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut rec = LatencyRecorder::new();
        for i in 0..50 {
            rec.record(0, 10_000_000, i);
        }
        let meta = SubscriberMetadata {
            broker: "localhost:1883".into(),
            topic: "wafer/test".into(),
            started_at_ns: 1,
            ended_at_ns: 2,
            exit_reason: "total-messages".into(),
            git_sha: Some("deadbeef".into()),
            host_tag: Some("shakedown-macos".into()),
            sequence_end_exclusive: None,
            ignored_sequence_count: 0,
            unexpected_sequence_count: 0,
            total_recorded: 0,
            total_messages: 0,
            parse_errors: 0,
            negative_latency_count: 0,
            latency_min_ns: 0,
            latency_max_ns: 0,
            latency_mean_ns: 0.0,
            latency_p50_ns: 0,
            latency_p95_ns: 0,
            latency_p99_ns: 0,
            latency_p999_ns: 0,
            histogram_lowest_ns: 0,
            histogram_highest_ns: 0,
            histogram_sig_digits: 0,
            sequence: SequenceReport {
                total_received: 0,
                total_gaps: 0,
                total_duplicates: 0,
                gap_ranges: vec![],
                duplicate_seqs: vec![],
                examples_truncated: false,
            },
        };
        rec.write_artifacts(dir.path(), meta).unwrap();

        let hdr = fs::read(dir.path().join("latency.hdr")).unwrap();
        assert_eq!(&hdr[..3], &[0x1c, 0x84, 0x93], "latency.hdr missing V2 cookie");
        let csv = fs::read_to_string(dir.path().join("sequence.csv")).unwrap();
        assert!(csv.starts_with("event_type,seq_start,seq_end,count\n"));
        let json = fs::read_to_string(dir.path().join("subscriber-metadata.json")).unwrap();
        let parsed: SubscriberMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.total_recorded, 50);
        assert_eq!(parsed.sequence.total_received, 50);
        assert_eq!(parsed.sequence.total_gaps, 0);
        assert_eq!(parsed.histogram_sig_digits, 3);
    }
}
