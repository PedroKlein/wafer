//! Per-node HdrHistogram for latency distribution recording.
//!
//! `NodeLatencyRecorder` holds one histogram per pipeline node, recording
//! per-hop processing times. This enables E-Perf-4 per-hop decomposition:
//! understanding which nodes contribute most to end-to-end latency.
//!
//! Unlike the atomic `NodeMetrics` counters (which only track cumulative ns),
//! these histograms capture the full latency distribution — percentiles, min/max,
//! and outliers per node.
//!
//! See docs/rfcs/RFC-008-evaluation-harness.md — D1, D15.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use hdrhistogram::Histogram;
use hdrhistogram::serialization::V2Serializer;
use hdrhistogram::serialization::interval_log::IntervalLogWriterBuilder;

/// Per-node latency histogram collection.
///
/// Created once when a benchmark pipeline starts. Each node's loop calls
/// `record()` after processing a message with the hop duration in nanoseconds.
#[derive(Debug)]
pub struct NodeLatencyRecorder {
    /// Node ID → HdrHistogram (nanoseconds).
    histograms: HashMap<String, Histogram<u64>>,
}

impl NodeLatencyRecorder {
    /// Create a new recorder pre-allocated for the given node IDs.
    ///
    /// # Panics
    ///
    /// Panics if the internal histogram cannot be created (compile-time constant bounds; unreachable in practice).
    #[must_use]
    #[expect(
        clippy::expect_used,
        reason = "Histogram bounds are compile-time constants (1µs–10s, 3 sig figs); cannot fail"
    )]
    pub fn new(node_ids: &[&str]) -> Self {
        let mut histograms = HashMap::with_capacity(node_ids.len());
        for id in node_ids {
            // Same bounds as BenchSink: 1µs to 10s, 3 significant digits
            let hist = Histogram::new_with_bounds(1_000, 10_000_000_000, 3)
                .expect("valid histogram bounds");
            histograms.insert((*id).to_owned(), hist);
        }
        Self { histograms }
    }

    /// Record a per-hop latency for a specific node.
    ///
    /// If `duration_ns` is below the histogram minimum (1µs), it's recorded as 1µs.
    /// If the node ID is not registered, the call is silently ignored.
    #[inline]
    #[expect(
        clippy::let_underscore_must_use,
        reason = "histogram record errors only on values outside configured range; clamped values can still exceed max — silently dropping is correct for latency sampling"
    )]
    pub fn record(&mut self, node_id: &str, duration_ns: u64) {
        if let Some(hist) = self.histograms.get_mut(node_id) {
            let clamped = duration_ns.max(1_000);
            let _ = hist.record(clamped);
        }
    }

    /// Get the histogram for a specific node.
    #[must_use]
    pub fn get(&self, node_id: &str) -> Option<&Histogram<u64>> {
        self.histograms.get(node_id)
    }

    /// Get all node IDs with registered histograms.
    pub fn node_ids(&self) -> impl Iterator<Item = &str> {
        self.histograms.keys().map(String::as_str)
    }

    /// Number of registered nodes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.histograms.len()
    }

    /// Whether any nodes are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.histograms.is_empty()
    }

    /// Export per-node metrics as CSV.
    ///
    /// Format: `node_id,count,min_ns,p50_ns,p99_ns,p999_ns,max_ns,mean_ns`
    #[expect(
        clippy::expect_used,
        reason = "std::fmt::Write for String is infallible — cannot panic"
    )]
    pub fn to_csv(&self) -> String {
        use std::fmt::Write as _;
        let mut csv = String::from("node_id,count,min_ns,p50_ns,p99_ns,p999_ns,max_ns,mean_ns\n");

        let mut entries: Vec<(&String, &Histogram<u64>)> = self.histograms.iter().collect();
        entries.sort_by_key(|(id, _)| *id);

        for (id, hist) in entries {
            if hist.is_empty() {
                continue;
            }
            writeln!(
                csv,
                "{},{},{},{},{},{},{},{:.0}",
                id,
                hist.len(),
                hist.min(),
                hist.value_at_quantile(0.50),
                hist.value_at_quantile(0.99),
                hist.value_at_quantile(0.999),
                hist.max(),
                hist.mean(),
            )
            .expect("String write is infallible");
        }
        csv
    }

    /// Export all node histograms to an HdrHistogram interval log.
    ///
    /// Each node gets its own tagged interval in the log file.
    ///
    /// # Panics
    ///
    /// Panics if in-memory histogram serialization fails (unreachable: buffers are always valid).
    #[expect(
        clippy::expect_used,
        reason = "HdrHistogram writer/serialization uses in-memory buffers that cannot fail; UTF-8 is guaranteed from ASCII content"
    )]
    pub fn to_hdr_log(&self) -> String {
        let mut buf = Vec::new();
        let mut serializer = V2Serializer::new();

        let start_time = std::time::UNIX_EPOCH;

        let mut writer_builder = IntervalLogWriterBuilder::new();
        writer_builder
            .with_start_time(start_time)
            .with_base_time(start_time)
            .add_comment("WAFER per-node latency histograms (nanoseconds)");

        let mut log_writer =
            writer_builder.begin_log_with(&mut buf, &mut serializer).expect("begin interval log");

        let mut entries: Vec<(&String, &Histogram<u64>)> = self.histograms.iter().collect();
        entries.sort_by_key(|(id, _)| *id);

        for (id, hist) in entries {
            if hist.is_empty() {
                continue;
            }
            log_writer
                .write_histogram(
                    hist,
                    std::time::Duration::ZERO,
                    std::time::Duration::from_secs(1), // placeholder duration
                    hdrhistogram::serialization::interval_log::Tag::new(id),
                )
                .expect("write histogram");
        }

        String::from_utf8(buf).expect("valid UTF-8")
    }

    /// Export to a directory.
    ///
    /// Creates:
    /// - `per_node_latency.csv` — summary statistics per node
    /// - `per_node_latency.hdr` — full histograms in interval log format
    pub fn export_to_dir(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;

        let csv_content = self.to_csv();
        let mut csv_file = std::fs::File::create(dir.join("per_node_latency.csv"))?;
        csv_file.write_all(csv_content.as_bytes())?;

        let hdr_content = self.to_hdr_log();
        let mut hdr_file = std::fs::File::create(dir.join("per_node_latency.hdr"))?;
        hdr_file.write_all(hdr_content.as_bytes())?;

        Ok(())
    }
}

impl Default for NodeLatencyRecorder {
    fn default() -> Self {
        Self::new(&[])
    }
}

#[cfg(test)]
#[expect(
    clippy::let_underscore_must_use,
    reason = "test cleanup: fs::remove_dir_all failure is benign"
)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_histograms_for_all_nodes() {
        let recorder = NodeLatencyRecorder::new(&["t1", "t2", "t3"]);
        assert_eq!(recorder.len(), 3);
        assert!(!recorder.is_empty());
        assert!(recorder.get("t1").is_some());
        assert!(recorder.get("t2").is_some());
        assert!(recorder.get("t3").is_some());
        assert!(recorder.get("unknown").is_none());
    }

    #[test]
    fn record_accumulates_values() {
        let mut recorder = NodeLatencyRecorder::new(&["node-a"]);

        for _ in 0..100 {
            recorder.record("node-a", 5_000); // 5µs
        }

        let hist = recorder.get("node-a").unwrap();
        assert_eq!(hist.len(), 100);
        // HdrHistogram quantizes: 5000 → nearest equivalent value (within 0.1%)
        assert!(hist.min() > 0);
        assert!(hist.max() <= 6_000);
    }

    #[test]
    fn record_clamps_below_minimum() {
        let mut recorder = NodeLatencyRecorder::new(&["fast"]);
        recorder.record("fast", 500); // 500ns < 1µs minimum

        let hist = recorder.get("fast").unwrap();
        assert_eq!(hist.len(), 1);
        // Value was clamped to 1_000 before recording;
        // HdrHistogram quantization may store a nearby equivalent value
        assert!(hist.min() <= 1_100, "min should be near 1000, got {}", hist.min());
    }

    #[test]
    fn record_ignores_unknown_node() {
        let mut recorder = NodeLatencyRecorder::new(&["known"]);
        recorder.record("unknown", 5_000); // Should not panic
        assert!(recorder.get("unknown").is_none());
    }

    #[test]
    fn to_csv_format() {
        let mut recorder = NodeLatencyRecorder::new(&["t1", "t2"]);

        for _ in 0..50 {
            recorder.record("t1", 10_000); // 10µs
        }
        for _ in 0..30 {
            recorder.record("t2", 50_000); // 50µs
        }

        let csv = recorder.to_csv();
        let lines: Vec<&str> = csv.lines().collect();

        assert_eq!(lines[0], "node_id,count,min_ns,p50_ns,p99_ns,p999_ns,max_ns,mean_ns");
        assert!(lines.len() >= 3); // header + 2 data rows

        // Verify t1 row exists with count=50
        let t1_line = lines.iter().find(|l| l.starts_with("t1,")).unwrap();
        let parts: Vec<&str> = t1_line.split(',').collect();
        assert_eq!(parts[1], "50"); // count
    }

    #[test]
    fn to_csv_skips_empty_histograms() {
        let recorder = NodeLatencyRecorder::new(&["empty"]);
        let csv = recorder.to_csv();
        // Only header — no data rows for empty histograms
        assert_eq!(csv.lines().count(), 1);
    }

    #[test]
    fn to_hdr_log_produces_valid_format() {
        let mut recorder = NodeLatencyRecorder::new(&["t1"]);
        for _ in 0..20 {
            recorder.record("t1", 8_000);
        }

        let hdr = recorder.to_hdr_log();
        assert!(hdr.contains("#[StartTime"));
        assert!(hdr.contains("per-node latency"));
        assert!(hdr.lines().any(|l| l.starts_with("Tag=t1,")));
    }

    #[test]
    fn export_to_dir_creates_files() {
        let mut recorder = NodeLatencyRecorder::new(&["n1", "n2"]);
        for _ in 0..10 {
            recorder.record("n1", 20_000);
            recorder.record("n2", 100_000);
        }

        let tmp_dir =
            std::env::temp_dir().join(format!("wafer-node-latency-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp_dir);

        recorder.export_to_dir(&tmp_dir).unwrap();

        assert!(tmp_dir.join("per_node_latency.csv").exists());
        assert!(tmp_dir.join("per_node_latency.hdr").exists());

        let csv = std::fs::read_to_string(tmp_dir.join("per_node_latency.csv")).unwrap();
        assert!(csv.contains("n1,"));
        assert!(csv.contains("n2,"));

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn default_is_empty() {
        let recorder = NodeLatencyRecorder::default();
        assert!(recorder.is_empty());
        assert_eq!(recorder.len(), 0);
    }

    #[test]
    fn multiple_nodes_independent() {
        let mut recorder = NodeLatencyRecorder::new(&["a", "b"]);

        recorder.record("a", 5_000);
        recorder.record("b", 50_000);

        let a = recorder.get("a").unwrap();
        let b = recorder.get("b").unwrap();

        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        // Different orders of magnitude — a should be much smaller than b
        assert!(a.min() < 10_000);
        assert!(b.min() > 40_000);
    }
}
