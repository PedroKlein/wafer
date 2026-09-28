//! WAFER Runtime — WebAssembly Flow Execution Runtime binary.
//!
//! Loads pipeline config, launches all nodes, runs until completion or signal.
//! Supports timed hot-swap triggers for benchmark evaluation (RQ3).
//!
//! # Exit status
//!
//! The evaluation harness treats any non-zero exit as a failed run, so the
//! code must never claim success for a run that failed:
//!
//! - `0`: the pipeline ran and every node task exited cleanly.
//! - `1`: startup failed for any other reason (engine, plugin load, control
//!   plane bind, output files).
//! - `2`: the configuration is invalid (also clap's code for bad arguments).
//! - `3`: the pipeline started but failed while running: a node task panicked,
//!   a source/sink `init()` failed, a source exhausted its poll-error budget,
//!   a Wasm node could not recover from a trap or was torn down by its error
//!   policy, the DLQ sink failed, or the timed hot-swap could not be prepared
//!   or dispatched. Bench artifacts are still flushed first.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use tokio::signal;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use wafer_config::{load_config, validate};
use wafer_core::api::{ApiConfig as CoreApiConfig, ApiServer, MetricsServer, MetricsServerConfig};
use wafer_core::bench::{MemoryRecorder, QueueDepthRecorder};
use wafer_core::config::{DeadLetterConfig, NodeDef};
use wafer_core::engine::Capabilities;
use wafer_core::error::WaferError;
use wafer_core::node::NodeKind;
use wafer_core::orchestrator::hotswap::prepare_transform_swap_timed_with_fuel;
use wafer_core::orchestrator::launch_pipeline_timed;
use wafer_core::orchestrator::{PipelineHandle, PipelineOrchestrator};

mod metadata;
mod startup;

/// Global handle to the background `MemoryRecorder` so `flush_bench_artifacts`
/// can retrieve samples after cancellation. Only populated when
/// `WAFER_BENCH_OUTPUT_DIR` is set.
static BENCH_RECORDER: std::sync::OnceLock<Arc<tokio::sync::Mutex<MemoryRecorder>>> =
    std::sync::OnceLock::new();
static QUEUE_DEPTH_RECORDER: std::sync::OnceLock<Arc<tokio::sync::Mutex<QueueDepthRecorder>>> =
    std::sync::OnceLock::new();

/// Exit code for startup failures other than an invalid configuration.
const EXIT_STARTUP_FAILED: u8 = 1;
/// Exit code for an invalid configuration (matches clap's usage-error code).
const EXIT_CONFIG_INVALID: u8 = 2;
/// Exit code for a pipeline that started but failed while running.
const EXIT_PIPELINE_FAILED: u8 = 3;

/// Error context marking a failure as an invalid configuration.
#[derive(Debug)]
struct ConfigInvalid;

impl std::fmt::Display for ConfigInvalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid configuration")
    }
}

/// Map a startup error to its exit code: configuration errors (from loading,
/// semantic validation, or source/sink `validate()` at launch) get
/// [`EXIT_CONFIG_INVALID`], everything else [`EXIT_STARTUP_FAILED`].
fn startup_exit_code(error: &anyhow::Error) -> u8 {
    let config_invalid = error.downcast_ref::<ConfigInvalid>().is_some()
        || error
            .chain()
            .any(|cause| matches!(cause.downcast_ref::<WaferError>(), Some(WaferError::Config(_))));
    if config_invalid { EXIT_CONFIG_INVALID } else { EXIT_STARTUP_FAILED }
}

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

/// Stamps process entry before the tokio runtime exists, then runs the async
/// boot sequence on the same multi-thread runtime `#[tokio::main]` builds.
#[expect(
    clippy::print_stderr,
    reason = "keeps the `Error: {e:?}` stderr report that returning `Err` from main used to print"
)]
fn main() -> ExitCode {
    let process_entry = startup::ProcessEntry::capture();
    let result = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Failed to build tokio runtime")
        .and_then(|runtime| runtime.block_on(Box::pin(run(process_entry))));
    match result {
        Ok(code) => code,
        Err(e) => {
            error!(error = format!("{e:#}"), "WAFER Runtime failed to start");
            eprintln!("Error: {e:?}");
            ExitCode::from(startup_exit_code(&e))
        }
    }
}

/// Boot the runtime, run the pipeline, and return the process exit code.
///
/// `Err` is a startup failure; a failure after the pipeline is running is
/// reported as `Ok(EXIT_PIPELINE_FAILED)` so artifacts are flushed first.
#[expect(
    clippy::too_many_lines,
    reason = "main() is the linear boot sequence: arg parsing, tracing init, config validation, orchestrator wiring, control-plane launch, shutdown handlers. Splitting into helpers obscures the boot order without adding testability."
)]
#[expect(
    clippy::let_underscore_must_use,
    reason = "fire-and-forget in spawned tasks: dir creation is best-effort; oneshot send may fail if receiver moved on"
)]
#[expect(
    clippy::large_futures,
    reason = "main() awaits launch_pipeline which holds WASM Store/Component; only one instance at startup"
)]
async fn run(process_entry: startup::ProcessEntry) -> Result<ExitCode> {
    let process_started = Instant::now();
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

    let mut config =
        load_config(&args.config).context("Failed to load configuration").context(ConfigInvalid)?;
    validate(&config)
        .map_err(|errors| {
            let messages = errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ");
            anyhow::anyhow!("configuration validation failed: {messages}")
        })
        .context(ConfigInvalid)?;

    let pipeline_name = config
        .pipeline
        .as_ref()
        .and_then(|pipeline| pipeline.name.as_deref())
        .unwrap_or("wafer-pipeline");
    info!(pipeline = %pipeline_name, "Configuration loaded");

    // A relative dead-letter file lands next to the other bench artefacts, so
    // one config serves every run.
    let bench_output_dir = std::env::var("WAFER_BENCH_OUTPUT_DIR").ok().map(PathBuf::from);
    if let (Some(dir), Some(DeadLetterConfig::File { path, .. })) =
        (&bench_output_dir, &mut config.dead_letter)
        && std::path::Path::new(path.as_str()).is_relative()
    {
        *path = dir.join(path.as_str()).to_string_lossy().into_owned();
    }

    // Check the timed-swap arguments before any node starts.
    let swap_plan = args
        .swap_after_secs
        .map(|delay_secs| SwapPlan::from_args(&args, &config, delay_secs))
        .transpose()
        .context(ConfigInvalid)?;

    // Resolved before launch so none of this runs inside `first_process`.
    let provenance = metadata::resolve_output_path().map(|path| ProvenanceSink {
        path,
        config_path: args.config.clone(),
        config: config.clone(),
    });
    let startup_output = startup::resolve_output_path();

    let launch_started = Instant::now();
    let launched = launch_pipeline_timed(config.clone(), Some(&args.config))
        .await
        .context("Failed to launch pipeline")?;
    let launch_completed = Instant::now();
    let (mut orchestrator, launch_timings) = launched.into_parts();

    info!(
        tasks = orchestrator.task_count(),
        wasm_nodes = orchestrator.wasm_node_count(),
        "Pipeline running"
    );

    // Emit runtime provenance BEFORE the run loop so the file exists even if
    // the runtime dies mid-run. The binary hash is deferred to the shutdown
    // write. The E-Perf-9 startup probe skips this write entirely: it runs
    // inside the `first_process` window, and the probe reaches shutdown.
    if let Some(sink) = &provenance
        && startup_output.is_none()
    {
        sink.write(&orchestrator.handle(), metadata::WrittenAt::Launch, None);
    }

    // Spawn memory sampler if WAFER_BENCH_OUTPUT_DIR is set (A19).
    let bench_cancel = tokio_util::sync::CancellationToken::new();
    let mut bench_tasks = Vec::new();
    if bench_output_dir.is_some() {
        let cancel = bench_cancel.clone();
        // The sampler task holds the lock for its whole life; the shared
        // handle lets flush_bench_artifacts read the samples once the task
        // has been cancelled and joined.
        let recorder = Arc::new(tokio::sync::Mutex::new(MemoryRecorder::new()));
        let rec_clone = Arc::clone(&recorder);
        bench_tasks.push(tokio::spawn(async move {
            rec_clone.lock().await.sample_loop(cancel).await;
        }));
        // Stash the handle so flush_bench_artifacts can retrieve samples.
        BENCH_RECORDER.get_or_init(|| recorder);
    }

    if std::env::var_os("WAFER_QUEUE_DEPTH_OUTPUT").is_some() {
        let queue_recorder =
            Arc::new(tokio::sync::Mutex::new(QueueDepthRecorder::new(orchestrator.handle())));
        let queue_clone = Arc::clone(&queue_recorder);
        let cancel = bench_cancel.clone();
        bench_tasks.push(tokio::spawn(async move {
            queue_clone.lock().await.sample_loop(cancel).await;
        }));
        QUEUE_DEPTH_RECORDER.get_or_init(|| queue_recorder);
    }

    let control_plane_tasks = launch_control_plane(&args, &orchestrator).await?;

    // Spawn timed swap trigger if configured (RQ3 benchmark mode)
    if let Some(SwapPlan {
        delay_secs,
        node_id,
        plugin_path,
        capabilities,
        memory_limit,
        node_fuel,
    }) = swap_plan
    {
        if !orchestrator.swappable_nodes().contains(&node_id.as_str()) {
            // Nodes are already running: stop them cleanly before reporting.
            if let Err(e) = orchestrator.shutdown().await {
                warn!(error = %e, "pipeline shutdown after a rejected --swap-node failed");
            }
            wait_control_plane(control_plane_tasks).await;
            return Err(anyhow::anyhow!("--swap-node '{node_id}' is not a swappable Wasm node")
                .context(ConfigInvalid));
        }

        let output_dir = args.swap_output_dir.clone();
        let engine = Arc::clone(orchestrator.engine());
        let fuel_limit = engine.fuel_budget(NodeKind::Transform, node_fuel);
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
                    let _ = tx.send(Err(format!(
                        "failed to read swap plugin {}: {e}",
                        plugin_path.display()
                    )));
                    return;
                }
            };

            let wasm_bytes: Arc<[u8]> = wasm_bytes.into();
            // Hashed alongside preparation so it cannot delay the dispatch.
            let plugin_hash = tokio::task::spawn_blocking({
                let wasm_bytes = Arc::clone(&wasm_bytes);
                move || wafer_core::registry::compute_hash(&wasm_bytes)
            });
            let (progress, completion) = wafer_core::runner::HotSwapProgress::channel();
            let result = prepare_transform_swap_timed_with_fuel(
                &engine,
                &wasm_bytes,
                &node_id,
                capabilities,
                memory_limit,
                fuel_limit,
                progress,
            )
            .await;

            match result {
                Ok(timed) => {
                    info!(
                        node = %node_id,
                        compile_ns = ?timed.timeline.compile_duration_ns(),
                        instantiate_ns = ?timed.timeline.instantiate_duration_ns(),
                        compile_cache = ?timed.timeline.compile_cache,
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

                    let plugin_hash = match plugin_hash.await {
                        Ok(hash) => hash,
                        Err(e) => {
                            let _ = tx.send(Err(format!("hashing swap plugin failed: {e}")));
                            return;
                        }
                    };
                    let _ = tx.send(Ok(PreparedSwap {
                        node_id,
                        payload: timed.payload,
                        plugin_hash,
                        completion,
                    }));
                }
                Err(e) => {
                    error!(error = %e, "Swap preparation failed");
                    let _ = tx.send(Err(format!("swap preparation for '{node_id}' failed: {e}")));
                }
            }
        });

        // Receive payload and dispatch swap within the run loop
        spawn_shutdown_signal_thread(orchestrator.cancel_token().clone())?;

        // Custom run loop that also handles swap delivery
        let run_result = run_with_swap(&mut orchestrator, rx, provenance.clone()).await;
        let binary_hash = spawn_binary_hash(provenance.as_ref());
        flush_bench_artifacts(&orchestrator, &bench_cancel, &mut bench_tasks).await;
        write_shutdown_provenance(&orchestrator, provenance.as_ref(), binary_hash).await;
        orchestrator.cancel();
        wait_control_plane(control_plane_tasks).await;

        info!("WAFER Runtime stopped");
        return Ok(pipeline_exit_code(&run_result));
    }

    // Standard mode: no swap trigger
    spawn_shutdown_signal_thread(orchestrator.cancel_token().clone())?;

    let run_result = orchestrator.run_until_complete().await.map_err(anyhow::Error::from);
    log_run_result(&run_result);

    let startup_written = startup_output.map_or(Ok(()), |path| {
        startup::write_startup(
            &path,
            &orchestrator,
            process_entry,
            process_started,
            launch_started,
            launch_completed,
            launch_timings,
        )
        .inspect(|()| info!(path = %path.display(), "Startup phases written"))
    });

    let binary_hash = spawn_binary_hash(provenance.as_ref());
    flush_bench_artifacts(&orchestrator, &bench_cancel, &mut bench_tasks).await;
    write_shutdown_provenance(&orchestrator, provenance.as_ref(), binary_hash).await;
    orchestrator.cancel();
    wait_control_plane(control_plane_tasks).await;

    info!("WAFER Runtime stopped");
    // A failed run outranks a failed startup.json write: report it as 3.
    if run_result.is_ok() {
        startup_written?;
    } else if let Err(e) = startup_written {
        error!(error = format!("{e:#}"), "startup phases write failed");
    }
    Ok(pipeline_exit_code(&run_result))
}

/// Timed hot-swap requested with `--swap-after-secs`, checked before launch.
struct SwapPlan {
    delay_secs: u64,
    node_id: String,
    plugin_path: PathBuf,
    capabilities: Capabilities,
    memory_limit: usize,
    node_fuel: Option<std::num::NonZeroU64>,
}

impl SwapPlan {
    fn from_args(
        args: &Args,
        config: &wafer_types::config::Config,
        delay_secs: u64,
    ) -> Result<Self> {
        let node_id =
            args.swap_node.clone().context("--swap-after-secs requires --swap-node <ID>")?;
        let plugin_path =
            args.swap_plugin.clone().context("--swap-after-secs requires --swap-plugin <PATH>")?;
        let Some(NodeDef::Transform(wasm)) = config.nodes.get(&node_id) else {
            anyhow::bail!("--swap-node '{node_id}' must name a Transform");
        };
        let capabilities = Capabilities::try_from(&wasm.capabilities)
            .context("invalid outbound HTTP capability")?;
        Ok(Self {
            delay_secs,
            memory_limit: wasm.memory_limit.unwrap_or(config.engine.memory.transform),
            node_fuel: wasm.fuel,
            node_id,
            plugin_path,
            capabilities,
        })
    }
}

fn log_run_result(run_result: &Result<()>) {
    match run_result {
        Ok(()) => info!("Pipeline completed"),
        Err(e) => error!(error = format!("{e:#}"), "Pipeline exited with error"),
    }
}

/// Exit code for a pipeline that got past startup.
fn pipeline_exit_code(run_result: &Result<()>) -> ExitCode {
    match run_result {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(EXIT_PIPELINE_FAILED),
    }
}

/// Where and from what the runtime writes `runtime-provenance.json`.
#[derive(Clone)]
struct ProvenanceSink {
    path: PathBuf,
    config_path: PathBuf,
    config: wafer_types::config::Config,
}

impl ProvenanceSink {
    fn write(
        &self,
        handle: &PipelineHandle,
        written_at: metadata::WrittenAt,
        runtime_sha256: Option<&str>,
    ) {
        match metadata::write_provenance(
            &self.path,
            handle,
            &self.config_path,
            &self.config,
            written_at,
            runtime_sha256,
        ) {
            Ok(()) => {
                info!(path = %self.path.display(), ?written_at, "Runtime provenance written");
            }
            Err(e) => {
                warn!(path = %self.path.display(), error = %e, "provenance write failed");
            }
        }
    }
}

/// A timed swap prepared off the run loop, ready to dispatch.
struct PreparedSwap {
    node_id: String,
    payload: wafer_core::runner::SwapPayload,
    plugin_hash: String,
    completion: tokio::sync::oneshot::Receiver<wafer_core::runner::HotSwapOutcome>,
}

/// Hash the runtime binary on the blocking pool once the run is over, so the
/// O(100 MB) read overlaps result export instead of any measured window.
fn spawn_binary_hash(
    provenance: Option<&ProvenanceSink>,
) -> Option<JoinHandle<std::io::Result<String>>> {
    provenance.map(|_| tokio::task::spawn_blocking(metadata::runtime_binary_sha256))
}

async fn write_shutdown_provenance(
    orchestrator: &PipelineOrchestrator,
    provenance: Option<&ProvenanceSink>,
    binary_hash: Option<JoinHandle<std::io::Result<String>>>,
) {
    let Some(sink) = provenance else {
        return;
    };
    let runtime_sha256 = match binary_hash {
        Some(task) => match task.await {
            Ok(Ok(hash)) => Some(hash),
            Ok(Err(error)) => {
                warn!(%error, "runtime binary hash failed");
                None
            }
            Err(error) => {
                warn!(%error, "runtime binary hash task failed");
                None
            }
        },
        None => None,
    };
    sink.write(&orchestrator.handle(), metadata::WrittenAt::Shutdown, runtime_sha256.as_deref());
}

/// Run the pipeline while also watching for a swap payload delivery.
///
/// This integrates the timed swap trigger into the pipeline run loop.
/// When the swap payload arrives, it's dispatched to the target node
/// via `send_swap()`, then we continue waiting for pipeline completion.
/// A swap that could not be prepared or dispatched fails the run: a
/// swap benchmark in which no swap happened is not a valid sample.
/// Once the runner adopts the replacement, provenance is re-written, so a
/// swap run names the live binary.
async fn run_with_swap(
    orchestrator: &mut PipelineOrchestrator,
    rx: tokio::sync::oneshot::Receiver<std::result::Result<PreparedSwap, String>>,
    provenance: Option<ProvenanceSink>,
) -> Result<()> {
    // Race the cancel signal against the swap oneshot. Either way we fall
    // through to run_until_complete, which drains on cancellation and reports
    // any node failure.
    let cancel = orchestrator.cancel_token().clone();
    let mut swap_failure = None;
    let mut adoption = None;
    let run_done = tokio_util::sync::CancellationToken::new();

    tokio::select! {
        biased;
        () = cancel.cancelled() => {}
        result = rx => match result {
            Ok(Ok(PreparedSwap { node_id, payload, plugin_hash, completion })) => {
                let handle = orchestrator.handle();
                let progress = payload.progress();
                handle.track_swap_plugin_hash(&node_id, &progress, plugin_hash);
                let dispatched = handle
                    .try_begin_swap(&node_id)
                    .and_then(|guard| handle.send_swap(&node_id, payload).map(|()| guard));
                match dispatched {
                    Ok(guard) => {
                        info!(node = %node_id, "Hot-swap dispatched");
                        let recorded = record_adopted_swap(
                            handle,
                            node_id,
                            progress,
                            completion,
                            provenance,
                            run_done.clone(),
                        );
                        adoption = Some(tokio::spawn(async move {
                            recorded.await;
                            drop(guard);
                        }));
                    }
                    Err(e) => {
                        error!(error = %e, "Hot-swap dispatch failed");
                        swap_failure =
                            Some(format!("hot-swap dispatch to '{node_id}' failed: {e}"));
                    }
                }
            }
            Ok(Err(e)) => swap_failure = Some(e),
            // The trigger task ended without a result. On cancel that is
            // expected; otherwise it panicked and no swap happened.
            Err(_) if cancel.is_cancelled() => {}
            Err(_) => {
                error!("Hot-swap trigger task ended without a result");
                swap_failure = Some("hot-swap trigger task ended without a result".to_owned());
            }
        }
    }

    let run_result = orchestrator.run_until_complete().await.map_err(anyhow::Error::from);
    log_run_result(&run_result);

    // The runners have exited, so any outcome they will ever report is already
    // in the channel. The swap payload (and with it the sender) stays alive in
    // the orchestrator, so tell the task to stop waiting instead of relying on
    // the channel closing. Awaiting keeps the swap write ahead of shutdown's.
    run_done.cancel();
    if let Some(task) = adoption
        && let Err(error) = task.await
    {
        warn!(%error, "swap provenance task failed");
    }

    match (swap_failure, run_result) {
        (None, run_result) => run_result,
        (Some(swap), Ok(())) => Err(anyhow::anyhow!(swap)),
        (Some(swap), Err(run)) => Err(run.context(swap)),
    }
}

/// Re-write provenance once the runner has adopted the replacement.
///
/// The runner records the replacement's hash when it adopts it and puts the
/// replaced hash back if it rolls back, so this only decides when to write.
/// The outcome report only arrives after the first message processed on the
/// replacement, so a node that stays idle until shutdown never sends one.
async fn record_adopted_swap(
    handle: PipelineHandle,
    node_id: String,
    progress: Arc<wafer_core::runner::HotSwapProgress>,
    completion: tokio::sync::oneshot::Receiver<wafer_core::runner::HotSwapOutcome>,
    provenance: Option<ProvenanceSink>,
    run_done: tokio_util::sync::CancellationToken,
) {
    let mut completion = completion;
    let outcome = tokio::select! {
        biased;
        outcome = &mut completion => outcome.ok(),
        () = run_done.cancelled() => completion.try_recv().ok(),
    };
    let adopted = match outcome {
        Some(Ok(_report)) => true,
        Some(Err(e)) => {
            warn!(node = %node_id, error = %e, "Hot-swap failed or rolled back; v1 hash kept");
            false
        }
        None => {
            let adopted = progress.replacement_adopted_at().is_some();
            if !adopted {
                warn!(node = %node_id, "Hot-swap not adopted before shutdown; v1 hash kept");
            }
            adopted
        }
    };
    if adopted && let Some(sink) = &provenance {
        sink.write(&handle, metadata::WrittenAt::Swap, None);
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
        let server = ApiServer::new(CoreApiConfig { bind, serve_metrics }, Arc::clone(&handle))
            .await
            .with_context(|| format!("failed to bind API server at {bind}"))?;
        let local_addr = server.local_addr().context("failed to read API server bind address")?;
        info!(addr = %local_addr, serve_metrics, "HTTP API server listening");

        let cancel = orchestrator.cancel_token().clone();
        tasks.push(tokio::spawn(async move {
            if let Err(error) =
                server.run_with_shutdown(async move { cancel.cancelled().await }).await
            {
                error!(%error, "HTTP API server exited with error");
            }
        }));
    }

    if metrics_enabled && let Some(bind) = args.metrics_bind {
        let server =
            MetricsServer::new(MetricsServerConfig { bind, path: metrics_config.path }, handle)
                .await
                .with_context(|| format!("failed to bind metrics server at {bind}"))?;
        let local_addr =
            server.local_addr().context("failed to read metrics server bind address")?;
        info!(addr = %local_addr, "metrics server listening");

        let cancel = orchestrator.cancel_token().clone();
        tasks.push(tokio::spawn(async move {
            if let Err(error) =
                server.run_with_shutdown(async move { cancel.cancelled().await }).await
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

/// Flush benchmark artifacts (memory.csv + `per_node_metrics.csv`) on graceful
/// shutdown when `WAFER_BENCH_OUTPUT_DIR` is set. Cancel-safe: fires the
/// sampler's cancellation token, then drains collected samples to disk.
async fn flush_bench_artifacts(
    orchestrator: &PipelineOrchestrator,
    bench_cancel: &tokio_util::sync::CancellationToken,
    bench_tasks: &mut Vec<JoinHandle<()>>,
) {
    let Some(dir) = std::env::var("WAFER_BENCH_OUTPUT_DIR").ok().map(PathBuf::from) else {
        return;
    };

    bench_cancel.cancel();
    while let Some(task) = bench_tasks.pop() {
        if let Err(error) = task.await {
            warn!(%error, "benchmark sampler task failed");
        }
    }

    // 2. Flush memory.csv.
    if let Some(recorder) = BENCH_RECORDER.get() {
        let (csv, samples, start_unix_epoch_ns) = {
            let guard = recorder.lock().await;
            (guard.to_csv(), guard.samples().len(), guard.start_unix_epoch_ns())
        };
        let path = dir.join("memory.csv");
        match std::fs::write(&path, csv) {
            Ok(()) => {
                info!(path = %path.display(), samples, "memory.csv written");
                if let Some(start_unix_epoch_ns) = start_unix_epoch_ns {
                    let clock_path = dir.join("memory-clock.json");
                    let clock = serde_json::json!({
                        "schema_version": 1,
                        "elapsed_clock": "monotonic",
                        "alignment_clock": "unix-epoch",
                        "alignment_clock_purpose": "cross-process-alignment-only",
                        "start_unix_epoch_ns": start_unix_epoch_ns,
                    });
                    match serde_json::to_string_pretty(&clock) {
                        Ok(encoded) => {
                            if let Err(error) = std::fs::write(&clock_path, format!("{encoded}\n"))
                            {
                                warn!(path = %clock_path.display(), %error, "failed to write memory clock");
                            }
                        }
                        Err(error) => warn!(%error, "failed to encode memory clock"),
                    }
                }
            }
            Err(e) => warn!(path = %path.display(), error = %e, "failed to write memory.csv"),
        }
    }

    if let (Some(recorder), Some(path)) = (
        QUEUE_DEPTH_RECORDER.get(),
        std::env::var_os("WAFER_QUEUE_DEPTH_OUTPUT").map(PathBuf::from),
    ) {
        let (csv, samples, truncated, start_unix_epoch_ns) = {
            let guard = recorder.lock().await;
            (guard.to_csv(), guard.samples().len(), guard.truncated(), guard.start_unix_epoch_ns())
        };
        match std::fs::write(&path, csv) {
            Ok(()) => {
                info!(path = %path.display(), samples, truncated, "queue-depth.csv written");
                let clock_path = dir.join("queue-depth-clock.json");
                let clock = serde_json::json!({
                    "schema_version": 1,
                    "elapsed_clock": "monotonic",
                    "alignment_clock": "unix-epoch",
                    "alignment_clock_purpose": "cross-process-alignment-only",
                    "start_unix_epoch_ns": start_unix_epoch_ns,
                });
                match serde_json::to_string_pretty(&clock) {
                    Ok(encoded) => {
                        if let Err(error) = std::fs::write(&clock_path, format!("{encoded}\n")) {
                            warn!(path = %clock_path.display(), %error, "failed to write queue clock");
                        }
                    }
                    Err(error) => warn!(%error, "failed to encode queue clock"),
                }
            }
            Err(e) => warn!(path = %path.display(), error = %e, "failed to write queue-depth.csv"),
        }
    }

    // 3. Flush per_node_metrics.csv.
    let metrics_path = dir.join("per_node_metrics.csv");
    match orchestrator.export_per_node_metrics(&dir) {
        Ok(()) => info!(path = %metrics_path.display(), "per_node_metrics.csv written"),
        Err(e) => warn!(error = %e, "failed to write per_node_metrics.csv"),
    }
}

/// Cancel the pipeline on the first SIGINT or SIGTERM and exit at once on the
/// second, for a shutdown that does not finish.
///
/// The listener runs on its own thread and runtime: a guest spinning on a
/// worker with no fuel or epoch limit can starve the main runtime's drivers,
/// and then no task there would ever see the signal.
fn spawn_shutdown_signal_thread(cancel: CancellationToken) -> std::io::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_io().build()?;
    let mut signals = {
        let _context = runtime.enter();
        ShutdownSignals::install()?
    };
    std::thread::Builder::new().name("wafer-signals".into()).spawn(move || {
        runtime.block_on(async move {
            signals.recv().await;
            info!("Shutdown signal received");
            cancel.cancel();
            let exit_code = signals.recv().await;
            error!("Second shutdown signal received; exiting without finishing the shutdown");
            #[expect(
                clippy::exit,
                reason = "the operator signalled again because the graceful shutdown did not finish"
            )]
            std::process::exit(exit_code);
        });
    })?;
    Ok(())
}

struct ShutdownSignals {
    #[cfg(unix)]
    terminate: signal::unix::Signal,
    #[cfg(unix)]
    interrupt: signal::unix::Signal,
}

impl ShutdownSignals {
    #[cfg(unix)]
    fn install() -> std::io::Result<Self> {
        use signal::unix::{SignalKind, signal};
        Ok(Self {
            terminate: signal(SignalKind::terminate())?,
            interrupt: signal(SignalKind::interrupt())?,
        })
    }

    #[cfg(not(unix))]
    const fn install() -> std::io::Result<Self> {
        Ok(Self {})
    }

    /// Wait for the next signal and return the shell's exit code for it.
    #[cfg(unix)]
    async fn recv(&mut self) -> i32 {
        tokio::select! {
            _ = self.terminate.recv() => 143,
            _ = self.interrupt.recv() => 130,
        }
    }

    #[cfg(not(unix))]
    async fn recv(&mut self) -> i32 {
        if signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
        130
    }
}
