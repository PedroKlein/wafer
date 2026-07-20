#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! WAFER Runtime — WebAssembly Flow Execution Runtime binary.
//!
//! Loads pipeline config, launches all nodes, runs until completion or signal.
//! Supports timed hot-swap triggers for benchmark evaluation (RQ3).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use tokio::signal;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use wafer_config::{load_config, validate};
use wafer_core::api::{ApiConfig as CoreApiConfig, ApiServer, MetricsServer, MetricsServerConfig};
use wafer_core::engine::Capabilities;
use wafer_core::orchestrator::hotswap::prepare_transform_swap_timed;
use wafer_core::orchestrator::launch_pipeline;
use wafer_core::orchestrator::PipelineOrchestrator;

/// Log output format.
#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum LogFormat {
    /// Pretty-printed text format (default)
    #[default]
    Pretty,
    /// JSON structured logging
    Json,
}

/// WAFER Runtime — WebAssembly Flow Execution Runtime
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

    /// Log output format
    #[arg(long, value_enum, default_value_t = LogFormat::Pretty)]
    log_format: LogFormat,

    /// Trigger hot-swap after N seconds (benchmark mode, RQ3 evaluation)
    #[arg(long, value_name = "SECS")]
    swap_after_secs: Option<u64>,

    /// Node ID to hot-swap (requires --swap-after-secs)
    #[arg(long, value_name = "ID")]
    swap_node: Option<String>,

    /// Path to replacement .wasm plugin (requires --swap-after-secs)
    #[arg(long, value_name = "PATH")]
    swap_plugin: Option<PathBuf>,

    /// Directory to write swap timeline JSON output
    #[arg(long, value_name = "DIR")]
    swap_output_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize tracing
    let env_filter = EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into());
    match args.log_format {
        LogFormat::Pretty => {
            tracing_subscriber::registry().with(fmt::layer()).with(env_filter).init();
        }
        LogFormat::Json => {
            tracing_subscriber::registry().with(fmt::layer().json()).with(env_filter).init();
        }
    }

    info!("WAFER Runtime starting...");
    info!(config = %args.config.display(), "Loading configuration");

    let config = load_config(&args.config).context("Failed to load configuration")?;
    validate(&config).map_err(|errors| {
        let messages = errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        anyhow::anyhow!("configuration validation failed: {messages}")
    })?;

    let pipeline_name = config
        .pipeline
        .as_ref()
        .and_then(|pipeline| pipeline.name.as_deref())
        .unwrap_or("wafer-pipeline");
    info!(pipeline = %pipeline_name, "Configuration loaded");

    // Launch pipeline (engine, plugins, sources, sinks, topology)
    let mut orchestrator = launch_pipeline(config, Some(&args.config))
        .await
        .context("Failed to launch pipeline")?;

    info!(
        tasks = orchestrator.task_count(),
        wasm_nodes = orchestrator.wasm_node_count(),
        "Pipeline running"
    );

    let control_plane_tasks = launch_control_plane(&args, &orchestrator).await?;

    // Spawn timed swap trigger if configured (RQ3 benchmark mode)
    if let Some(delay_secs) = args.swap_after_secs {
        let node_id = args.swap_node.clone().context(
            "--swap-after-secs requires --swap-node <ID>"
        )?;
        let plugin_path = args.swap_plugin.clone().context(
            "--swap-after-secs requires --swap-plugin <PATH>"
        )?;

        if !orchestrator.swappable_nodes().contains(&node_id.as_str()) {
            anyhow::bail!("--swap-node '{node_id}' is not a swappable Wasm node");
        }

        let output_dir = args.swap_output_dir.clone();
        let engine = Arc::clone(orchestrator.engine());
        let cancel = orchestrator.cancel_token().clone();

        // Use a oneshot to pass the prepared swap payload back to main
        let (tx, rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            tokio::select! {
                biased;
                () = cancel.cancelled() => return,
                () = tokio::time::sleep(Duration::from_secs(delay_secs)) => {}
            }

            info!(node = %node_id, delay_secs, "Timed swap trigger firing");

            let wasm_bytes = match tokio::fs::read(&plugin_path).await {
                Ok(b) => b,
                Err(e) => {
                    error!(path = %plugin_path.display(), error = %e, "Failed to read swap plugin");
                    return;
                }
            };

            let (progress, _completion_rx) = wafer_core::runner::HotSwapProgress::channel();
            // CLI-driven prepare-only smoke path: use the default transform memory limit.
            let result = prepare_transform_swap_timed(
                &engine,
                &wasm_bytes,
                &node_id,
                Capabilities::sandbox(),
                64 * 1024 * 1024,
                progress,
            ).await;

            match result {
                Ok(timed) => {
                    info!(
                        node = %node_id,
                        compile_ns = ?timed.timeline.compile_duration_ns(),
                        instantiate_ns = ?timed.timeline.instantiate_duration_ns(),
                        "Swap prepared"
                    );

                    // Write timeline
                    if let Some(ref dir) = output_dir {
                        let _ = tokio::fs::create_dir_all(dir).await;
                        let path = dir.join(format!("swap-timeline-{node_id}.json"));
                        match tokio::fs::write(&path, timed.timeline.to_json()).await {
                            Ok(()) => info!(path = %path.display(), "Swap timeline written"),
                            Err(e) => warn!(error = %e, "Failed to write timeline"),
                        }
                    }

                    let _ = tx.send((node_id, timed.payload));
                }
                Err(e) => error!(error = %e, "Swap preparation failed"),
            }
        });

        // Receive payload and dispatch swap within the run loop
        let cancel = orchestrator.cancel_token().clone();
        tokio::spawn(async move {
            shutdown_signal().await;
            info!("Shutdown signal received");
            cancel.cancel();
        });

        // Custom run loop that also handles swap delivery
        run_with_swap(&mut orchestrator, rx).await;
        orchestrator.cancel();
        wait_control_plane(control_plane_tasks).await;

        info!("WAFER Runtime stopped");
        return Ok(());
    }

    // Standard mode: no swap trigger
    let cancel = orchestrator.cancel_token().clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        info!("Shutdown signal received");
        cancel.cancel();
    });

    match orchestrator.run_until_complete().await {
        Ok(()) => info!("Pipeline completed"),
        Err(e) => error!(error = %e, "Pipeline exited with error"),
    }

    orchestrator.cancel();
    wait_control_plane(control_plane_tasks).await;

    info!("WAFER Runtime stopped");
    Ok(())
}

/// Run the pipeline while also watching for a swap payload delivery.
///
/// This integrates the timed swap trigger into the pipeline run loop.
/// When the swap payload arrives, it's dispatched to the target node
/// via `send_swap()`, then we continue waiting for pipeline completion.
async fn run_with_swap(
    orchestrator: &mut PipelineOrchestrator,
    rx: tokio::sync::oneshot::Receiver<(String, wafer_core::runner::SwapPayload)>,
) {
    // We can't use run_until_complete directly because we need to interleave
    // with the swap oneshot. Instead, replicate the logic with an additional arm.
    let mut swap_rx = Some(rx);
    let cancel = orchestrator.cancel_token().clone();

    loop {
        if let Some(rx) = swap_rx.take() {
            tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                result = rx => {
                    if let Ok((node_id, payload)) = result {
                        match orchestrator.send_swap(&node_id, payload) {
                            Ok(()) => info!(node = %node_id, "Hot-swap dispatched"),
                            Err(e) => error!(error = %e, "Hot-swap dispatch failed"),
                        }
                    }
                    // After swap dispatched, fall through to run_until_complete
                }
            }
        }

        // Now just run until complete
        match orchestrator.run_until_complete().await {
            Ok(()) => info!("Pipeline completed"),
            Err(e) => error!(error = %e, "Pipeline exited with error"),
        }
        break;
    }
}

async fn launch_control_plane(
    args: &Args,
    orchestrator: &PipelineOrchestrator,
) -> Result<Vec<JoinHandle<()>>> {
    let mut tasks = Vec::new();
    let handle = Arc::new(orchestrator.handle());

    let api_config = orchestrator.config().api.clone().unwrap_or_default();
    let metrics_config = orchestrator.config().metrics.clone().unwrap_or_default();
    let metrics_enabled = metrics_config.enabled;

    if api_config.enabled && !args.no_api {
        let bind = match args.api_bind {
            Some(bind) => bind,
            None => api_config
                .bind
                .parse::<SocketAddr>()
                .with_context(|| format!("invalid api bind address '{}'", api_config.bind))?,
        };

        let serve_metrics = metrics_enabled && args.metrics_bind.is_none();
        let server = ApiServer::new(
            CoreApiConfig { bind, serve_metrics },
            Arc::clone(&handle),
        )
        .await
        .with_context(|| format!("failed to bind API server at {bind}"))?;
        let local_addr = server.local_addr().context("failed to read API server bind address")?;
        info!(addr = %local_addr, serve_metrics, "HTTP API server listening");

        let cancel = orchestrator.cancel_token().clone();
        tasks.push(tokio::spawn(async move {
            if let Err(error) = server
                .run_with_shutdown(async move { cancel.cancelled().await })
                .await
            {
                error!(%error, "HTTP API server exited with error");
            }
        }));
    }

    if metrics_enabled && let Some(bind) = args.metrics_bind {
        let server = MetricsServer::new(
            MetricsServerConfig { bind, path: metrics_config.path },
            handle,
        )
        .await
        .with_context(|| format!("failed to bind metrics server at {bind}"))?;
        let local_addr = server.local_addr().context("failed to read metrics server bind address")?;
        info!(addr = %local_addr, "metrics server listening");

        let cancel = orchestrator.cancel_token().clone();
        tasks.push(tokio::spawn(async move {
            if let Err(error) = server
                .run_with_shutdown(async move { cancel.cancelled().await })
                .await
            {
                error!(%error, "metrics server exited with error");
            }
        }));
    }

    Ok(tasks)
}

async fn wait_control_plane(tasks: Vec<JoinHandle<()>>) {
    for task in tasks {
        if let Err(error) = task.await {
            error!(%error, "control-plane task panicked");
        }
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c().await.expect("Failed to install Ctrl+C handler");
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
        () = ctrl_c => {},
        () = terminate => {},
    }
}
