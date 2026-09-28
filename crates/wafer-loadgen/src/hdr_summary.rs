//! `hdr-summary` — read a `latency.hdr` and emit p50/p95/p99/p999 as JSON.
//!
//! Reads both encodings: the interval log `BenchSink` writes and the bare V2
//! histogram `wafer-loadgen subscribe` writes.
//!
//! Bypasses the Python `hdrh` library (its V2-cookie handling is
//! incompatible with the Rust `hdrhistogram` crate's serialiser); the
//! shakedown runner + canonical-run notebooks call this instead.
//! See RFC-008 §D9.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use clap::Args;
use hdrhistogram::Histogram;
use hdrhistogram::serialization::Deserializer;
use hdrhistogram::serialization::interval_log::{IntervalLogIterator, LogEntry};
use serde::Serialize;

#[derive(Args, Debug)]
pub struct HdrSummaryArgs {
    /// Path to a `latency.hdr` written by `BenchSink` (interval log) or
    /// `wafer-loadgen subscribe` (bare V2 histogram).
    #[arg(long, value_name = "PATH")]
    pub hdr: PathBuf,

    /// Emit the JSON summary to this path. Prints to stdout when omitted.
    #[arg(long, value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Pretty-print the JSON output (adds ~10% file size but readable in
    /// text editors).
    #[arg(long)]
    pub pretty: bool,
}

/// JSON schema emitted by `hdr-summary`. `_ns` suffixed fields are
/// nanoseconds. `total_count` counts recorded (post-warmup) values;
/// `intervals_read` is the number of interval-log entries aggregated (0 for a
/// bare V2 histogram).
#[derive(Debug, Serialize)]
struct Summary {
    hdr_path: String,
    intervals_read: usize,
    total_count: u64,
    min_ns: u64,
    max_ns: u64,
    mean_ns: f64,
    stdev_ns: f64,
    p50_ns: u64,
    p90_ns: u64,
    p95_ns: u64,
    p99_ns: u64,
    p999_ns: u64,
    p9999_ns: u64,
}

/// # Errors
///
/// Returns error if the HDR file cannot be read or parsed.
pub fn run(args: &HdrSummaryArgs) -> Result<()> {
    let bytes = std::fs::read(&args.hdr)
        .with_context(|| format!("failed to read {}", args.hdr.display()))?;
    let (agg, intervals) = read_latency_hdr(&bytes)?;
    write_summary(
        &summarize(args.hdr.display().to_string(), &agg, intervals),
        args.output.as_deref(),
        args.pretty,
    )
}

/// The first bytes of every V2 histogram; the fourth varies with the word size.
const V2_COOKIE_PREFIX: [u8; 3] = [0x1c, 0x84, 0x93];

/// Aggregates a `latency.hdr` into one histogram, returning it with the
/// number of interval-log entries read.
fn read_latency_hdr(bytes: &[u8]) -> Result<(Histogram<u64>, usize)> {
    let mut agg = Histogram::<u64>::new_with_bounds(
        wafer_types::latency::LATENCY_LOWEST_NS,
        wafer_types::latency::LATENCY_HIGHEST_NS,
        wafer_types::latency::LATENCY_SIG_DIGITS,
    )
    .map_err(|e| anyhow!("failed to create aggregator histogram: {e:?}"))?;
    let mut deserializer = Deserializer::new();

    if bytes.starts_with(&V2_COOKIE_PREFIX) {
        let h: Histogram<u64> = deserializer
            .deserialize(&mut std::io::Cursor::new(bytes))
            .context("failed to deserialise V2 histogram")?;
        agg.add(&h).map_err(|e| anyhow!("aggregation failed (bounds mismatch): {e:?}"))?;
        return Ok((agg, 0));
    }

    let mut intervals = 0_usize;
    for entry in IntervalLogIterator::new(bytes) {
        match entry {
            Ok(LogEntry::Interval(hist_line)) => {
                let encoded = hist_line.encoded_histogram();
                let raw = base64_decode(encoded).with_context(|| {
                    format!("failed to base64-decode interval entry: {encoded}")
                })?;
                let mut cursor = std::io::Cursor::new(raw);
                let h: Histogram<u64> =
                    deserializer.deserialize(&mut cursor).with_context(|| {
                        "failed to deserialise interval histogram — V2 cookie mismatch?".to_string()
                    })?;
                agg.add(&h).map_err(|e| anyhow!("aggregation failed (bounds mismatch): {e:?}"))?;
                intervals = intervals.saturating_add(1);
            }
            Ok(LogEntry::BaseTime(_) | LogEntry::StartTime(_)) => {}
            Err(e) => return Err(anyhow!("interval-log parse error: {e:?}")),
        }
    }
    Ok((agg, intervals))
}

fn summarize(hdr_path: String, agg: &Histogram<u64>, intervals: usize) -> Summary {
    if agg.is_empty() {
        // Empty run — still emit a valid summary so downstream tooling
        // never sees `null`. Callers must gate on `total_count`.
        return Summary {
            hdr_path,
            intervals_read: intervals,
            total_count: 0,
            min_ns: 0,
            max_ns: 0,
            mean_ns: 0.0,
            stdev_ns: 0.0,
            p50_ns: 0,
            p90_ns: 0,
            p95_ns: 0,
            p99_ns: 0,
            p999_ns: 0,
            p9999_ns: 0,
        };
    }

    Summary {
        hdr_path,
        intervals_read: intervals,
        total_count: agg.len(),
        min_ns: agg.min(),
        max_ns: agg.max(),
        mean_ns: agg.mean(),
        stdev_ns: agg.stdev(),
        p50_ns: agg.value_at_quantile(0.50),
        p90_ns: agg.value_at_quantile(0.90),
        p95_ns: agg.value_at_quantile(0.95),
        p99_ns: agg.value_at_quantile(0.99),
        p999_ns: agg.value_at_quantile(0.999),
        p9999_ns: agg.value_at_quantile(0.9999),
    }
}

fn write_summary(summary: &Summary, output: Option<&std::path::Path>, pretty: bool) -> Result<()> {
    let json = if pretty {
        serde_json::to_string_pretty(summary)?
    } else {
        serde_json::to_string(summary)?
    };
    if let Some(path) = output {
        std::fs::write(path, &json)
            .with_context(|| format!("failed to write summary to {}", path.display()))?;
    } else {
        #[expect(
            clippy::print_stdout,
            reason = "CLI tool — stdout is the default output destination when no --output path given"
        )]
        {
            println!("{json}");
        }
    }
    Ok(())
}

/// Minimal STANDARD base64 decoder — avoids a workspace-dep on
/// `base64` here; the Rust `HdrHistogram` serialiser emits STANDARD.
fn base64_decode(s: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    STANDARD.decode(s.trim()).map_err(|e| anyhow!("base64 decode failed: {e}"))
}

#[cfg(test)]
mod tests {
    use hdrhistogram::serialization::V2Serializer;
    use hdrhistogram::serialization::interval_log::{IntervalLogWriterBuilder, Tag};

    use super::*;
    use crate::recorder::LatencyRecorder;

    fn summarize_file(bytes: &[u8]) -> serde_json::Value {
        let dir = tempfile::tempdir().unwrap();
        let hdr = dir.path().join("latency.hdr");
        let output = dir.path().join("summary.json");
        std::fs::write(&hdr, bytes).unwrap();
        run(&HdrSummaryArgs { hdr, output: Some(output.clone()), pretty: false }).unwrap();
        serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap()
    }

    #[test]
    fn reads_the_subscriber_file_and_the_bench_sink_file_alike() {
        let mut recorder = LatencyRecorder::new();
        let mut histogram = Histogram::<u64>::new_with_bounds(
            wafer_types::latency::LATENCY_LOWEST_NS,
            wafer_types::latency::LATENCY_HIGHEST_NS,
            wafer_types::latency::LATENCY_SIG_DIGITS,
        )
        .unwrap();
        for seq in 0..1_000 {
            let latency_ns = 50_000 + seq * 1_000;
            recorder.record(0, latency_ns, seq);
            histogram.record(latency_ns).unwrap();
        }

        let mut interval_log = Vec::new();
        let mut serializer = V2Serializer::new();
        IntervalLogWriterBuilder::new()
            .begin_log_with(&mut interval_log, &mut serializer)
            .unwrap()
            .write_histogram(
                &histogram,
                std::time::Duration::ZERO,
                std::time::Duration::from_secs(1),
                Tag::new("latency_ns"),
            )
            .unwrap();

        let subscriber = summarize_file(&recorder.serialize_v2().unwrap());
        let bench_sink = summarize_file(&interval_log);

        assert_eq!(subscriber["total_count"], 1_000);
        assert_eq!(subscriber["intervals_read"], 0);
        assert_eq!(bench_sink["intervals_read"], 1);
        for field in ["total_count", "min_ns", "max_ns", "p50_ns", "p99_ns", "p9999_ns"] {
            assert_eq!(subscriber[field], bench_sink[field], "{field}");
        }
    }
}
