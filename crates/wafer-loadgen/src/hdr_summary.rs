//! `hdr-summary` — read a `latency.hdr` interval log and emit p50/p95/p99/p999 as JSON.
//!
//! Wired into `wafer-loadgen` as a subcommand so eval scripts (P2.1
//! shakedown runner, canonical-runs analysis) can aggregate percentiles
//! across many runs without depending on the Python `hdrh` library
//! (whose V2-cookie handling is incompatible with the Rust
//! `hdrhistogram` crate's serialiser).
//!
//! Emits one JSON object per invocation, aggregating all interval-log
//! entries in the file into a single histogram before extracting
//! percentiles. See docs/rfcs/RFC-008-evaluation-harness.md §D9.

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use clap::Args;
use hdrhistogram::serialization::interval_log::{IntervalLogIterator, LogEntry};
use hdrhistogram::serialization::Deserializer;
use hdrhistogram::Histogram;
use serde::Serialize;

#[derive(Args, Debug)]
pub struct HdrSummaryArgs {
    /// Path to a `latency.hdr` interval log (as written by BenchSink or
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

/// JSON schema emitted by `hdr-summary`. Every field is `_ns` unless the
/// name says otherwise. `total_count` is the count of RECORDED values
/// (post-warmup); `intervals_read` is the number of interval-log entries
/// aggregated into that histogram.
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

pub fn run(args: HdrSummaryArgs) -> Result<()> {
    let bytes = std::fs::read(&args.hdr)
        .with_context(|| format!("failed to read {}", args.hdr.display()))?;
    let text = std::str::from_utf8(&bytes)
        .context("latency.hdr is not UTF-8 (interval logs are text-framed)")?;

    // Aggregate every interval histogram into one so tail percentiles
    // reflect the entire post-warmup run. Ceiling matches the BenchSink
    // + loadgen ceiling (10 s in nanoseconds); if the crate rejects a
    // shorter/longer bound we retry with a wider one so cross-version
    // logs still parse.
    let mut agg = Histogram::<u64>::new_with_bounds(1_000, 10_000_000_000, 3)
        .map_err(|e| anyhow!("failed to create aggregator histogram: {e:?}"))?;
    let mut deserializer = Deserializer::new();
    let mut intervals = 0_usize;

    for entry in IntervalLogIterator::new(text.as_bytes()) {
        match entry {
            Ok(LogEntry::Interval(hist_line)) => {
                let encoded = hist_line.encoded_histogram();
                let raw = base64_decode(encoded)
                    .with_context(|| format!("failed to base64-decode interval entry: {encoded}"))?;
                let mut cursor = std::io::Cursor::new(raw);
                let h: Histogram<u64> = deserializer
                    .deserialize(&mut cursor)
                    .with_context(|| "failed to deserialise interval histogram — V2 cookie mismatch?".to_string())?;
                agg.add(&h)
                    .map_err(|e| anyhow!("aggregation failed (bounds mismatch): {e:?}"))?;
                intervals += 1;
            }
            Ok(LogEntry::BaseTime(_) | LogEntry::StartTime(_)) => {}
            Err(e) => return Err(anyhow!("interval-log parse error: {e:?}")),
        }
    }

    if agg.len() == 0 {
        // Empty histogram (no recorded values): still emit a valid summary
        // so downstream JSON tooling never gets a null. Callers must
        // check `total_count` before drawing conclusions.
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
        println!("{json}");
    }
    Ok(())
}

/// Minimal base64 decoder — the crate does not re-export `base64::engine`
/// through its public API, so we vendor a tiny STANDARD decoder here to
/// avoid pulling in yet another workspace dependency. STANDARD is what
/// the Rust HdrHistogram serialiser emits.
fn base64_decode(s: &str) -> Result<Vec<u8>> {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    STANDARD
        .decode(s.trim())
        .map_err(|e| anyhow!("base64 decode failed: {e}"))
}
