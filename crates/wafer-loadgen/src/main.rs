//! WAFER Evaluation: MQTT load generator CLI.
//!
//! Two subcommands:
//! - `publish`    — pre-refactor publisher behaviour (see `publish.rs`).
//! - `subscribe`  — MQTT subscriber + [`HdrHistogram`] sink (P0.1, see `sub.rs`).
//!
//! Publisher flags are preserved verbatim from the pre-refactor binary.
//! Subscribe is a new subcommand.
//!
//! See docs/rfcs/RFC-008-evaluation-harness.md — Session 8 D3B / D4 / D9.

use clap::{Parser, Subcommand};

use wafer_loadgen::{
    HdrSummaryArgs, PublishArgs, SubscribeArgs, run_hdr_summary, run_publisher, run_subscriber,
};

#[derive(Parser, Debug)]
#[command(name = "wafer-loadgen", about = "MQTT load generator + subscriber for WAFER evaluation")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Publish JSON telemetry to an MQTT topic (open-loop rate control).
    Publish(PublishArgs),
    /// Subscribe to an MQTT topic and record end-to-end latency into
    /// latency.hdr + sequence.csv + subscriber-metadata.json.
    Subscribe(SubscribeArgs),
    /// Read a `latency.hdr` interval log and emit p50/p95/p99/p999 as JSON.
    ///
    /// Used by the E-Perf-4 shakedown runner and the canonical-runs
    /// analysis notebooks (RFC-008 §D9). Bypasses the Python `hdrh`
    /// library, which mis-decodes the V2 cookie the Rust `hdrhistogram`
    /// crate emits.
    HdrSummary(HdrSummaryArgs),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    match cli.command {
        Command::Publish(args) => {
            run_publisher(args).await?;
        }
        Command::Subscribe(args) => {
            run_subscriber(args).await?;
        }
        Command::HdrSummary(args) => {
            run_hdr_summary(&args)?;
        }
    }
    Ok(())
}
