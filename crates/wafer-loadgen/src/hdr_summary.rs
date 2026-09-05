//! `hdr-summary` — read a `latency.hdr` interval log and emit
//! p50/p95/p99/p999 as JSON.
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
    /// Path to a `latency.hdr` interval log (as written by `BenchSink` or
    /// `wafer-loadgen subscribe`).
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
/// `intervals_read` is the number of interval-log entries aggregated.
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
/// Returns error if the HDR file cannot be read, parsed, or is not valid UTF-8.
pub fn run(args: &HdrSummaryArgs) -> Result<()> {
    let bytes = std::fs::read(&args.hdr)
        .with_context(|| format!("failed to read {}", args.hdr.display()))?;
    let text = std::str::from_utf8(&bytes)
        .context("latency.hdr is not UTF-8 (interval logs are text-framed)")?;

    // Aggregate every interval entry into one histogram so tail
    // percentiles reflect the whole post-warmup run. Bounds match the
    // BenchSink + loadgen ceiling (1µs–10s).
    let mut agg = Histogram::<u64>::new_with_bounds(1_000, 10_000_000_000, 3)
        .map_err(|e| anyhow!("failed to create aggregator histogram: {e:?}"))?;
    let mut deserializer = Deserializer::new();
    let mut intervals = 0_usize;

    for entry in IntervalLogIterator::new(text.as_bytes()) {
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

    if agg.is_empty() {
        // Empty run — still emit a valid summary so downstream tooling
        // never sees `null`. Callers must gate on `total_count`.
        let summary = Summary {
            hdr_path: args.hdr.display().to_string(),
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
        write_summary(&summary, args.output.as_deref(), args.pretty)?;
        return Ok(());
    }

    let summary = Summary {
        hdr_path: args.hdr.display().to_string(),
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
    };
    write_summary(&summary, args.output.as_deref(), args.pretty)
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
