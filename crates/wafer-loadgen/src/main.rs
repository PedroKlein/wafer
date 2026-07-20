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

use wafer_loadgen::{run_publisher, run_subscriber, PublishArgs, SubscribeArgs};

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
    }
    Ok(())
}
