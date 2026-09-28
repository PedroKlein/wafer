//! Measurement recorder: [`HdrHistogram`] + sequence tracker + artifact writer.
//!
//! Records end-to-end latency for MQTT messages emitted by `wafer-loadgen publish`
//! (the JSON payload embeds `ts` = scheduled publish time in nanoseconds, `seq` =
//! monotonic sequence). The subscriber (`sub.rs`) drives this recorder for every
//! received message and flushes aggregate artifacts plus an optional bounded interval fragment on graceful exit:
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
//! coordinated omission (Gil Tene 2012). If the pipeline or the publisher
//! pauses, `ts` keeps following the schedule, so the recorded latency includes
//! the time messages waited to be sent.
//!
//! See docs/rfcs/RFC-008-evaluation-harness.md — Session 8 D4 / D9.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use hdrhistogram::Histogram;
use hdrhistogram::serialization::{Serializer, V2Serializer};
use serde::{Deserialize, Serialize};
use wafer_types::latency::{
    LATENCY_HIGHEST_NS, LATENCY_LOWEST_NS, LATENCY_SIG_DIGITS, LatencyClamps,
};

pub(crate) const EVENT_BUCKET_WIDTH_NS: u64 = 100_000_000;
pub(crate) const EVENT_BUCKET_COUNT: usize = 200;
pub(crate) const EVENT_COVERAGE_START_NS: i64 = -10_000_000_000;
pub(crate) const EVENT_COVERAGE_END_NS: i64 = 10_000_000_000;
pub(crate) const EVENT_ALIGNMENT_TOLERANCE_NS: i64 = 10_000_000;
const EVENT_SAMPLE_CAPACITY: usize = 65_536;
const FINE_EVENT_BUCKET_WIDTH_NS: i64 = 10_000_000;
const FINE_EVENT_BUCKET_COUNT: usize = 400;
const FINE_EVENT_COVERAGE_START_NS: i64 = -2_000_000_000;
const FINE_EVENT_COVERAGE_END_NS: i64 = 2_000_000_000;
const FINE_EVENT_PARENT_BUCKET_WIDTH_NS: i64 = 100_000_000;
const FINE_EVENT_PARENT_BUCKET_COUNT: usize = 40;
const INTERVAL_WIDTH_NS: u64 = 1_000_000_000;

#[derive(Debug, Clone, Serialize)]
struct IntervalLatencyRow {
    interval_start_ns: u64,
    interval_end_ns: u64,
    interval_start_unix_epoch_ns: u64,
    interval_end_unix_epoch_ns: u64,
    latency_count: u64,
    latency_p50_ns: Option<u64>,
    latency_p95_ns: Option<u64>,
    latency_p99_ns: Option<u64>,
    received_events: u64,
    throughput_messages: u64,
    duplicates: u64,
}

struct IntervalRecorder {
    measurement_start_unix_epoch_ns: u64,
    declared_measurement_duration_ns: u64,
    maximum_rows: usize,
    current_bucket: usize,
    current_histogram: Histogram<u64>,
    current_events: u64,
    current_unique: u64,
    current_duplicates: u64,
    rows: Vec<IntervalLatencyRow>,
    late_arrivals: u64,
    finalized: bool,
}

impl IntervalRecorder {
    fn new(measurement_start_unix_epoch_ns: u64, measurement_secs: u64) -> anyhow::Result<Self> {
        if measurement_secs == 0 {
            anyhow::bail!("interval measurement duration must be positive");
        }
        let maximum_rows = usize::try_from(measurement_secs)
            .unwrap_or(usize::MAX)
            .checked_add(2)
            .ok_or_else(|| anyhow::anyhow!("interval maximum row count overflow"))?;
        let histogram =
            Histogram::new_with_bounds(LATENCY_LOWEST_NS, LATENCY_HIGHEST_NS, LATENCY_SIG_DIGITS)?;
        Ok(Self {
            measurement_start_unix_epoch_ns,
            declared_measurement_duration_ns: measurement_secs.saturating_mul(INTERVAL_WIDTH_NS),
            maximum_rows,
            current_bucket: 0,
            current_histogram: histogram,
            current_events: 0,
            current_unique: 0,
            current_duplicates: 0,
            rows: Vec::with_capacity(maximum_rows),
            late_arrivals: 0,
            finalized: false,
        })
    }

    fn record(&mut self, elapsed_ns: u64, latency_ns: u64, duplicate: bool) -> anyhow::Result<()> {
        let bucket = usize::try_from(elapsed_ns / INTERVAL_WIDTH_NS).unwrap_or(usize::MAX);
        if bucket < self.current_bucket {
            self.late_arrivals = self.late_arrivals.saturating_add(1);
            return Ok(());
        }
        if bucket >= self.maximum_rows {
            anyhow::bail!("interval row limit exceeded ({})", self.maximum_rows);
        }
        while self.current_bucket < bucket {
            self.finish_current(INTERVAL_WIDTH_NS)?;
        }
        self.current_histogram.record(latency_ns)?;
        self.current_events = self.current_events.saturating_add(1);
        if duplicate {
            self.current_duplicates = self.current_duplicates.saturating_add(1);
        } else {
            self.current_unique = self.current_unique.saturating_add(1);
        }
        Ok(())
    }

    fn finalize(&mut self, elapsed_ns: u64) -> anyhow::Result<()> {
        if self.finalized {
            return Ok(());
        }
        self.finalized = true;
        if elapsed_ns == 0 {
            return Ok(());
        }
        let final_bucket =
            usize::try_from(elapsed_ns.saturating_sub(1) / INTERVAL_WIDTH_NS).unwrap_or(usize::MAX);
        if final_bucket >= self.maximum_rows {
            anyhow::bail!("interval row limit exceeded ({})", self.maximum_rows);
        }
        while self.current_bucket < final_bucket {
            self.finish_current(INTERVAL_WIDTH_NS)?;
        }
        let remainder = elapsed_ns % INTERVAL_WIDTH_NS;
        self.finish_current(if remainder == 0 { INTERVAL_WIDTH_NS } else { remainder })
    }

    fn finish_current(&mut self, width_ns: u64) -> anyhow::Result<()> {
        if self.rows.len() == self.maximum_rows {
            anyhow::bail!("interval row limit exceeded ({})", self.maximum_rows);
        }
        let start_ns = u64::try_from(self.current_bucket)
            .unwrap_or(u64::MAX)
            .saturating_mul(INTERVAL_WIDTH_NS);
        let end_ns = start_ns.saturating_add(width_ns);
        let count = self.current_histogram.len();
        self.rows.push(IntervalLatencyRow {
            interval_start_ns: start_ns,
            interval_end_ns: end_ns,
            interval_start_unix_epoch_ns: self
                .measurement_start_unix_epoch_ns
                .saturating_add(start_ns),
            interval_end_unix_epoch_ns: self.measurement_start_unix_epoch_ns.saturating_add(end_ns),
            latency_count: count,
            latency_p50_ns: (count > 0).then(|| self.current_histogram.value_at_quantile(0.50)),
            latency_p95_ns: (count > 0).then(|| self.current_histogram.value_at_quantile(0.95)),
            latency_p99_ns: (count > 0).then(|| self.current_histogram.value_at_quantile(0.99)),
            received_events: self.current_events,
            throughput_messages: self.current_unique,
            duplicates: self.current_duplicates,
        });
        self.current_histogram.reset();
        self.current_events = 0;
        self.current_unique = 0;
        self.current_duplicates = 0;
        self.current_bucket = self.current_bucket.saturating_add(1);
        Ok(())
    }

    fn artifact(&self, aggregate_count: u64) -> anyhow::Result<Vec<u8>> {
        if !self.finalized {
            anyhow::bail!("interval recorder was not finalized");
        }
        let interval_count = self.rows.iter().map(|row| row.latency_count).sum::<u64>();
        if interval_count != aggregate_count {
            anyhow::bail!(
                "interval latency population {interval_count} differs from aggregate {aggregate_count}"
            );
        }
        let artifact = serde_json::json!({
            "schema_version": 1,
            "interval_clock": "monotonic-elapsed",
            "alignment_clock": "unix-epoch",
            "alignment_clock_purpose": "cross-process-alignment-only",
            "measurement_start_unix_epoch_ns": self.measurement_start_unix_epoch_ns,
            "declared_measurement_duration_ns": self.declared_measurement_duration_ns,
            "bucket_width_ns": INTERVAL_WIDTH_NS,
            "maximum_rows": self.maximum_rows,
            "row_count": self.rows.len(),
            "aggregate_latency_count": aggregate_count,
            "late_arrivals": self.late_arrivals,
            "rows": self.rows,
        });
        Ok(format!("{}\n", serde_json::to_string_pretty(&artifact)?).into_bytes())
    }
}

#[derive(Deserialize)]
struct TimestampedMessage {
    ts: u64,
    seq: u64,
}

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

#[derive(Debug, Clone, Serialize, PartialEq)]
struct ThroughputBucket {
    start_offset_ns: i64,
    end_offset_ns: i64,
    received_unique: u64,
    received_events: u64,
    duplicates: u64,
    rate_msg_s: f64,
}

fn event_buckets(
    samples: &[EventSample],
    event_timestamp_ns: u64,
    start_offset_ns: i64,
    width_ns: i64,
    count: usize,
) -> Vec<ThroughputBucket> {
    let mut unique = vec![0_u64; count];
    let mut events = vec![0_u64; count];
    let mut duplicates = vec![0_u64; count];
    let end_offset_ns = start_offset_ns
        .saturating_add(i64::try_from(count).unwrap_or(i64::MAX).saturating_mul(width_ns));
    for sample in samples {
        let offset = signed_offset(sample.receive_unix_epoch_ns, event_timestamp_ns);
        if !(start_offset_ns..end_offset_ns).contains(&offset) {
            continue;
        }
        let index = usize::try_from(
            offset.saturating_sub(start_offset_ns).checked_div(width_ns).unwrap_or(i64::MAX),
        )
        .unwrap_or(count);
        let (Some(event_count), Some(unique_count), Some(duplicate_count)) =
            (events.get_mut(index), unique.get_mut(index), duplicates.get_mut(index))
        else {
            continue;
        };
        *event_count = event_count.saturating_add(1);
        if sample.duplicate {
            *duplicate_count = duplicate_count.saturating_add(1);
        } else {
            *unique_count = unique_count.saturating_add(1);
        }
    }
    unique
        .iter()
        .zip(&events)
        .zip(&duplicates)
        .enumerate()
        .map(|(index, ((received_unique, received_events), duplicates))| {
            let start_offset_ns = start_offset_ns
                .saturating_add(i64::try_from(index).unwrap_or(i64::MAX).saturating_mul(width_ns));
            ThroughputBucket {
                start_offset_ns,
                end_offset_ns: start_offset_ns.saturating_add(width_ns),
                received_unique: *received_unique,
                received_events: *received_events,
                duplicates: *duplicates,
                rate_msg_s: f64::from(u32::try_from(*received_unique).unwrap_or(u32::MAX))
                    * 1_000_000_000.0
                    / f64::from(u32::try_from(width_ns).unwrap_or(u32::MAX)),
            }
        })
        .collect()
}

fn bucket_totals(buckets: &[ThroughputBucket]) -> (u64, u64, u64) {
    buckets.iter().fold((0, 0, 0), |(unique, events, duplicates), bucket| {
        (
            unique.saturating_add(bucket.received_unique),
            events.saturating_add(bucket.received_events),
            duplicates.saturating_add(bucket.duplicates),
        )
    })
}

fn write_fine_event_buckets(
    path: &Path,
    samples: &[EventSample],
    measurement_start_timestamp_ns: u64,
    scheduled_event_timestamp_ns: u64,
    event_timestamp_ns: u64,
    alignment_error_ns: i64,
    alignment_tolerance_ns: i64,
) -> anyhow::Result<()> {
    let buckets = event_buckets(
        samples,
        event_timestamp_ns,
        FINE_EVENT_COVERAGE_START_NS,
        FINE_EVENT_BUCKET_WIDTH_NS,
        FINE_EVENT_BUCKET_COUNT,
    );
    let parent_buckets = event_buckets(
        samples,
        event_timestamp_ns,
        FINE_EVENT_COVERAGE_START_NS,
        FINE_EVENT_PARENT_BUCKET_WIDTH_NS,
        FINE_EVENT_PARENT_BUCKET_COUNT,
    );
    let (received_unique, received_events, duplicates) = bucket_totals(&buckets);
    if bucket_totals(&parent_buckets) != (received_unique, received_events, duplicates) {
        anyhow::bail!("fine and parent event bucket populations differ");
    }
    let artifact = serde_json::json!({
        "schema_version": 1,
        "clock": "unix-epoch",
        "clock_purpose": "cross-process-alignment",
        "alignment": "actual-t0",
        "measurement_start_timestamp_ns": measurement_start_timestamp_ns,
        "scheduled_event_timestamp_ns": scheduled_event_timestamp_ns,
        "event_timestamp_ns": event_timestamp_ns,
        "alignment_error_ns": alignment_error_ns,
        "alignment_tolerance_ns": alignment_tolerance_ns,
        "bucket_width_ns": FINE_EVENT_BUCKET_WIDTH_NS,
        "bucket_count": FINE_EVENT_BUCKET_COUNT,
        "coverage_start_offset_ns": FINE_EVENT_COVERAGE_START_NS,
        "coverage_end_offset_ns": FINE_EVENT_COVERAGE_END_NS,
        "parent_bucket_width_ns": FINE_EVENT_PARENT_BUCKET_WIDTH_NS,
        "parent_bucket_count": FINE_EVENT_PARENT_BUCKET_COUNT,
        "received_unique": received_unique,
        "received_events": received_events,
        "duplicates": duplicates,
        "buckets": buckets,
        "parent_buckets": parent_buckets,
        "canonical_series": "throughput-buckets.json",
        "loss_accounting": "canonical-sequence-and-primary-drain-only",
    });
    write_atomic(path, format!("{}\n", serde_json::to_string_pretty(&artifact)?).as_bytes())?;
    Ok(())
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
        let buckets = event_buckets(
            &self.samples,
            action.event_timestamp_ns,
            EVENT_COVERAGE_START_NS,
            i64::try_from(EVENT_BUCKET_WIDTH_NS).unwrap_or(i64::MAX),
            EVENT_BUCKET_COUNT,
        );
        let (received_unique, received_events, duplicates) = bucket_totals(&buckets);
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
            "received_unique": received_unique,
            "received_events": received_events,
            "duplicates": duplicates,
            "buckets": buckets,
        });
        write_atomic(path, format!("{}\n", serde_json::to_string_pretty(&artifact)?).as_bytes())?;
        write_fine_event_buckets(
            &path.with_file_name("throughput-buckets-10ms.json"),
            &self.samples,
            self.measurement_start_timestamp_ns,
            self.scheduled_event_timestamp_ns,
            action.event_timestamp_ns,
            action.alignment_error_ns,
            action.alignment_tolerance_ns,
        )
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
    /// Message parsed and its latency+sequence were recorded. `latency_ns` is
    /// the value the histogram received, within the shared latency bounds.
    Recorded { intended_ns: u64, latency_ns: u64, seq: u64 },
    /// Payload could not be parsed as JSON with `ts` and `seq` fields.
    ParseError,
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
    /// `complete`, or `partial` when a bounded side artifact overflowed or
    /// failed and was left out. The aggregate artifacts are complete either way.
    #[serde(default = "complete_status")]
    pub status: String,
    /// Why the run is `partial`. Empty for a complete run.
    #[serde(default)]
    pub partial_reasons: Vec<String>,
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
    /// Latencies below zero, recorded at the lower histogram bound.
    pub negative_latency_count: u64,
    /// Latencies above the upper histogram bound, recorded at that bound.
    #[serde(default)]
    pub above_highest_latency_count: u64,
    /// Wall-clock steps seen while receiving (see [`ClockSteps`]).
    #[serde(default)]
    pub clock_steps: u64,
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

fn complete_status() -> String {
    "complete".to_owned()
}

/// End-to-end latency recorder for the `wafer-loadgen subscribe` command.
///
/// Uses the shared latency bounds (1 µs to 1 h, 3 significant digits), the
/// same as `BenchSink`, and counts every sample it has to clamp.
pub struct LatencyRecorder {
    histogram: Histogram<u64>,
    intervals: Option<IntervalRecorder>,
    interval_origin_elapsed_ns: Option<u64>,
    sequence: SequenceTracker,
    total_messages: u64,
    parse_errors: u64,
    clamps: LatencyClamps,
    clock_steps: ClockSteps,
    ignored_sequences: u64,
    last_record_duplicate: bool,
    partial_reasons: Vec<String>,
}

impl LatencyRecorder {
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
        let histogram = Histogram::<u64>::new_with_bounds(
            LATENCY_LOWEST_NS,
            LATENCY_HIGHEST_NS,
            LATENCY_SIG_DIGITS,
        )
        .expect("HdrHistogram bounds are compile-time constants and known valid");
        Self {
            histogram,
            intervals: None,
            interval_origin_elapsed_ns: None,
            sequence: max_examples
                .map_or_else(SequenceTracker::new, SequenceTracker::with_max_examples),
            total_messages: 0,
            parse_errors: 0,
            clamps: LatencyClamps::default(),
            clock_steps: ClockSteps::default(),
            ignored_sequences: 0,
            last_record_duplicate: false,
            partial_reasons: Vec::new(),
        }
    }

    pub(crate) fn enable_intervals(
        &mut self,
        measurement_start_unix_epoch_ns: u64,
        measurement_secs: u64,
    ) -> anyhow::Result<()> {
        self.intervals =
            Some(IntervalRecorder::new(measurement_start_unix_epoch_ns, measurement_secs)?);
        Ok(())
    }

    /// Parse a JSON payload (with `ts` = intended-publish-ns and `seq` = u64)
    /// and record the observed latency against `receive_ns`.
    pub fn record_json(&mut self, payload: &[u8], receive_ns: u64) -> RecordOutcome {
        self.record_json_with_sequence_end(payload, receive_ns, None)
    }

    /// Like [`Self::record_json`], and also feeds the interval recorder when
    /// enabled. An interval bound violation drops `interval-latency.json` and
    /// marks the run partial; the aggregate recording continues.
    pub(crate) fn record_json_at(
        &mut self,
        payload: &[u8],
        receive_ns: u64,
        elapsed_ns: u64,
        sequence_end_exclusive: Option<u64>,
    ) -> RecordOutcome {
        self.clock_steps.observe(receive_ns, elapsed_ns);
        let outcome =
            self.record_json_with_sequence_end(payload, receive_ns, sequence_end_exclusive);
        if let RecordOutcome::Recorded { latency_ns, .. } = outcome
            && let Some(intervals) = &mut self.intervals
        {
            let origin = *self.interval_origin_elapsed_ns.get_or_insert_with(|| {
                intervals.measurement_start_unix_epoch_ns = receive_ns;
                elapsed_ns
            });
            if let Err(error) = intervals.record(
                elapsed_ns.saturating_sub(origin),
                latency_ns,
                self.last_record_duplicate,
            ) {
                self.drop_intervals(&error);
            }
        }
        outcome
    }

    /// Take the wall clock at the monotonic zero that `elapsed_ns` counts from,
    /// so a step before the first message is caught too.
    pub(crate) fn anchor_clock(&mut self, wall_ns_at_zero: u64) {
        self.clock_steps.observe(wall_ns_at_zero, 0);
    }

    fn drop_intervals(&mut self, error: &anyhow::Error) {
        self.intervals = None;
        self.mark_partial(format!("interval-latency.json: {error}"));
    }

    /// Record why the run's artifacts are incomplete.
    pub(crate) fn mark_partial(&mut self, reason: String) {
        self.partial_reasons.push(reason);
    }

    pub(crate) fn partial_reasons(&self) -> &[String] {
        &self.partial_reasons
    }

    /// Record only messages whose sequence is below `sequence_end_exclusive`.
    #[cfg(test)]
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
        let Ok(TimestampedMessage { ts, seq }) = serde_json::from_slice(payload) else {
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
        let latency_ns = self.clamps.bound(intended_publish_ns, receive_ns);
        self.histogram.saturating_record(latency_ns);
        self.last_record_duplicate = self.sequence.record(seq);
        RecordOutcome::Recorded { intended_ns: intended_publish_ns, latency_ns, seq }
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
    pub const fn clamps(&self) -> LatencyClamps {
        self.clamps
    }

    #[must_use]
    pub const fn clock_steps(&self) -> u64 {
        self.clock_steps.steps
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

    pub(crate) fn finalize_intervals(&mut self, measurement_elapsed_ns: u64) {
        if let Some(intervals) = &mut self.intervals {
            let elapsed_ns = measurement_elapsed_ns
                .saturating_sub(self.interval_origin_elapsed_ns.unwrap_or_default());
            if let Err(error) = intervals.finalize(elapsed_ns) {
                self.drop_intervals(&error);
            }
        }
    }

    /// Write aggregate artifacts and, when enabled, `interval-latency.json` to `dir`,
    /// then `subscriber-metadata.json`.
    ///
    /// # Errors
    /// - Directory creation failure.
    /// - Any file write failure.
    /// - [`HdrHistogram`] V2 serialization failure (unreachable in practice).
    pub fn write_artifacts(
        &mut self,
        dir: &Path,
        metadata: SubscriberMetadata,
    ) -> anyhow::Result<()> {
        self.write_measurements(dir)?;
        self.write_metadata(dir, metadata)
    }

    /// Write `latency.hdr`, `sequence.csv` and, when enabled,
    /// `interval-latency.json`. An interval fragment that fails its checks is
    /// left out and marks the run partial.
    ///
    /// # Errors
    /// Directory creation, serialization or file write failure.
    pub(crate) fn write_measurements(&mut self, dir: &Path) -> anyhow::Result<()> {
        fs::create_dir_all(dir)?;
        let hdr_bytes =
            self.serialize_v2().map_err(|e| anyhow::anyhow!("hdr V2 serialize: {e:?}"))?;
        write_atomic(&dir.join("latency.hdr"), &hdr_bytes)?;
        write_atomic(&dir.join("sequence.csv"), self.sequence_csv().as_bytes())?;
        if let Some(intervals) = &self.intervals {
            match intervals.artifact(self.histogram.len()) {
                Ok(bytes) => write_atomic(&dir.join("interval-latency.json"), &bytes)?,
                Err(error) => self.drop_intervals(&error),
            }
        }
        Ok(())
    }

    /// Write `subscriber-metadata.json`. Call it after every other subscriber
    /// artifact, so its presence means they are complete.
    ///
    /// # Errors
    /// Directory creation, serialization or file write failure.
    pub(crate) fn write_metadata(
        &self,
        dir: &Path,
        mut metadata: SubscriberMetadata,
    ) -> anyhow::Result<()> {
        fs::create_dir_all(dir)?;
        metadata.total_recorded = self.total_recorded();
        metadata.total_messages = self.total_messages;
        metadata.parse_errors = self.parse_errors;
        metadata.negative_latency_count = self.clamps.negative;
        metadata.above_highest_latency_count = self.clamps.above_highest;
        metadata.clock_steps = self.clock_steps.steps;
        metadata.ignored_sequence_count = self.ignored_sequences;
        metadata.latency_min_ns = self.min_ns();
        metadata.latency_max_ns = self.max_ns();
        metadata.latency_mean_ns = self.mean_ns();
        metadata.latency_p50_ns = self.p50_ns();
        metadata.latency_p95_ns = self.p95_ns();
        metadata.latency_p99_ns = self.p99_ns();
        metadata.latency_p999_ns = self.p999_ns();
        metadata.histogram_lowest_ns = LATENCY_LOWEST_NS;
        metadata.histogram_highest_ns = LATENCY_HIGHEST_NS;
        metadata.histogram_sig_digits = LATENCY_SIG_DIGITS;
        metadata.sequence = SequenceReport::from(&self.sequence);
        metadata.partial_reasons.clone_from(&self.partial_reasons);
        let status = if self.partial_reasons.is_empty() { "complete" } else { "partial" };
        status.clone_into(&mut metadata.status);
        let json = serde_json::to_string_pretty(&metadata)?;
        write_atomic(&dir.join("subscriber-metadata.json"), format!("{json}\n").as_bytes())?;
        Ok(())
    }
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
    let result = fs::File::create(&temporary)
        .and_then(|mut file| file.write_all(bytes).and_then(|()| file.sync_all()))
        .and_then(|()| fs::rename(&temporary, path));
    if result.is_err() {
        drop(fs::remove_file(&temporary));
    }
    result
}

/// Counts steps of the wall clock between received messages.
///
/// Latency subtracts the publisher's wall-clock schedule from this process's
/// wall-clock receive time, so a clock step during the run shifts every later
/// sample by the size of the step. NTP rate corrections move the wall and
/// monotonic clocks together; only a step changes the gap between them.
#[derive(Debug, Default)]
struct ClockSteps {
    wall_minus_monotonic_ns: Option<i128>,
    steps: u64,
}

impl ClockSteps {
    /// NTP clients slew small offsets and only step large ones (ntpd above
    /// 128 ms, systemd-timesyncd above 400 ms), so a real step is well above
    /// this, while a thread delayed between the two clock reads stays below it.
    const TOLERANCE_NS: u128 = 100_000_000;

    fn observe(&mut self, wall_ns: u64, monotonic_elapsed_ns: u64) {
        let gap = i128::from(wall_ns).saturating_sub(i128::from(monotonic_elapsed_ns));
        if let Some(previous) = self.wall_minus_monotonic_ns.replace(gap)
            && gap.abs_diff(previous) > Self::TOLERANCE_NS
        {
            self.steps = self.steps.saturating_add(1);
        }
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
        assert_eq!(
            outcome,
            RecordOutcome::Recorded { intended_ns: intended, latency_ns: 250_000, seq: 7 }
        );
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
            RecordOutcome::Recorded { intended_ns: intended, latency_ns: 250_000, seq: 0 }
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
    fn out_of_range_latencies_are_recorded_at_a_bound_and_counted() {
        let dir = tempfile::tempdir().unwrap();
        let mut rec = LatencyRecorder::new();
        rec.enable_intervals(0, 4).unwrap();
        let receive_origin = 2 * LATENCY_HIGHEST_NS;
        // (scheduled time relative to receive, value the histograms must get)
        let cases = [
            (1_000_i128, LATENCY_LOWEST_NS),
            (-500, LATENCY_LOWEST_NS),
            (-20_000_000_000, 20_000_000_000),
            (-i128::from(LATENCY_HIGHEST_NS) - 1, LATENCY_HIGHEST_NS),
        ];
        for (seq, (ts_offset, expected)) in (0_u64..).zip(cases) {
            let elapsed = seq * 1_000_000_000;
            let receive = receive_origin + elapsed;
            let ts = u64::try_from(i128::from(receive) + ts_offset).unwrap();
            let payload = format!(r#"{{"ts":{ts},"seq":{seq}}}"#);
            let outcome = rec.record_json_at(payload.as_bytes(), receive, elapsed, None);
            assert_eq!(
                outcome,
                RecordOutcome::Recorded { intended_ns: ts, latency_ns: expected, seq }
            );
        }
        rec.finalize_intervals(4_000_000_000);
        rec.write_measurements(dir.path()).unwrap();

        assert_eq!(rec.total_recorded(), 4);
        assert_eq!(rec.histogram.count_at(LATENCY_LOWEST_NS), 2);
        assert_eq!(rec.histogram.count_at(20_000_000_000), 1);
        assert_eq!(rec.histogram.count_at(LATENCY_HIGHEST_NS), 1);
        assert_eq!(rec.clamps(), LatencyClamps { negative: 1, above_highest: 1 });
        assert_eq!(rec.clock_steps(), 0);
        let intervals: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.path().join("interval-latency.json")).unwrap())
                .unwrap();
        for (row, (_, expected)) in intervals["rows"].as_array().unwrap().iter().zip(cases) {
            assert_eq!(row["latency_count"], 1);
            let interval_value = row["latency_p50_ns"].as_u64().unwrap();
            assert!(
                rec.histogram.equivalent(interval_value, expected),
                "interval recorded {interval_value}, aggregate {expected}"
            );
        }
    }

    #[test]
    fn a_wall_clock_step_between_messages_is_counted() {
        let mut rec = LatencyRecorder::new();
        rec.anchor_clock(1_900_000_000);
        let payload = |seq: u64| format!(r#"{{"ts":1000000000,"seq":{seq}}}"#);

        rec.record_json_at(payload(0).as_bytes(), 2_100_000_000, 50_000_000, None);
        assert_eq!(rec.clock_steps(), 1, "a step before the first message");

        rec.record_json_at(payload(1).as_bytes(), 2_150_000_000, 100_000_000, None);
        assert_eq!(rec.clock_steps(), 1);

        rec.record_json_at(payload(2).as_bytes(), 2_050_000_000, 150_000_000, None);
        assert_eq!(rec.clock_steps(), 2, "a step back between messages");

        rec.record_json_at(payload(3).as_bytes(), 2_050_000_000, 200_000_000, None);
        assert_eq!(rec.clock_steps(), 2, "50 ms between two clock reads is not a step");
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

        let fine: serde_json::Value = serde_json::from_slice(
            &fs::read(dir.path().join("throughput-buckets-10ms.json")).unwrap(),
        )
        .unwrap();
        let fine_rows = fine["buckets"].as_array().unwrap();
        let parents = fine["parent_buckets"].as_array().unwrap();
        assert_eq!(fine_rows.len(), 400);
        assert_eq!(parents.len(), 40);
        assert_eq!(fine_rows[0]["start_offset_ns"], -2_000_000_000_i64);
        assert_eq!(fine_rows[399]["end_offset_ns"], 2_000_000_000_i64);
        assert_eq!(fine["alignment"], "actual-t0");
        assert_eq!(fine["event_timestamp_ns"], action.event_timestamp_ns);
        assert_eq!(fine["alignment_error_ns"], action.alignment_error_ns);
        assert_eq!(fine["alignment_tolerance_ns"], action.alignment_tolerance_ns);
        assert_eq!(fine["canonical_series"], "throughput-buckets.json");
        assert_eq!(fine["loss_accounting"], "canonical-sequence-and-primary-drain-only");
        assert!(
            fs::metadata(dir.path().join("throughput-buckets-10ms.json")).unwrap().len()
                < 256 * 1024
        );
        for (parent_index, parent) in parents.iter().enumerate() {
            for field in ["received_unique", "received_events", "duplicates"] {
                let nested = fine_rows[parent_index * 10..(parent_index + 1) * 10]
                    .iter()
                    .map(|row| row[field].as_u64().unwrap())
                    .sum::<u64>();
                assert_eq!(parent[field], nested);
            }
        }
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
    fn interval_recorder_rejects_zero_duration() {
        assert!(IntervalRecorder::new(0, 0).is_err());
    }

    #[test]
    fn one_second_intervals_emit_empty_and_final_partial_rows() {
        let mut intervals = IntervalRecorder::new(10_000_000_000, 3).unwrap();
        intervals.record(0, 10_000, false).unwrap();
        intervals.record(2_000_000_000, 30_000, false).unwrap();
        intervals.finalize(2_500_000_000).unwrap();

        assert_eq!(intervals.rows.len(), 3);
        assert_eq!(intervals.rows[0].interval_start_ns, 0);
        assert_eq!(intervals.rows[0].interval_end_ns, 1_000_000_000);
        assert_eq!(intervals.rows[1].latency_count, 0);
        assert_eq!(intervals.rows[1].latency_p50_ns, None);
        assert_eq!(intervals.rows[2].interval_end_ns, 2_500_000_000);
        assert_eq!(intervals.rows.iter().map(|row| row.latency_count).sum::<u64>(), 2);
    }

    #[test]
    fn exact_boundary_starts_the_next_interval_without_extra_shutdown_row() {
        let mut intervals = IntervalRecorder::new(5_000_000_000, 2).unwrap();
        intervals.record(999_999_999, 10_000, false).unwrap();
        intervals.record(1_000_000_000, 20_000, true).unwrap();
        intervals.finalize(2_000_000_000).unwrap();

        assert_eq!(intervals.rows.len(), 2);
        assert_eq!(intervals.rows[0].latency_count, 1);
        assert_eq!(intervals.rows[1].latency_count, 1);
        assert_eq!(intervals.rows[1].received_events, 1);
        assert_eq!(intervals.rows[1].throughput_messages, 0);
        assert_eq!(intervals.rows[1].duplicates, 1);
    }

    #[test]
    fn intervals_bound_cardinality_and_count_late_arrivals() {
        let mut intervals = IntervalRecorder::new(0, 2).unwrap();
        let allocated_rows = intervals.rows.capacity();
        intervals.record(1_000_000_000, 10_000, false).unwrap();
        intervals.record(500_000_000, 10_000, false).unwrap();
        intervals.record(3_999_999_999, 10_000, false).unwrap();
        assert_eq!(intervals.late_arrivals, 1);
        assert_eq!(intervals.rows.capacity(), allocated_rows);
        assert_eq!(intervals.rows.len(), 3);
        assert!(intervals.record(4_000_000_000, 10_000, false).is_err());
    }

    #[test]
    fn interval_shutdown_beyond_declared_slack_fails_closed() {
        let mut intervals = IntervalRecorder::new(0, 1).unwrap();
        intervals.record(0, 10_000, false).unwrap();
        assert!(intervals.finalize(3_000_000_001).is_err());
    }

    #[test]
    fn interval_artifact_has_clock_labels_and_no_per_message_fields() {
        let mut intervals = IntervalRecorder::new(9_000_000_000, 1).unwrap();
        intervals.record(0, 10_000, false).unwrap();
        intervals.finalize(1_000_000_000).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&intervals.artifact(1).unwrap()).unwrap();
        assert_eq!(value["interval_clock"], "monotonic-elapsed");
        assert_eq!(value["alignment_clock"], "unix-epoch");
        assert_eq!(value["maximum_rows"], 3);
        let serialized = serde_json::to_string(&value).unwrap();
        for forbidden in ["message_id", "sequence_id", "per_message_timestamp"] {
            assert!(!serialized.contains(forbidden));
        }
    }

    #[test]
    fn interval_artifact_rejects_population_mismatch() {
        let mut intervals = IntervalRecorder::new(0, 1).unwrap();
        intervals.record(0, 10_000, false).unwrap();
        intervals.finalize(1_000_000_000).unwrap();
        intervals.artifact(2).unwrap_err();
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
    fn latency_recorder_writes_interval_fragment_on_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = LatencyRecorder::with_sequence_example_limit(Some(4));
        recorder.enable_intervals(1_000_000_000, 2).unwrap();
        let payload = br#"{"ts":1000000000,"seq":0}"#;
        recorder.record_json_at(payload, 1_000_010_000, 500_000_000, None);
        let metadata = SubscriberMetadata {
            broker: "localhost:1883".into(),
            topic: "wafer/test".into(),
            started_at_ns: 1_000_000_000,
            ended_at_ns: 2_500_000_000,
            exit_reason: "sigint".into(),
            status: String::new(),
            partial_reasons: vec![],
            git_sha: None,
            host_tag: None,
            sequence_end_exclusive: None,
            ignored_sequence_count: 0,
            unexpected_sequence_count: 0,
            total_recorded: 0,
            total_messages: 0,
            parse_errors: 0,
            negative_latency_count: 0,
            above_highest_latency_count: 0,
            clock_steps: 0,
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
        recorder.finalize_intervals(2_000_000_000);
        recorder.write_artifacts(dir.path(), metadata).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.path().join("interval-latency.json")).unwrap())
                .unwrap();
        assert_eq!(value["row_count"], 2);
        assert_eq!(value["rows"][0]["latency_count"], 1);
        assert_eq!(value["rows"][1]["latency_count"], 0);
    }

    #[test]
    fn interval_overflow_keeps_aggregate_artifacts_and_marks_run_partial() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = LatencyRecorder::new();
        recorder.enable_intervals(1_000_000_000, 1).unwrap();
        for (seq, elapsed_ns) in [(0, 0), (1, 5_000_000_000), (2, 5_100_000_000)] {
            let payload = format!(r#"{{"ts":1000000000,"seq":{seq}}}"#);
            recorder.record_json_at(payload.as_bytes(), 1_000_010_000, elapsed_ns, None);
        }
        recorder.finalize_intervals(6_000_000_000);
        let metadata: SubscriberMetadata = serde_json::from_value(serde_json::json!({
            "broker": "localhost:1883", "topic": "wafer/test", "started_at_ns": 1,
            "ended_at_ns": 2, "exit_reason": "sigterm", "git_sha": null, "host_tag": null,
            "total_recorded": 0, "total_messages": 0, "parse_errors": 0,
            "negative_latency_count": 0, "latency_min_ns": 0, "latency_max_ns": 0,
            "latency_mean_ns": 0.0, "latency_p50_ns": 0, "latency_p95_ns": 0,
            "latency_p99_ns": 0, "latency_p999_ns": 0, "histogram_lowest_ns": 0,
            "histogram_highest_ns": 0, "histogram_sig_digits": 0,
            "sequence": {"total_received": 0, "total_gaps": 0, "total_duplicates": 0,
                         "gap_ranges": [], "duplicate_seqs": []},
        }))
        .unwrap();

        recorder.write_artifacts(dir.path(), metadata).unwrap();

        assert!(dir.path().join("latency.hdr").is_file());
        assert!(dir.path().join("sequence.csv").is_file());
        assert!(!dir.path().join("interval-latency.json").exists());
        let written: SubscriberMetadata =
            serde_json::from_slice(&fs::read(dir.path().join("subscriber-metadata.json")).unwrap())
                .unwrap();
        assert_eq!(written.status, "partial");
        assert_eq!(written.partial_reasons.len(), 1);
        assert!(written.partial_reasons[0].contains("interval row limit exceeded"));
        assert_eq!(written.total_recorded, 3);
        assert_eq!(written.sequence.total_received, 3);
    }

    #[test]
    fn interval_population_mismatch_marks_run_partial() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = LatencyRecorder::new();
        recorder.enable_intervals(1_000_000_000, 5).unwrap();
        for (seq, elapsed_ns) in [(0, 0), (1, 1_500_000_000), (2, 500_000_000)] {
            let payload = format!(r#"{{"ts":1000000000,"seq":{seq}}}"#);
            recorder.record_json_at(payload.as_bytes(), 1_000_010_000, elapsed_ns, None);
        }
        recorder.finalize_intervals(2_000_000_000);

        recorder.write_measurements(dir.path()).unwrap();

        assert!(recorder.partial_reasons()[0].contains("differs from aggregate"));
        assert!(!dir.path().join("interval-latency.json").exists());
    }

    #[test]
    fn write_artifacts_leaves_no_temporary_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = LatencyRecorder::new();
        recorder.record(0, 10_000_000, 0);
        let metadata: SubscriberMetadata = serde_json::from_value(serde_json::json!({
            "broker": "localhost:1883", "topic": "wafer/test", "started_at_ns": 1,
            "ended_at_ns": 2, "exit_reason": "total-messages", "git_sha": null,
            "host_tag": null, "total_recorded": 0, "total_messages": 0, "parse_errors": 0,
            "negative_latency_count": 0, "latency_min_ns": 0, "latency_max_ns": 0,
            "latency_mean_ns": 0.0, "latency_p50_ns": 0, "latency_p95_ns": 0,
            "latency_p99_ns": 0, "latency_p999_ns": 0, "histogram_lowest_ns": 0,
            "histogram_highest_ns": 0, "histogram_sig_digits": 0,
            "sequence": {"total_received": 0, "total_gaps": 0, "total_duplicates": 0,
                         "gap_ranges": [], "duplicate_seqs": []},
        }))
        .unwrap();

        recorder.write_artifacts(dir.path(), metadata).unwrap();

        let mut names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["latency.hdr", "sequence.csv", "subscriber-metadata.json"]);
    }

    #[test]
    fn interval_output_is_additive_to_existing_aggregate_artifacts() {
        let baseline_dir = tempfile::tempdir().unwrap();
        let interval_dir = tempfile::tempdir().unwrap();
        let mut baseline = LatencyRecorder::with_sequence_example_limit(Some(4));
        let mut interval = LatencyRecorder::with_sequence_example_limit(Some(4));
        interval.enable_intervals(1_000_000_000, 1).unwrap();
        for seq in 0..3 {
            let payload = format!(r#"{{"ts":1000000000,"seq":{seq}}}"#);
            let elapsed = seq * 100_000_000;
            baseline.record_json(payload.as_bytes(), 1_000_010_000 + elapsed);
            interval.record_json_at(payload.as_bytes(), 1_000_010_000 + elapsed, elapsed, None);
        }
        let metadata = SubscriberMetadata {
            broker: "localhost:1883".into(),
            topic: "wafer/test".into(),
            started_at_ns: 1_000_000_000,
            ended_at_ns: 2_000_000_000,
            exit_reason: "total-messages".into(),
            status: String::new(),
            partial_reasons: vec![],
            git_sha: None,
            host_tag: None,
            sequence_end_exclusive: None,
            ignored_sequence_count: 0,
            unexpected_sequence_count: 0,
            total_recorded: 0,
            total_messages: 0,
            parse_errors: 0,
            negative_latency_count: 0,
            above_highest_latency_count: 0,
            clock_steps: 0,
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
        baseline.write_artifacts(baseline_dir.path(), metadata.clone()).unwrap();
        interval.finalize_intervals(1_000_000_000);
        interval.write_artifacts(interval_dir.path(), metadata).unwrap();
        for artifact in ["latency.hdr", "sequence.csv", "subscriber-metadata.json"] {
            assert_eq!(
                fs::read(baseline_dir.path().join(artifact)).unwrap(),
                fs::read(interval_dir.path().join(artifact)).unwrap(),
                "interval instrumentation changed {artifact}",
            );
        }
        assert!(interval_dir.path().join("interval-latency.json").is_file());
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
            status: String::new(),
            partial_reasons: vec![],
            git_sha: Some("deadbeef".into()),
            host_tag: Some("shakedown-macos".into()),
            sequence_end_exclusive: None,
            ignored_sequence_count: 0,
            unexpected_sequence_count: 0,
            total_recorded: 0,
            total_messages: 0,
            parse_errors: 0,
            negative_latency_count: 0,
            above_highest_latency_count: 0,
            clock_steps: 0,
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
