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
}

impl SequenceTracker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a received sequence number.
    pub fn record(&mut self, seq: u64) {
        self.total_received = self.total_received.saturating_add(1);
        match seq.cmp(&self.expected_next) {
            std::cmp::Ordering::Equal => {
                self.expected_next = self.expected_next.saturating_add(1);
            }
            std::cmp::Ordering::Greater => {
                // seq > expected_next, so subtraction cannot underflow
                #[expect(clippy::arithmetic_side_effects, reason = "seq > expected_next checked by match arm; seq - 1 safe because seq > 0 (seq > expected_next >= 0)")]
                {
                    self.gaps.push((self.expected_next, seq - 1));
                    self.expected_next = seq + 1;
                }
            }
            std::cmp::Ordering::Less => {
                self.duplicates.push(seq);
            }
        }
    }

    #[must_use]
    pub fn total_gaps(&self) -> u64 {
        self.gaps.iter().map(|(s, e)| e.saturating_sub(*s).saturating_add(1)).sum()
    }

    #[must_use]
    pub fn total_duplicates(&self) -> u64 {
        #[expect(clippy::as_conversions, reason = "Vec::len() is usize which fits in u64 on all targets")]
        { self.duplicates.len() as u64 }
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
}

impl From<&SequenceTracker> for SequenceReport {
    fn from(t: &SequenceTracker) -> Self {
        Self {
            total_received: t.total_received(),
            total_gaps: t.total_gaps(),
            total_duplicates: t.total_duplicates(),
            gap_ranges: t.gaps().to_vec(),
            duplicate_seqs: t.duplicates().to_vec(),
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
    /// Parsed messages excluded because they were outside the measured range.
    #[serde(default)]
    pub ignored_sequence_count: u64,

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
}

impl LatencyRecorder {
    /// Range: 1 µs to 10 s, 3 significant digits.
    pub const LOWEST_NS: u64 = 1_000;
    pub const HIGHEST_NS: u64 = 10_000_000_000;
    pub const SIG_DIGITS: u8 = 3;

    /// # Panics
    /// Never in practice. The three histogram parameters are compile-time
    /// constants proven valid by the `record_ten_ms_latency*` unit test
    /// (which constructs an instance) and by hdrhistogram's contract:
    /// `new_with_bounds` only fails for `low >= high` or `sig_digits > 5`.
    #[must_use]
    #[expect(
        clippy::expect_used,
        reason = "HdrHistogram bounds are compile-time constants proven valid by the unit tests below; new_with_bounds cannot fail with these arguments"
    )]
    pub fn new() -> Self {
        let histogram = Histogram::<u64>::new_with_bounds(
            Self::LOWEST_NS,
            Self::HIGHEST_NS,
            Self::SIG_DIGITS,
        )
        .expect("HdrHistogram bounds are compile-time constants and known valid");
        Self {
            histogram,
            sequence: SequenceTracker::new(),
            total_messages: 0,
            parse_errors: 0,
            negative_latency: 0,
            ignored_sequences: 0,
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
        #[expect(clippy::arithmetic_side_effects, reason = "subtraction safe: receive_ns >= intended_publish_ns guarded by the if-check above")]
        let latency_ns = receive_ns - intended_publish_ns;
        // Clamp below-histogram-floor values to the floor rather than dropping.
        // Sub-microsecond latencies are physically impossible over MQTT, but a
        // clock-skew payload could show one. Clamping preserves the count.
        let recorded = latency_ns.clamp(Self::LOWEST_NS, Self::HIGHEST_NS);
        // hdrhistogram returns Err only if the value is above the ceiling; we
        // already clamped so this can only fail on internal invariants.
        let _r = self.histogram.record(recorded);
        self.sequence.record(seq);
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
        let hdr_bytes = self
            .serialize_v2()
            .map_err(|e| anyhow::anyhow!("hdr V2 serialize: {e:?}"))?;
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
