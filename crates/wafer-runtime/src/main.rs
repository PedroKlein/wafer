//! WAFER Runtime - Main entry point.
//!
//! This binary wraps wafer-core with HTTP API enabled by default.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use tokio::signal;
use tracing::{info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use wafer_core::config::loader::load_config;
use wafer_core::dag::PipelineOrchestrator;

/// WAFER Runtime - WebAssembly Flow Execution Runtime
#[derive(Parser, Debug)]
#[command(name = "wafer")]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to the pipeline configuration file
    #[arg(short, long)]
    config: PathBuf,

    /// API server bind address (overrides config)
    #[arg(long, value_name = "ADDR")]
    api_bind: Option<SocketAddr>,

    /// Disable the HTTP API server
    #[arg(long)]
    no_api: bool,

    /// Metrics server bind address (if different from API)
    #[arg(long, value_name = "ADDR")]
    metrics_bind: Option<SocketAddr>,

    /// Skip OCI registry cache for remote plugins
    #[arg(long)]
    no_cache: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .init();

    let args = Args::parse();

    info!("WAFER Runtime starting...");
    info!(config = %args.config.display(), "Loading configuration");

    // Load configuration
    let config = load_config(&args.config)
        .await
        .context("Failed to load configuration")?;

    let pipeline_name = config.pipeline.name.clone();
    info!(pipeline = %pipeline_name, "Configuration loaded");

    // Create the pipeline orchestrator wrapped in Arc for sharing with API server
    let orchestrator = std::sync::Arc::new(
        PipelineOrchestrator::from_config(config, !args.no_cache)
            .await
            .context("Failed to create pipeline orchestrator")?,
    );

    // TODO: HTTP API server integration
    // Now that run() takes &self (using internal mutability), we can share the
    // orchestrator via Arc with the API server. Full integration in follow-up change.
    if !args.no_api {
        let bind = args.api_bind.unwrap_or_else(|| "127.0.0.1:9090".parse().unwrap());
        warn!(address = %bind, "API server not yet integrated (coming in follow-up)");
        // Future: tokio::spawn(api_server::run(Arc::clone(&orchestrator), bind));
    }

    if args.metrics_bind.is_some() {
        warn!("Metrics server not yet integrated");
    }

    // Get cancel token for shutdown handling
    let cancel_token = orchestrator.cancel_token();

    // Spawn shutdown signal handler
    tokio::spawn(async move {
        shutdown_signal().await;
        info!("Shutdown signal received, cancelling pipeline...");
        cancel_token.cancel();
    });

    // Run pipeline until completion or cancellation
    // Note: run() now takes &self (not &mut self) thanks to internal mutability
    orchestrator.run().await.context("Pipeline execution failed")?;

    info!("WAFER Runtime stopped");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
