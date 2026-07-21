//! Pipeline orchestrator — lifecycle management with watch-channel hot-swap.
//!
//! The orchestrator builds, spawns, monitors, and tears down the pipeline.
//! After spawn, nodes OWN their instances — no shared Mutex on the hot path.
//! Hot-swap signals go through `watch::Sender` per Wasm node.
//! Status queries use atomic reads from `Arc<NodeStateTracker>` + `Arc<NodeMetrics>`.
//!
//! See docs/rfcs/RFC-005-orchestrator.md D6, D11, D12.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::engine::WaferEngine;
use crate::error::{Result, WaferError};
use crate::node::{NodeMetrics, NodeStateTracker};
use wafer_types::NodeState;
use crate::orchestrator::builder::{BuildOutput, NodeBundleKind};
use crate::runner::SwapPayload;
use crate::runner::{DownstreamSender, send_downstream};
use crate::runner::error_policy::DlqEnvelope;
use crate::runner::source::run_source_loop;
use crate::runner::sink::run_sink_loop;
use crate::runner::transform::run_transform_loop;
use crate::runner::filter::run_filter_loop;
use crate::runner::router::run_router_loop;

/// Default timeout for graceful shutdown (waiting for tasks to exit).
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Pipeline orchestrator managing node lifecycle with watch-channel hot-swap.
///
/// After `spawn()`, the orchestrator retains only:
/// - `JoinSet` for task supervision (detect panics, await completion)
/// - Watch senders for hot-swap signaling (one per Wasm node)
/// - `CancellationToken` for triggering shutdown
/// - Config for diffing on resync
/// - State trackers + metrics for lock-free status queries
///
/// NO shared mutex on the hot path. Nodes own their instances.
pub struct PipelineHandle {
    watch_senders: HashMap<Box<str>, watch::Sender<Option<SwapPayload>>>,
    cancel_token: CancellationToken,
    config: Config,
    state_trackers: HashMap<Box<str>, Arc<NodeStateTracker>>,
    metrics: HashMap<Box<str>, Arc<NodeMetrics>>,
    engine: Arc<WaferEngine>,
    running: Arc<AtomicBool>,
    /// P0.10 (A3 residual): per-node compare-and-swap guards preventing
    /// two concurrent swap requests for the same node from both winning
    /// the watch-channel race. Populated at build time for every
    /// swappable node. Independent of `watch_senders` so that internal
    /// (non-API) swap paths can also participate.
    swap_in_progress: HashMap<Box<str>, Arc<AtomicBool>>,
    /// P0.10 (A3 residual): shared hot-swap-metrics store, populated on
    /// every successful swap via [`record_hotswap_phase`](Self::record_hotswap_phase)
    /// and rendered by the /metrics handler.
    hotswap_metrics: Arc<crate::metrics::types::HotSwapMetrics>,
    /// P0.12 (A5 residual): per-node cached SHA-256 (hex) of the currently
    /// loaded plugin bytes. Populated by the hot-swap handler on
    /// successful convergence. Consumed by
    /// [`verify_plugin_hash`](Self::verify_plugin_hash) so `/reconfigure`
    /// can reject callers whose mental model has diverged from the
    /// actually-running binary.
    plugin_hashes: Arc<std::sync::RwLock<HashMap<Box<str>, String>>>,
}

/// RAII guard returned by [`PipelineHandle::try_begin_swap`]. Dropping
/// the guard releases the corresponding node's swap-in-progress flag so
/// the next API call can succeed.
#[must_use = "drop the guard once the swap has converged or failed"]
pub struct SwapGuard {
    flag: Arc<AtomicBool>,
}

impl std::fmt::Debug for SwapGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SwapGuard")
            .field("held", &self.flag.load(Ordering::Acquire))
            .finish()
    }
}

impl Drop for SwapGuard {
    fn drop(&mut self) {
        // Release is enough: any subsequent `try_begin_swap` uses an
        // Acquire compare_exchange which synchronises with this store.
        self.flag.store(false, Ordering::Release);
    }
}

impl PipelineHandle {
    /// Attempt to acquire the swap slot for `node_id`. Returns `Ok(guard)`
    /// on success. Returns `Err(WaferError::Runtime("swap-in-progress"))`
    /// when another swap request is already in flight for the same node.
    ///
    /// The guard MUST be held for the lifetime of the swap (from
    /// preparation through to completion or timeout). Dropping the guard
    /// releases the slot.
    ///
    /// # Errors
    ///
    /// - `WaferError::Runtime("node-not-swappable")` when the node id does
    ///   not correspond to a Wasm node (Source/Sink cannot swap).
    /// - `WaferError::Runtime("swap-in-progress")` when a concurrent swap
    ///   is already in flight for this node.
    pub fn try_begin_swap(&self, node_id: &str) -> Result<SwapGuard> {
        let flag = self.swap_in_progress.get(node_id).ok_or_else(|| {
            WaferError::Runtime(format!("node-not-swappable: {node_id}"))
        })?;
        flag.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map_err(|_| WaferError::Runtime(format!("swap-in-progress: {node_id}")))?;
        Ok(SwapGuard { flag: Arc::clone(flag) })
    }

    /// P0.10 (A3 residual): record a single hot-swap phase timing on the
    /// shared `hot_swap_phase_ns` histogram. Phase label is one of
    /// {compile, instantiate, signal, ack, first_v2, convergence}.
    pub fn record_hotswap_phase(&self, phase: &str, node_id: &str, ns: u64) {
        let key = (phase.to_owned(), node_id.to_owned());
        if let Ok(guard) = self.hotswap_metrics.phase_histogram.read()
            && let Some(h) = guard.get(&key)
        {
            h.record(ns);
            return;
        }
        if let Ok(mut guard) = self.hotswap_metrics.phase_histogram.write() {
            let h = guard
                .entry(key)
                .or_insert_with(crate::metrics::types::PhaseHistogram::new);
            h.record(ns);
        }
    }

    /// P0.11 (A7 residual): record a single Recovering → Running duration
    /// (nanoseconds) on the shared per-node recovery histogram.
    pub fn record_recovery_duration(&self, node_id: &str, ns: u64) {
        let key = node_id.to_owned();
        if let Ok(guard) = self.hotswap_metrics.recovery_duration.read()
            && let Some(h) = guard.get(&key)
        {
            h.record(ns);
            return;
        }
        if let Ok(mut guard) = self.hotswap_metrics.recovery_duration.write() {
            let h = guard
                .entry(key)
                .or_insert_with(crate::metrics::types::PhaseHistogram::new);
            h.record(ns);
        }
    }

    /// P0.12 (A5 residual): register the SHA-256 (hex) of the plugin bytes
    /// currently loaded on `node_id`. Called on every successful hot-swap.
    pub fn record_plugin_hash(&self, node_id: &str, hex_hash: impl Into<String>) {
        if let Ok(mut guard) = self.plugin_hashes.write() {
            guard.insert(node_id.into(), hex_hash.into());
        }
    }

    /// P0.12 (A5 residual): verify that `expected_hex` matches the cached
    /// hash for `node_id`. Returns:
    /// - `Ok(())` when the node has no cached hash yet (backward compat
    ///   with the initial-launcher path, which does not yet register).
    /// - `Ok(())` when the expected hash exactly matches the cached one.
    /// - `Err(WaferError::Runtime("plugin-hash-mismatch…"))` otherwise.
    ///
    /// The API handler maps the `"plugin-hash-mismatch"` prefix to HTTP
    /// 409 CONFLICT.
    ///
    /// # Errors
    ///
    /// See variants above.
    pub fn verify_plugin_hash(&self, node_id: &str, expected_hex: &str) -> Result<()> {
        let guard = self.plugin_hashes.read().map_err(|e| {
            WaferError::Runtime(format!("plugin_hashes lock poisoned: {e}"))
        })?;
        match guard.get(node_id) {
            Some(cached) if cached.eq_ignore_ascii_case(expected_hex) => Ok(()),
            Some(cached) => Err(WaferError::Runtime(format!(
                "plugin-hash-mismatch: node '{node_id}' has hash {cached} but caller supplied {expected_hex}"
            ))),
            None => Ok(()),
        }
    }

    /// Read-only handle to the hot-swap metrics store for use by the
    /// /metrics HTTP handler.
    #[must_use]
    pub fn hotswap_metrics(&self) -> &Arc<crate::metrics::types::HotSwapMetrics> {
        &self.hotswap_metrics
    }

    /// Test-only constructor for P0.10 unit tests. Populates just the
    /// fields needed to exercise `try_begin_swap` and
    /// `record_hotswap_phase`; everything else is defaulted.
    #[cfg(test)]
    pub(crate) fn for_p0_10_test(swappable_ids: &[&str]) -> Self {
        let swap_in_progress: HashMap<Box<str>, Arc<AtomicBool>> = swappable_ids
            .iter()
            .map(|id| ((*id).into(), Arc::new(AtomicBool::new(false))))
            .collect();
        Self {
            watch_senders: HashMap::new(),
            cancel_token: CancellationToken::new(),
            config: Config::default(),
            state_trackers: HashMap::new(),
            metrics: HashMap::new(),
            engine: Arc::new(WaferEngine::new().expect("test engine")),
            running: Arc::new(AtomicBool::new(true)),
            swap_in_progress,
            hotswap_metrics: Arc::new(crate::metrics::types::HotSwapMetrics::default()),
            plugin_hashes: Arc::new(std::sync::RwLock::new(HashMap::new())),
        }
    }

    /// Send a hot-swap payload to a specific node via its watch channel.
    ///
    /// # Errors
    ///
    /// Returns error if the node doesn't exist or doesn't support hot-swap.
    pub fn send_swap(&self, node_id: &str, payload: SwapPayload) -> Result<()> {
        let sender = self.watch_senders.get(node_id).ok_or_else(|| {
            WaferError::Runtime(format!(
                "cannot hot-swap node '{node_id}': not found or not a Wasm node"
            ))
        })?;

        sender.send(Some(payload)).map_err(|_| {
            WaferError::Runtime(format!(
                "cannot hot-swap node '{node_id}': receiver dropped (task dead?)"
            ))
        })?;

        Ok(())
    }

    /// Request shutdown without waiting (non-blocking).
    pub fn cancel(&self) {
        self.cancel_token.cancel();
    }

    /// Check if the pipeline is still running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    /// Get the current state of a specific node.
    #[must_use]
    pub fn node_state(&self, node_id: &str) -> Option<NodeState> {
        self.state_trackers.get(node_id).map(|t| t.state())
    }

    /// Get a snapshot of metrics for a specific node.
    #[must_use]
    pub fn node_metrics(&self, node_id: &str) -> Option<&Arc<NodeMetrics>> {
        self.metrics.get(node_id)
    }

    /// Get all node IDs that support hot-swap.
    #[must_use]
    pub fn swappable_nodes(&self) -> Vec<&str> {
        self.watch_senders.keys().map(|k| &**k).collect()
    }

    /// Access the current configuration.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Access the Wasm engine (for hot-swap compilation).
    #[must_use]
    pub fn engine(&self) -> &Arc<WaferEngine> {
        &self.engine
    }
}

pub struct PipelineOrchestrator {
    /// Supervised task set — first-failure detection via JoinSet.
    tasks: JoinSet<()>,
    /// Hot-swap signal channels (ownership transfer via watch).
    watch_senders: HashMap<Box<str>, watch::Sender<Option<SwapPayload>>>,
    /// Shared cancellation token — fires to initiate graceful shutdown.
    cancel_token: CancellationToken,
    /// Current pipeline configuration (for diff on resync).
    config: Config,
    /// Per-node state trackers — atomic reads, no lock needed.
    state_trackers: HashMap<Box<str>, Arc<NodeStateTracker>>,
    /// Per-node metrics — atomic reads for Prometheus exposition.
    metrics: HashMap<Box<str>, Arc<NodeMetrics>>,
    /// Wasm engine for hot-swap compilation.
    engine: Arc<WaferEngine>,
    /// DLQ task handle (spawned separately from node tasks).
    dlq_handle: Option<tokio::task::JoinHandle<()>>,
    /// Shared running flag used by API handles.
    running: Arc<AtomicBool>,
    /// Per-node in-progress-swap flags (see [`PipelineHandle::try_begin_swap`]).
    swap_in_progress: HashMap<Box<str>, Arc<AtomicBool>>,
    /// Shared hot-swap metrics store, held for the /metrics handler.
    hotswap_metrics: Arc<crate::metrics::types::HotSwapMetrics>,
    /// Shared plugin-hash registry, populated on every successful hot-swap.
    plugin_hashes: Arc<std::sync::RwLock<HashMap<Box<str>, String>>>,
}

impl PipelineOrchestrator {
    /// Build and spawn a pipeline from configuration.
    ///
    /// This is the primary constructor — builds the pipeline infrastructure
    /// (channels, bundles, control) and spawns all node tasks.
    ///
    /// # Errors
    ///
    /// Returns error if DAG validation fails or builder encounters issues.
    pub fn from_build_output(
        build_output: BuildOutput,
        config: Config,
        engine: Arc<WaferEngine>,
    ) -> Self {
        let swap_in_progress = build_output
            .watch_senders
            .keys()
            .map(|k| (k.clone(), Arc::new(AtomicBool::new(false))))
            .collect::<HashMap<_, _>>();

        let mut orchestrator = Self {
            tasks: JoinSet::new(),
            watch_senders: build_output.watch_senders,
            cancel_token: build_output.cancel_token.clone(),
            config,
            state_trackers: build_output.state_trackers,
            metrics: build_output.metrics_map,
            engine,
            dlq_handle: None,
            running: Arc::new(AtomicBool::new(true)),
            swap_in_progress,
            hotswap_metrics: Arc::new(crate::metrics::types::HotSwapMetrics::default()),
            plugin_hashes: Arc::new(std::sync::RwLock::new(HashMap::new())),
        };

        // Spawn DLQ sink task if configured
        if let Some(dlq_rx) = build_output.dlq_receiver {
            let cancel = build_output.cancel_token.clone();
            let handle = tokio::spawn(run_dlq_sink(dlq_rx, cancel));
            orchestrator.dlq_handle = Some(handle);
        }

        // Spawn node tasks — each bundle moves into its runner loop
        orchestrator.spawn_bundles(build_output.node_bundles);

        orchestrator
    }

    /// Create a cloneable control-plane handle for HTTP API tasks.
    #[must_use]
    pub fn handle(&self) -> PipelineHandle {
        PipelineHandle {
            watch_senders: self.watch_senders.clone(),
            cancel_token: self.cancel_token.clone(),
            config: self.config.clone(),
            state_trackers: self.state_trackers.clone(),
            metrics: self.metrics.clone(),
            engine: Arc::clone(&self.engine),
            running: Arc::clone(&self.running),
            swap_in_progress: self.swap_in_progress.clone(),
            hotswap_metrics: Arc::clone(&self.hotswap_metrics),
            plugin_hashes: Arc::clone(&self.plugin_hashes),
        }
    }

    /// Spawn all node bundles into independent tokio tasks via JoinSet.
    ///
    /// Source/Sink: init() is called here before spawning the adapter loop.
    /// Wasm nodes: spawned with real runner loops when node instance is present.
    fn spawn_bundles(
        &mut self,
        bundles: Vec<crate::orchestrator::builder::NodeBundle>,
    ) {
        for bundle in bundles {
            let node_id = bundle.node_id.clone();
            let cancel = bundle.cancel;
            let state = bundle.state;
            let metrics = bundle.metrics;

            match bundle.kind {
                NodeBundleKind::Transform { receiver, senders, swap_rx, policy, node } => {
                    if let Some(transform) = node {
                        self.tasks.spawn(async move {
                            run_transform_loop(
                                transform, receiver, senders, swap_rx,
                                policy, cancel, state, metrics,
                            ).await;
                        });
                    } else {
                        // No compiled Wasm node — run as identity passthrough.
                        // Native transforms forward messages unchanged.
                        self.tasks.spawn(async move {
                            run_passthrough_loop(receiver, senders, cancel, state, metrics).await;
                        });
                    }
                }
                NodeBundleKind::Filter { receiver, senders, swap_rx, policy, node } => {
                    if let Some(filter) = node {
                        self.tasks.spawn(async move {
                            run_filter_loop(
                                filter, receiver, senders, swap_rx,
                                policy, cancel, state, metrics,
                            ).await;
                        });
                    } else {
                        // Native filter — forward all messages (no-op filter passes everything)
                        self.tasks.spawn(async move {
                            run_passthrough_loop(receiver, senders, cancel, state, metrics).await;
                        });
                    }
                }
                NodeBundleKind::Router { receiver, senders, swap_rx, policy, node } => {
                    if let Some(router) = node {
                        self.tasks.spawn(async move {
                            run_router_loop(
                                router, receiver, senders, swap_rx,
                                policy, cancel, state, metrics,
                            ).await;
                        });
                    } else {
                        // Native router — broadcast to all downstreams
                        self.tasks.spawn(async move {
                            run_passthrough_loop(receiver, senders, cancel, state, metrics).await;
                        });
                    }
                }
                NodeBundleKind::Source { source, senders } => {
                    if let Some(mut source) = source {
                        // Init source before spawning its loop
                        self.tasks.spawn(async move {
                            if let Err(e) = source.init().await {
                                tracing::error!(node = %node_id, error = %e, "source init failed");
                                return;
                            }
                            run_source_loop(source, senders, cancel, state, metrics).await;
                        });
                    } else {
                        // No source instance (unit test without I/O construction)
                        tracing::debug!(node = %node_id, "Source task: no instance, awaiting cancel");
                        self.tasks.spawn(async move {
                            cancel.cancelled().await;
                        });
                    }
                }
                NodeBundleKind::Sink { sink, receiver } => {
                    if let Some(mut sink) = sink {
                        // Init sink before spawning its loop
                        self.tasks.spawn(async move {
                            if let Err(e) = sink.init().await {
                                tracing::error!(node = %node_id, error = %e, "sink init failed");
                                return;
                            }
                            run_sink_loop(sink, receiver, cancel, state, metrics).await;
                        });
                    } else {
                        // No sink instance (unit test without I/O construction)
                        tracing::debug!(node = %node_id, "Sink task: no instance, awaiting cancel");
                        self.tasks.spawn(async move {
                            cancel.cancelled().await;
                        });
                    }
                }
            }
        }
    }

    /// Send a hot-swap payload to a specific node via its watch channel.
    ///
    /// The node loop will pick up the new instance between messages.
    /// This is non-blocking — the caller doesn't wait for the swap to complete.
    ///
    /// # Errors
    ///
    /// Returns error if the node doesn't exist or doesn't support hot-swap.
    pub fn send_swap(&self, node_id: &str, payload: SwapPayload) -> Result<()> {
        let sender = self.watch_senders.get(node_id).ok_or_else(|| {
            WaferError::Runtime(format!(
                "cannot hot-swap node '{node_id}': not found or not a Wasm node"
            ))
        })?;

        sender.send(Some(payload)).map_err(|_| {
            WaferError::Runtime(format!(
                "cannot hot-swap node '{node_id}': receiver dropped (task dead?)"
            ))
        })?;

        Ok(())
    }

    /// Initiate graceful shutdown.
    ///
    /// Fires the cancellation token → all runner loops break → flush retries →
    /// tasks complete → join all.
    ///
    /// Uses a timeout to prevent hanging if a task is stuck.
    pub async fn shutdown(&mut self) -> Result<()> {
        tracing::info!("Pipeline shutdown initiated");
        self.cancel_token.cancel();

        // Wait for all tasks with timeout
        let deadline = tokio::time::sleep(SHUTDOWN_TIMEOUT);
        tokio::pin!(deadline);

        loop {
            tokio::select! {
                biased;
                () = &mut deadline => {
                    tracing::warn!(
                        remaining = self.tasks.len(),
                        "Shutdown timeout — aborting remaining tasks"
                    );
                    self.tasks.shutdown().await;
                    break;
                }
                result = self.tasks.join_next() => {
                    match result {
                        Some(Ok(())) => {} // Task exited cleanly
                        Some(Err(e)) => {
                            tracing::error!(error = %e, "Task panicked during shutdown");
                        }
                        None => break, // All tasks done
                    }
                }
            }
        }

        // Wait for DLQ task
        if let Some(handle) = self.dlq_handle.take() {
            match tokio::time::timeout(Duration::from_secs(5), handle).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::error!(error = %e, "DLQ task panicked"),
                Err(_) => tracing::warn!("DLQ task did not exit within timeout"),
            }
        }

        self.running.store(false, Ordering::Release);
        tracing::info!("Pipeline shutdown complete");
        Ok(())
    }

    /// Run the pipeline until all tasks complete naturally or cancellation fires.
    ///
    /// For finite pipelines (e.g., BenchSource with `total_messages`), the source
    /// task exits after sending all messages → its downstream channel closes →
    /// transform/filter tasks see `recv() = None` and exit → sink channels close →
    /// sink tasks drain and exit → JoinSet empties → this method returns.
    ///
    /// Returns `Ok(())` if all tasks exited cleanly, `Err` if any panicked.
    pub async fn run_until_complete(&mut self) -> Result<()> {
        let mut had_panic = false;

        loop {
            tokio::select! {
                biased;
                () = self.cancel_token.cancelled() => {
                    // External cancellation — drain remaining tasks with timeout
                    let deadline = tokio::time::sleep(SHUTDOWN_TIMEOUT);
                    tokio::pin!(deadline);
                    loop {
                        tokio::select! {
                            biased;
                            () = &mut deadline => {
                                self.tasks.shutdown().await;
                                break;
                            }
                            result = self.tasks.join_next() => match result {
                                Some(Ok(())) => {}
                                Some(Err(e)) => {
                                    tracing::error!(error = %e, "Task panicked during shutdown");
                                    had_panic = true;
                                }
                                None => break,
                            },
                        }
                    }
                    break;
                }
                result = self.tasks.join_next() => {
                    match result {
                        Some(Ok(())) => {}
                        Some(Err(e)) => {
                            tracing::error!(error = %e, "Task panicked during pipeline run");
                            had_panic = true;
                        }
                        None => break, // All tasks completed
                    }
                }
            }
        }

        // Wait for DLQ task
        if let Some(handle) = self.dlq_handle.take() {
            match tokio::time::timeout(Duration::from_secs(5), handle).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::error!(error = %e, "DLQ task panicked");
                    had_panic = true;
                }
                Err(_) => tracing::warn!("DLQ task did not exit within timeout"),
            }
        }

        self.running.store(false, Ordering::Release);

        if had_panic {
            Err(WaferError::Runtime("one or more tasks panicked during pipeline run".into()))
        } else {
            Ok(())
        }
    }

    /// Request shutdown without waiting (non-blocking).
    pub fn cancel(&self) {
        self.cancel_token.cancel();
    }

    /// Check if the pipeline is still running (has active tasks).
    #[must_use]
    pub fn is_running(&self) -> bool {
        !self.tasks.is_empty()
    }

    /// Get the current state of a specific node.
    #[must_use]
    pub fn node_state(&self, node_id: &str) -> Option<NodeState> {
        self.state_trackers.get(node_id).map(|t| t.state())
    }

    /// Get a snapshot of metrics for a specific node.
    #[must_use]
    pub fn node_metrics(&self, node_id: &str) -> Option<&Arc<NodeMetrics>> {
        self.metrics.get(node_id)
    }

    /// Get the number of active tasks.
    #[must_use]
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    /// Get the number of Wasm nodes (those with watch channels for hot-swap).
    #[must_use]
    pub fn wasm_node_count(&self) -> usize {
        self.watch_senders.len()
    }

    /// Get all node IDs that support hot-swap.
    #[must_use]
    pub fn swappable_nodes(&self) -> Vec<&str> {
        self.watch_senders.keys().map(|k| &**k).collect()
    }

    /// Access the current configuration.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Access the cancellation token (for external shutdown triggers).
    #[must_use]
    pub fn cancel_token(&self) -> &CancellationToken {
        &self.cancel_token
    }

    /// Access the Wasm engine (for hot-swap compilation).
    #[must_use]
    pub fn engine(&self) -> &Arc<WaferEngine> {
        &self.engine
    }
}

/// Identity passthrough loop for native nodes without Wasm instances.
///
/// Receives messages, records metrics, and forwards unchanged to all downstreams.
/// Used for native-transform (passthrough/uppercase) and nodes without .wasm.
async fn run_passthrough_loop(
    mut receiver: tokio::sync::mpsc::Receiver<crate::queue::RuntimeEnvelope>,
    senders: Vec<DownstreamSender>,
    cancel: CancellationToken,
    state: Arc<NodeStateTracker>,
    metrics: Arc<NodeMetrics>,
) {
    use crate::node::ProcessingGuard;
    use std::time::Instant;

    state.transition_to_running();

    loop {
        let envelope = tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            msg = receiver.recv() => match msg {
                Some(e) => e,
                None => break,
            },
        };

        let start = Instant::now();
        let _guard = ProcessingGuard::enter(&state);
        // Identity: forward unchanged
        send_downstream(&senders, envelope).await;
        let duration_ns = start.elapsed().as_nanos() as u64;
        drop(_guard);
        metrics.record_processed(duration_ns);
    }
}

/// Simple DLQ sink that drains envelopes until cancelled.
///
/// In a full deployment, this would write to a file, MQTT topic, or HTTP endpoint.
/// For now, it logs and drops. The DLQ channel is bounded — if this task falls
/// behind, senders will see backpressure.
async fn run_dlq_sink(
    mut receiver: tokio::sync::mpsc::Receiver<DlqEnvelope>,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                // Drain remaining messages before exiting
                while let Ok(envelope) = receiver.try_recv() {
                    tracing::warn!(
                        node = %envelope.source_node,
                        reason = ?envelope.reason,
                        "DLQ: message during shutdown drain"
                    );
                }
                break;
            }
            msg = receiver.recv() => {
                match msg {
                    Some(envelope) => {
                        tracing::warn!(
                            node = %envelope.source_node,
                            category = ?envelope.error_category,
                            reason = ?envelope.reason,
                            retry_count = envelope.retry_count,
                            "DLQ: dead letter received"
                        );
                    }
                    None => break, // All senders dropped
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::config::{
        Config, EdgeDef, EngineConfig, NodeDef, SourceDef, SinkDef, StdinSourceConfig,
        StdoutSinkConfig, WasmNodeDef,
    };
    use crate::orchestrator::builder::{build_pipeline, build_pipeline_with_io};
    use crate::queue::RuntimeEnvelope;
    use crate::testing::channel::{ChannelSink, ChannelSource};

    fn edge(from: &str, to: &str) -> EdgeDef {
        EdgeDef {
            from: from.to_string(),
            to: to.to_string(),
            port: None,
            capacity: None,
            overflow: None,
        }
    }

    fn test_config() -> Config {
        Config {
            engine: EngineConfig::default(),
            nodes: HashMap::from([
                (
                    "src".to_string(),
                    NodeDef::Source(SourceDef::Stdin(StdinSourceConfig::default())),
                ),
                (
                    "t1".to_string(),
                    NodeDef::Transform(WasmNodeDef {
                        plugin: wafer_types::config::PluginSpec::WasmPath("test.wasm".to_string()),
                        ..Default::default()
                    }),
                ),
                (
                    "sink".to_string(),
                    NodeDef::Sink(SinkDef::Stdout(StdoutSinkConfig::default())),
                ),
            ]),
            edges: vec![edge("src", "t1"), edge("t1", "sink")],
            ..Default::default()
        }
    }

    fn source_sink_config() -> Config {
        Config {
            engine: EngineConfig::default(),
            nodes: HashMap::from([
                (
                    "src".to_string(),
                    NodeDef::Source(SourceDef::Stdin(StdinSourceConfig::default())),
                ),
                (
                    "sink".to_string(),
                    NodeDef::Sink(SinkDef::Stdout(StdoutSinkConfig::default())),
                ),
            ]),
            edges: vec![edge("src", "sink")],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_build_creates_correct_watch_senders() {
        let config = test_config();
        let engine = Arc::new(WaferEngine::new().expect("engine"));
        let build_output = build_pipeline(&config).expect("build");

        let orch = PipelineOrchestrator::from_build_output(build_output, config, engine);

        assert_eq!(orch.wasm_node_count(), 1);
        assert!(orch.swappable_nodes().contains(&"t1"));
    }

    #[tokio::test]
    async fn test_spawn_creates_tasks() {
        let config = test_config();
        let engine = Arc::new(WaferEngine::new().expect("engine"));
        let build_output = build_pipeline(&config).expect("build");

        let orch = PipelineOrchestrator::from_build_output(build_output, config, engine);

        assert_eq!(orch.task_count(), 3);
        assert!(orch.is_running());
    }

    #[tokio::test]
    async fn test_shutdown_completes_all_tasks() {
        let config = test_config();
        let engine = Arc::new(WaferEngine::new().expect("engine"));
        let build_output = build_pipeline(&config).expect("build");

        let mut orch = PipelineOrchestrator::from_build_output(build_output, config, engine);

        assert!(orch.is_running());

        orch.shutdown().await.expect("shutdown");

        assert!(!orch.is_running());
        assert_eq!(orch.task_count(), 0);
    }

    #[tokio::test]
    async fn test_cancel_triggers_shutdown() {
        let config = test_config();
        let engine = Arc::new(WaferEngine::new().expect("engine"));
        let build_output = build_pipeline(&config).expect("build");

        let mut orch = PipelineOrchestrator::from_build_output(build_output, config, engine);

        orch.cancel();
        tokio::time::sleep(Duration::from_millis(50)).await;
        orch.shutdown().await.expect("shutdown");
        assert!(!orch.is_running());
    }

    #[tokio::test]
    async fn test_node_state_returns_tracker_value() {
        let config = test_config();
        let engine = Arc::new(WaferEngine::new().expect("engine"));
        let build_output = build_pipeline(&config).expect("build");

        let orch = PipelineOrchestrator::from_build_output(build_output, config, engine);

        assert!(orch.node_state("t1").is_some());
        assert!(orch.node_state("nonexistent").is_none());
    }

    #[tokio::test]
    async fn test_node_metrics_returns_metrics() {
        let config = test_config();
        let engine = Arc::new(WaferEngine::new().expect("engine"));
        let build_output = build_pipeline(&config).expect("build");

        let orch = PipelineOrchestrator::from_build_output(build_output, config, engine);

        let metrics = orch.node_metrics("t1").expect("metrics");
        assert_eq!(metrics.processed(), 0);
    }

    #[tokio::test]
    async fn test_send_swap_nonexistent_node_fails() {
        let config = test_config();
        let engine = Arc::new(WaferEngine::new().expect("engine"));
        let build_output = build_pipeline(&config).expect("build");

        let mut orch = PipelineOrchestrator::from_build_output(build_output, config, engine);

        assert!(orch.watch_senders.contains_key("t1"));
        assert!(!orch.watch_senders.contains_key("nonexistent"));
        assert!(!orch.watch_senders.contains_key("src"));
        assert!(!orch.watch_senders.contains_key("sink"));

        orch.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn test_source_sink_real_loops_process_messages() {
        let config = source_sink_config();
        let engine = Arc::new(WaferEngine::new().expect("engine"));

        let (source_tx, source) = ChannelSource::new("src");
        let (sink, mut sink_rx) = ChannelSink::new("sink");

        let mut sources = HashMap::new();
        sources.insert("src".to_string(), Box::new(source) as Box<dyn crate::node::Source + Send>);
        let mut sinks = HashMap::new();
        sinks.insert("sink".to_string(), Box::new(sink) as Box<dyn crate::node::Sink + Send>);

        let build_output = build_pipeline_with_io(&config, sources, sinks).expect("build");
        let mut orch = PipelineOrchestrator::from_build_output(build_output, config, engine);

        assert_eq!(orch.task_count(), 2);

        for i in 0..5 {
            source_tx
                .send(RuntimeEnvelope::from_string("test", format!("msg-{i}")))
                .await
                .unwrap();
        }
        drop(source_tx);

        let mut received = Vec::new();
        let deadline = tokio::time::sleep(Duration::from_secs(2));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                biased;
                () = &mut deadline => panic!("timeout waiting for sink messages"),
                msg = sink_rx.recv() => match msg {
                    Some(env) => received.push(env),
                    None => break,
                }
            }
        }

        assert_eq!(received.len(), 5);
        for (i, env) in received.iter().enumerate() {
            assert_eq!(env.payload_as_string(), format!("msg-{i}"));
        }

        orch.shutdown().await.expect("shutdown");
        assert!(!orch.is_running());
    }

    #[tokio::test]
    async fn test_run_until_complete_finite_pipeline() {
        let config = source_sink_config();
        let engine = Arc::new(WaferEngine::new().expect("engine"));

        let (source_tx, source) = ChannelSource::new("src");
        let (sink, mut sink_rx) = ChannelSink::new("sink");

        let mut sources = HashMap::new();
        sources.insert("src".to_string(), Box::new(source) as Box<dyn crate::node::Source + Send>);
        let mut sinks = HashMap::new();
        sinks.insert("sink".to_string(), Box::new(sink) as Box<dyn crate::node::Sink + Send>);

        let build_output = build_pipeline_with_io(&config, sources, sinks).expect("build");
        let mut orch = PipelineOrchestrator::from_build_output(build_output, config, engine);

        tokio::spawn(async move {
            for i in 0..10 {
                source_tx
                    .send(RuntimeEnvelope::from_string("test", format!("msg-{i}")))
                    .await
                    .unwrap();
            }
            drop(source_tx);
        });

        tokio::spawn(async move {
            while sink_rx.recv().await.is_some() {}
        });

        let result = tokio::time::timeout(Duration::from_secs(5), orch.run_until_complete())
            .await
            .expect("timeout: run_until_complete didn't finish");

        assert!(result.is_ok(), "run_until_complete failed: {:?}", result.err());
        assert!(!orch.is_running());
    }

    // ========================================================================
    // P0.10 (A3 residual) unit tests
    // ========================================================================

    /// AC2: overlapping same-node swap requests return "swap-in-progress";
    /// serialised requests succeed one after another.
    #[test]
    fn swap_guard_prevents_overlapping_swap() {
        let handle = PipelineHandle::for_p0_10_test(&["transform"]);

        let first = handle.try_begin_swap("transform").expect("first must acquire");
        let err = handle
            .try_begin_swap("transform")
            .expect_err("concurrent second must be rejected");
        assert!(
            err.to_string().contains("swap-in-progress"),
            "expected swap-in-progress error, got: {err}"
        );

        // Drop first guard — slot is released.
        drop(first);
        let _third = handle
            .try_begin_swap("transform")
            .expect("after guard dropped, next request must succeed");
    }

    /// Non-swappable / unknown node yields node-not-swappable, distinct
    /// from the in-progress error so the API handler can map it to 404
    /// instead of 409.
    #[test]
    fn swap_guard_reports_unknown_node() {
        let handle = PipelineHandle::for_p0_10_test(&["transform"]);
        let err = handle
            .try_begin_swap("does-not-exist")
            .expect_err("unknown node id must fail");
        assert!(
            err.to_string().contains("node-not-swappable"),
            "expected node-not-swappable error, got: {err}"
        );
    }

    /// AC1: recording every phase populates six independent series in
    /// the phase histogram. The Prometheus emitter reads from this map.
    #[test]
    fn phase_histogram_records_six_phases() {
        let handle = PipelineHandle::for_p0_10_test(&["transform"]);

        for (phase, ns) in [
            ("compile",       50_000_000_u64),
            ("instantiate",    5_000_000_u64),
            ("signal",             1_000_u64),
            ("ack",               50_000_u64),
            ("first_v2",         200_000_u64),
            ("convergence",   10_000_000_u64),
        ] {
            handle.record_hotswap_phase(phase, "transform", ns);
        }

        let hs = handle.hotswap_metrics();
        let guard = hs.phase_histogram.read().unwrap();
        assert_eq!(guard.len(), 6, "expected exactly six (phase, node) series");
        for phase in ["compile", "instantiate", "signal", "ack", "first_v2", "convergence"] {
            let key = (phase.to_owned(), "transform".to_owned());
            let h = guard.get(&key).unwrap_or_else(|| panic!("missing phase: {phase}"));
            assert_eq!(
                h.count.load(std::sync::atomic::Ordering::Relaxed),
                1,
                "phase {phase} must have exactly one sample",
            );
            assert!(
                h.sum_ns.load(std::sync::atomic::Ordering::Relaxed) > 0,
                "phase {phase} sum_ns must be non-zero",
            );
        }
    }

    // ========================================================================
    // P0.12 (A5 residual) tests
    // ========================================================================

    /// AC1: `verify_plugin_hash` short-circuits when the node has no
    /// cached hash (initial-launcher path). After a hot-swap-shaped hash
    /// registration, a mismatched supplied hash yields the exact
    /// `plugin-hash-mismatch` error prefix so the handler can map it to
    /// 409 CONFLICT. A matching hash yields `Ok(())`.
    #[test]
    fn plugin_hash_guard_rejects_mismatch() {
        let handle = PipelineHandle::for_p0_10_test(&["transform"]);

        // No hash cached yet — pass-through so the initial-launcher path
        // stays backward compatible.
        assert!(
            handle.verify_plugin_hash("transform", "abcdef").is_ok(),
            "empty registry must fall through"
        );

        // Register a canonical hash (from a hot-swap).
        handle.record_plugin_hash(
            "transform",
            "a".repeat(64), // 32-byte SHA-256 in hex; content-neutral for the test.
        );

        // Mismatch → error whose message starts with the sentinel string
        // the API handler maps to 409 CONFLICT.
        let err = handle
            .verify_plugin_hash("transform", "deadbeef")
            .expect_err("mismatch must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("plugin-hash-mismatch"),
            "error must start with plugin-hash-mismatch, got: {msg}"
        );

        // Case-insensitive match — hex encoders differ on case.
        assert!(
            handle
                .verify_plugin_hash("transform", &"A".repeat(64))
                .is_ok(),
            "hash match must be case-insensitive (hex encoders vary)"
        );

        // Exact-case match still works.
        assert!(
            handle
                .verify_plugin_hash("transform", &"a".repeat(64))
                .is_ok(),
            "exact-case hash match must pass"
        );
    }
}
