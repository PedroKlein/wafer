//! Pipeline orchestrator — lifecycle management with watch-channel hot-swap.
//!
//! The orchestrator builds, spawns, monitors, and tears down the pipeline.
//! After spawn, nodes OWN their instances — no shared Mutex on the hot path.
//! Hot-swap signals go through `watch::Sender` per Wasm node.
//! Status queries use atomic reads from `Arc<NodeStateTracker>` + `Arc<NodeMetrics>`.
//!
//! See docs/rfcs/RFC-005-orchestrator.md D6, D11, D12.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::io::{BufWriter, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::{Id as TaskId, JoinError, JoinSet};
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::engine::WaferEngine;
use crate::error::{Result, WaferError};
use crate::node::{NodeMetrics, NodeStateTracker};
use crate::orchestrator::builder::{BuildOutput, NodeBundleKind, QueueProbe};
use crate::runner::error_policy::DlqEnvelope;
use crate::runner::filter::run_filter_loop;
use crate::runner::router::run_router_loop;
use crate::runner::sink::run_sink_loop;
use crate::runner::source::run_source_loop;
use crate::runner::transform::run_transform_loop_with_config;
use crate::runner::{DownstreamSender, TrackedReceiver, send_downstream};
use crate::runner::{HotSwapProgress, SwapPayload};
use wafer_types::NodeState;

/// Default timeout for graceful shutdown (waiting for tasks to exit).
///
/// Kept well below the stop deadlines of the evaluation scripts
/// (`run-experiment.sh` waits 12 s after SIGTERM, the canonical runner 10 s),
/// so the bench artifacts written after the drain are not lost to a SIGKILL.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Exit value of one supervised node task: `Err` means the node failed before
/// or while running (for example a source/sink `init()` error), which fails
/// the whole run.
type NodeTaskResult = Result<()>;

fn spawn_wasm_runner(
    tasks: &mut JoinSet<NodeTaskResult>,
    runner: impl Future<Output = ()> + Send + 'static,
) -> TaskId {
    tasks
        .spawn(async move {
            runner.await;
            Ok(())
        })
        .id()
}

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
#[derive(Clone)]
pub struct PipelineHandle {
    watch_senders: HashMap<Box<str>, watch::Sender<Option<SwapPayload>>>,
    cancel_token: CancellationToken,
    config: Config,
    state_trackers: HashMap<Box<str>, Arc<NodeStateTracker>>,
    metrics: HashMap<Box<str>, Arc<NodeMetrics>>,
    engine: Arc<WaferEngine>,
    running: Arc<AtomicBool>,
    /// Loaded Wasm processing roles eligible for replacement.
    replacement_eligible: HashSet<Box<str>>,
    /// Per-node compare-and-swap guards shared by hot-swap and reconfigure.
    swap_in_progress: HashMap<Box<str>, Arc<AtomicBool>>,
    /// P0.10 (A3 residual): shared hot-swap-metrics store, populated on
    /// every successful swap via [`record_hotswap_phase`](Self::record_hotswap_phase)
    /// and rendered by the /metrics handler.
    hotswap_metrics: Arc<crate::metrics::types::HotSwapMetrics>,
    /// P0.12 (A5 residual): per-node cached SHA-256 (hex) of the currently
    /// loaded plugin bytes. Seeded by the launcher for every Wasm node and
    /// updated by the hot-swap handler on successful convergence. Consumed by
    /// [`verify_plugin_hash`](Self::verify_plugin_hash) so `/reconfigure`
    /// can reject callers whose mental model has diverged from the
    /// actually-running binary.
    plugin_hashes: Arc<std::sync::RwLock<HashMap<Box<str>, String>>>,
    queue_probes: Vec<QueueProbe>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueSnapshot {
    pub queue: Box<str>,
    pub depth: u64,
    pub capacity: usize,
    pub accepted: u64,
    pub dequeued: u64,
    pub processed: u64,
    pub dropped: u64,
    pub dead_lettered: u64,
    pub downstream_closed: u64,
    pub dlq_full: u64,
    pub dlq_closed: u64,
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
        f.debug_struct("SwapGuard").field("held", &self.flag.load(Ordering::Acquire)).finish()
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
        let flag = self
            .swap_in_progress
            .get(node_id)
            .ok_or_else(|| WaferError::Runtime(format!("node-not-swappable: {node_id}")))?;
        flag.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map_err(|_was_true| WaferError::Runtime(format!("swap-in-progress: {node_id}")))?;
        Ok(SwapGuard { flag: Arc::clone(flag) })
    }

    /// P0.10 (A3 residual): record a single hot-swap phase timing on the
    /// shared `hot_swap_phase_ns` histogram. Phase label is one of
    /// {compile, instantiate, signal, replacement_adopted, first_post_replacement_local_outcome}.
    pub fn record_hotswap_phase(&self, phase: &str, node_id: &str, ns: u64) {
        let key = (phase.to_owned(), node_id.to_owned());
        if let Ok(guard) = self.hotswap_metrics.phase_histogram.read()
            && let Some(h) = guard.get(&key)
        {
            h.record(ns);
            return;
        }
        if let Ok(mut guard) = self.hotswap_metrics.phase_histogram.write() {
            let h = guard.entry(key).or_insert_with(crate::metrics::types::PhaseHistogram::new);
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
            let h = guard.entry(key).or_insert_with(crate::metrics::types::PhaseHistogram::new);
            h.record(ns);
        }
    }

    /// P0.12 (A5 residual): register the SHA-256 (hex) of the plugin bytes
    /// currently loaded on `node_id`. Called by the launcher for every Wasm
    /// plugin at initial load, and by the API handler on every successful
    /// hot-swap. Shared with F2 provenance emission.
    pub fn record_plugin_hash(&self, node_id: &str, hex_hash: impl Into<String>) {
        if let Ok(mut guard) = self.plugin_hashes.write() {
            guard.insert(node_id.into(), hex_hash.into());
        }
    }

    /// Snapshot of `node_id -> sha256_hex` for every Wasm plugin currently
    /// loaded. Empty when the pipeline has no Wasm nodes (all-native
    /// baseline) or when the launcher has not yet registered any hashes.
    /// Reader‑facing single source of truth for the F2 metadata.json
    /// `wafer_plugin_hashes` field (AC2).
    #[must_use]
    pub fn plugin_hashes_snapshot(&self) -> HashMap<String, String> {
        self.plugin_hashes
            .read()
            .map(|g| g.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
            .unwrap_or_default()
    }

    /// P0.12 (A5 residual): verify that `expected_hex` matches the cached
    /// hash for `node_id`. Returns:
    /// - `Ok(())` when the node has no cached hash. The launcher registers
    ///   every Wasm plugin it loads, so this only applies to nodes without
    ///   a Wasm plugin.
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
        let guard = self
            .plugin_hashes
            .read()
            .map_err(|e| WaferError::Runtime(format!("plugin_hashes lock poisoned: {e}")))?;
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
    pub const fn hotswap_metrics(&self) -> &Arc<crate::metrics::types::HotSwapMetrics> {
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
            replacement_eligible: HashSet::from_iter(
                swappable_ids.iter().map(|id| Box::<str>::from(*id)),
            ),
            swap_in_progress,
            hotswap_metrics: Arc::new(crate::metrics::types::HotSwapMetrics::default()),
            plugin_hashes: Arc::new(std::sync::RwLock::new(HashMap::new())),
            queue_probes: Vec::new(),
        }
    }

    #[cfg(all(test, feature = "http-api"))]
    pub(crate) fn for_replacement_test(
        config: Config,
        node_id: &str,
        sender: watch::Sender<Option<SwapPayload>>,
    ) -> Result<Self> {
        let mut handle = Self::for_p0_10_test(&[node_id]);
        handle.engine = Arc::new(WaferEngine::from_engine_config(&config.engine)?);
        handle.config = config;
        handle.watch_senders.insert(node_id.into(), sender);
        Ok(handle)
    }

    /// Send a hot-swap payload to a specific node via its watch channel.
    ///
    /// # Errors
    ///
    /// Returns error if the node doesn't exist or doesn't support hot-swap.
    pub fn send_swap(&self, node_id: &str, payload: SwapPayload) -> Result<()> {
        if !self.replacement_eligible.contains(node_id) {
            return Err(WaferError::Runtime(format!("node-not-swappable: {node_id}")));
        }
        let sender = self.watch_senders.get(node_id).ok_or_else(|| {
            WaferError::Runtime(format!(
                "cannot hot-swap node '{node_id}': not found or not a Wasm node"
            ))
        })?;

        sender.send(Some(payload)).map_err(|_send_err| {
            WaferError::Runtime(format!(
                "cannot hot-swap node '{node_id}': receiver dropped (task dead?)"
            ))
        })?;

        Ok(())
    }

    /// Clear a withdrawn swap payload from the node's watch slot so its
    /// prepared Store is freed. Only clears the slot while it still holds the
    /// payload tracked by `progress`, and does not wake the runner: the
    /// withdrawal itself is carried by [`HotSwapProgress::try_withdraw`].
    pub fn retract_swap(&self, node_id: &str, progress: &Arc<HotSwapProgress>) {
        if let Some(sender) = self.watch_senders.get(node_id) {
            sender.send_if_modified(|slot| {
                if slot.as_ref().is_some_and(|payload| Arc::ptr_eq(&payload.progress(), progress)) {
                    *slot = None;
                }
                false
            });
        }
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

    #[must_use]
    pub fn queue_snapshots(&self) -> Vec<QueueSnapshot> {
        self.queue_probes
            .iter()
            .map(|probe| QueueSnapshot {
                queue: probe.queue.clone(),
                depth: probe.metrics.depth(),
                capacity: probe.capacity,
                accepted: probe.metrics.enqueued(),
                dequeued: probe.metrics.dequeued(),
                processed: self.metrics.get(&probe.queue).map_or(0, |metrics| metrics.processed()),
                dropped: probe.metrics.dropped(),
                dead_lettered: probe.metrics.dead_lettered(),
                downstream_closed: probe.metrics.downstream_closed(),
                dlq_full: probe.metrics.dlq_full(),
                dlq_closed: probe.metrics.dlq_closed(),
            })
            .collect()
    }

    /// Get loaded Wasm processing roles eligible for replacement.
    #[must_use]
    pub fn swappable_nodes(&self) -> Vec<&str> {
        self.replacement_eligible.iter().map(|node_id| &**node_id).collect()
    }

    /// Access the current configuration.
    #[must_use]
    pub const fn config(&self) -> &Config {
        &self.config
    }

    /// Access the Wasm engine (for hot-swap compilation).
    #[must_use]
    pub const fn engine(&self) -> &Arc<WaferEngine> {
        &self.engine
    }
}

/// Mark an I/O node whose `init()` failed as `Error`, cancel the pipeline so the
/// run ends instead of completing "cleanly" without that node, and build the
/// task's failure.
fn fail_io_init(
    role: &str,
    node_id: &str,
    error: &WaferError,
    state: &NodeStateTracker,
    pipeline_cancel: &CancellationToken,
) -> WaferError {
    tracing::error!(node = %node_id, error = %error, "{role} init failed; cancelling pipeline");
    state.transition_to_error();
    pipeline_cancel.cancel();
    WaferError::PluginInit { message: format!("{role} '{node_id}' init failed: {error}") }
}

pub struct PipelineOrchestrator {
    /// Supervised task set — first-failure detection via JoinSet.
    tasks: JoinSet<NodeTaskResult>,
    /// Node id of every task in `tasks`, so exits and panics name their node.
    task_nodes: HashMap<TaskId, Box<str>>,
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
    dlq_handle: Option<tokio::task::JoinHandle<Result<()>>>,
    /// Shared running flag used by API handles.
    running: Arc<AtomicBool>,
    /// Loaded Wasm processing roles eligible for replacement.
    replacement_eligible: HashSet<Box<str>>,
    /// Per-node mutation flags shared by hot-swap and reconfigure.
    swap_in_progress: HashMap<Box<str>, Arc<AtomicBool>>,
    /// Shared hot-swap metrics store, held for the /metrics handler.
    hotswap_metrics: Arc<crate::metrics::types::HotSwapMetrics>,
    /// Shared plugin-hash registry, populated on every successful hot-swap.
    plugin_hashes: Arc<std::sync::RwLock<HashMap<Box<str>, String>>>,
    queue_probes: Vec<QueueProbe>,
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
            .replacement_eligible
            .iter()
            .map(|node_id| (node_id.clone(), Arc::new(AtomicBool::new(false))))
            .collect::<HashMap<_, _>>();

        let mut orchestrator = Self {
            tasks: JoinSet::new(),
            task_nodes: HashMap::new(),
            watch_senders: build_output.watch_senders,
            cancel_token: build_output.cancel_token.clone(),
            config,
            state_trackers: build_output.state_trackers,
            metrics: build_output.metrics_map,
            engine,
            dlq_handle: None,
            running: Arc::new(AtomicBool::new(true)),
            replacement_eligible: build_output.replacement_eligible,
            swap_in_progress,
            hotswap_metrics: Arc::new(crate::metrics::types::HotSwapMetrics::default()),
            plugin_hashes: Arc::new(std::sync::RwLock::new(HashMap::new())),
            queue_probes: build_output.queue_probes,
        };

        if let Some((dlq_rx, dlq_config)) = build_output.dlq {
            let cancel = build_output.cancel_token.clone();
            let handle = tokio::spawn(run_dlq_sink(dlq_rx, cancel, dlq_config));
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
            replacement_eligible: self.replacement_eligible.clone(),
            swap_in_progress: self.swap_in_progress.clone(),
            hotswap_metrics: Arc::clone(&self.hotswap_metrics),
            plugin_hashes: Arc::clone(&self.plugin_hashes),
            queue_probes: self.queue_probes.clone(),
        }
    }

    /// Spawn all node bundles into independent tokio tasks via JoinSet.
    ///
    /// Source/Sink: init() is called inside the node task, right before its
    /// adapter loop, so init-time anchors (e.g. the bench sink's interval
    /// recorder) keep their meaning. An init failure marks the node `Error`,
    /// cancels the pipeline, and makes `run_until_complete` return `Err`.
    /// Wasm nodes: spawned with real runner loops when node instance is present.
    #[expect(
        clippy::too_many_lines,
        reason = "one match arm per node role; each arm only wires its runner loop"
    )]
    fn spawn_bundles(&mut self, bundles: Vec<crate::orchestrator::builder::NodeBundle>) {
        let hot_swap_config = self.config.engine.hot_swap.clone();
        for bundle in bundles {
            let node_id = bundle.node_id.clone();
            let cancel = bundle.cancel;
            let state = bundle.state;
            let metrics = bundle.metrics;

            let task_id = match bundle.kind {
                NodeBundleKind::Transform { receiver, senders, swap_rx, policy, node } => {
                    if let Some(transform) = node {
                        let hs_cfg = hot_swap_config.clone();
                        spawn_wasm_runner(&mut self.tasks, async move {
                            run_transform_loop_with_config(
                                transform, receiver, senders, swap_rx, policy, cancel, state,
                                metrics, hs_cfg,
                            )
                            .await;
                        })
                    } else {
                        // No compiled Wasm node — run as identity passthrough.
                        // Native transforms forward messages unchanged.
                        self.tasks
                            .spawn(async move {
                                run_passthrough_loop(receiver, senders, cancel, state, metrics)
                                    .await;
                                Ok(())
                            })
                            .id()
                    }
                }
                NodeBundleKind::Filter { receiver, senders, swap_rx, policy, node } => {
                    if let Some(filter) = node {
                        spawn_wasm_runner(&mut self.tasks, async move {
                            run_filter_loop(
                                filter, receiver, senders, swap_rx, policy, cancel, state, metrics,
                            )
                            .await;
                        })
                    } else {
                        // Native filter — forward all messages (no-op filter passes everything)
                        self.tasks
                            .spawn(async move {
                                run_passthrough_loop(receiver, senders, cancel, state, metrics)
                                    .await;
                                Ok(())
                            })
                            .id()
                    }
                }
                NodeBundleKind::Router { receiver, senders, swap_rx, policy, node } => {
                    if let Some(router) = node {
                        spawn_wasm_runner(&mut self.tasks, async move {
                            run_router_loop(
                                router, receiver, senders, swap_rx, policy, cancel, state, metrics,
                            )
                            .await;
                        })
                    } else {
                        // Native router — broadcast to all downstreams
                        self.tasks
                            .spawn(async move {
                                run_passthrough_loop(receiver, senders, cancel, state, metrics)
                                    .await;
                                Ok(())
                            })
                            .id()
                    }
                }
                NodeBundleKind::Source { source, senders } => {
                    if let Some(mut source) = source {
                        let pipeline_cancel = self.cancel_token.clone();
                        let task_node_id = node_id.clone();
                        self.tasks
                            .spawn(async move {
                                if let Err(e) = source.init().await {
                                    return Err(fail_io_init(
                                        "source",
                                        &task_node_id,
                                        &e,
                                        &state,
                                        &pipeline_cancel,
                                    ));
                                }
                                run_source_loop(source, senders, cancel, state, metrics).await
                            })
                            .id()
                    } else {
                        // No source instance (unit test without I/O construction)
                        tracing::debug!(node = %node_id, "Source task: no instance, awaiting cancel");
                        self.tasks
                            .spawn(async move {
                                cancel.cancelled().await;
                                Ok(())
                            })
                            .id()
                    }
                }
                NodeBundleKind::Sink { sink, receiver } => {
                    if let Some(mut sink) = sink {
                        let pipeline_cancel = self.cancel_token.clone();
                        let task_node_id = node_id.clone();
                        self.tasks
                            .spawn(async move {
                                if let Err(e) = sink.init().await {
                                    return Err(fail_io_init(
                                        "sink",
                                        &task_node_id,
                                        &e,
                                        &state,
                                        &pipeline_cancel,
                                    ));
                                }
                                run_sink_loop(sink, receiver, cancel, state, metrics).await
                            })
                            .id()
                    } else {
                        // No sink instance (unit test without I/O construction)
                        tracing::debug!(node = %node_id, "Sink task: no instance, awaiting cancel");
                        self.tasks
                            .spawn(async move {
                                cancel.cancelled().await;
                                Ok(())
                            })
                            .id()
                    }
                }
            };
            self.task_nodes.insert(task_id, node_id);
        }
    }

    /// Account for one finished node task and return a failure description
    /// if it failed or panicked.
    fn record_task_exit(
        &mut self,
        joined: std::result::Result<(TaskId, NodeTaskResult), JoinError>,
        phase: &str,
    ) -> Option<String> {
        match joined {
            Ok((task_id, Ok(()))) => {
                self.task_nodes.remove(&task_id);
                None
            }
            Ok((task_id, Err(e))) => {
                let node = self.take_task_node(task_id);
                tracing::error!(node = %node, error = %e, "Node task failed {phase}");
                Some(format!("node '{node}' failed: {e}"))
            }
            Err(e) => {
                let node = self.take_task_node(e.id());
                if let Some(tracker) = self.state_trackers.get(node.as_str()) {
                    tracker.transition_to_error();
                }
                tracing::error!(node = %node, error = %e, "Node task panicked {phase}");
                Some(format!("node '{node}' panicked: {e}"))
            }
        }
    }

    /// Abort the node tasks still running at the shutdown deadline and describe
    /// them. Their `close()` never ran, so their output may be incomplete.
    async fn abort_stuck_tasks(&mut self) -> String {
        let mut stuck: Vec<String> = self.task_nodes.drain().map(|(_, node)| node.into()).collect();
        stuck.sort_unstable();
        let stuck = stuck.join("', '");
        tracing::error!(
            nodes = %stuck,
            timeout_secs = SHUTDOWN_TIMEOUT.as_secs(),
            "Shutdown timeout, aborting remaining node tasks"
        );
        self.tasks.shutdown().await;
        format!(
            "node(s) '{stuck}' did not stop within {}s and were aborted",
            SHUTDOWN_TIMEOUT.as_secs()
        )
    }

    fn take_task_node(&mut self, task_id: TaskId) -> String {
        self.task_nodes.remove(&task_id).map_or_else(|| "<unknown>".to_owned(), String::from)
    }

    /// Initiate graceful shutdown.
    ///
    /// Fires the cancellation token → all runner loops break → flush retries →
    /// tasks complete → join all.
    ///
    /// Uses a timeout to prevent hanging if a task is stuck.
    ///
    /// # Errors
    ///
    /// Same as [`Self::run_until_complete`]: a node that failed, panicked, or
    /// had to be aborted at the timeout fails the shutdown.
    pub async fn shutdown(&mut self) -> Result<()> {
        tracing::info!("Pipeline shutdown initiated");
        self.cancel_token.cancel();
        let result = self.run_until_complete().await;
        tracing::info!("Pipeline shutdown complete");
        result
    }

    /// Run the pipeline until all tasks complete naturally or cancellation fires.
    ///
    /// For finite pipelines (e.g., BenchSource with `total_messages`), the source
    /// task exits after sending all messages → its downstream channel closes →
    /// transform/filter tasks see `recv() = None` and exit → sink channels close →
    /// sink tasks drain and exit → JoinSet empties → this method returns.
    ///
    /// Returns `Ok(())` if all tasks exited cleanly. Returns `Err` naming every
    /// failed node if any node task panicked or failed (e.g. a source/sink
    /// `init()` error) or the DLQ sink failed; the runtime binary turns that
    /// into a non-zero exit code.
    ///
    /// # Errors
    ///
    /// Returns [`WaferError::Runtime`] listing each failed node.
    pub async fn run_until_complete(&mut self) -> Result<()> {
        let mut failures: Vec<String> = Vec::new();

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
                                failures.push(self.abort_stuck_tasks().await);
                                break;
                            }
                            result = self.tasks.join_next_with_id() => match result {
                                Some(joined) => {
                                    failures.extend(
                                        self.record_task_exit(joined, "during shutdown"),
                                    );
                                }
                                None => break,
                            },
                        }
                    }
                    break;
                }
                result = self.tasks.join_next_with_id() => {
                    match result {
                        Some(joined) => {
                            failures.extend(
                                self.record_task_exit(joined, "during pipeline run"),
                            );
                        }
                        None => break, // All tasks completed
                    }
                }
            }
        }

        // Wait for DLQ task
        if let Some(handle) = self.dlq_handle.take() {
            match tokio::time::timeout(Duration::from_secs(5), handle).await {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(e))) => {
                    tracing::error!(error = %e, "DLQ sink failed");
                    failures.push(format!("DLQ sink failed: {e}"));
                }
                Ok(Err(e)) => {
                    tracing::error!(error = %e, "DLQ task panicked");
                    failures.push(format!("DLQ task panicked: {e}"));
                }
                Err(_) => {
                    tracing::error!("DLQ task did not exit within timeout");
                    failures.push("DLQ task did not stop within 5s".to_owned());
                }
            }
        }

        self.running.store(false, Ordering::Release);

        if failures.is_empty() {
            Ok(())
        } else {
            Err(WaferError::Runtime(format!("pipeline run failed: {}", failures.join("; "))))
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

    /// Get loaded Wasm processing roles eligible for replacement.
    #[must_use]
    pub fn swappable_nodes(&self) -> Vec<&str> {
        self.replacement_eligible.iter().map(|node_id| &**node_id).collect()
    }

    /// Access the current configuration.
    #[must_use]
    pub const fn config(&self) -> &Config {
        &self.config
    }

    /// Access the cancellation token (for external shutdown triggers).
    #[must_use]
    pub const fn cancel_token(&self) -> &CancellationToken {
        &self.cancel_token
    }

    /// Access the Wasm engine (for hot-swap compilation).
    #[must_use]
    pub const fn engine(&self) -> &Arc<WaferEngine> {
        &self.engine
    }

    /// Export aggregate per-node metrics and exact recovery samples.
    ///
    /// Only emitted on graceful shutdown, after every runner has flushed its
    /// retry buffer, so each processing node satisfies
    /// `messages_in = messages_out + filtered_out + skipped + retry_exhausted_skips
    /// + dlq_sent + dlq_lost + dropped_on_recovery + dropped_on_teardown`. `messages_in` is the node's
    /// input-queue dequeue count (0 for sources). `traps_*` count calls the
    /// host aborted, `guest_*` count errors the guest returned, and
    /// `attempts_failed` counts every failed call including retries.
    /// `error_state_seconds` is the cumulative Error to Running recovery time.
    ///
    /// # Errors
    ///
    /// Returns I/O errors while writing `per_node_metrics.csv` or `recovery.csv`.
    pub fn export_per_node_metrics(&self, dir: &std::path::Path) -> std::io::Result<()> {
        use std::io::Write;

        use crate::runner::error_policy::{ErrorCategory, TrapKind};

        let path = dir.join("per_node_metrics.csv");
        let mut f = std::fs::File::create(&path)?;
        write!(f, "node_id,messages_in,messages_out,filtered_out,traps_total")?;
        for kind in TrapKind::ALL {
            write!(f, ",traps_{}", kind.as_str())?;
        }
        for category in ErrorCategory::ALL {
            write!(f, ",guest_{category}")?;
        }
        writeln!(
            f,
            ",attempts_failed,retries,dlq_sent,dlq_lost,skipped,retry_exhausted_skips,dropped_on_recovery,dropped_on_teardown,error_state_seconds,recovery_count"
        )?;
        let mut recovery = std::fs::File::create(dir.join("recovery.csv"))?;
        writeln!(recovery, "node_id,sample_index,duration_ns")?;

        // Sort by node_id for deterministic output
        let mut node_ids: Vec<&str> = self.metrics.keys().map(|k| &**k).collect();
        node_ids.sort_unstable();

        for node_id in node_ids {
            let Some(m) = self.metrics.get(node_id) else { continue };
            let messages_in = self
                .queue_probes
                .iter()
                .find(|probe| &*probe.queue == node_id)
                .map_or(0, |probe| probe.metrics.dequeued());
            write!(
                f,
                "{node_id},{messages_in},{},{},{}",
                m.processed(),
                m.filtered_out(),
                m.traps_total()
            )?;
            for kind in TrapKind::ALL {
                write!(f, ",{}", m.traps(kind))?;
            }
            for category in ErrorCategory::ALL {
                write!(f, ",{}", m.guest_errors(category))?;
            }
            #[expect(
                clippy::as_conversions,
                clippy::cast_precision_loss,
                reason = "u64→f64 precision loss is acceptable for display-only metric (max 584 years)"
            )]
            let error_state_secs = m.recovery_ns_total() as f64 / 1_000_000_000.0;
            writeln!(
                f,
                ",{},{},{},{},{},{},{},{},{error_state_secs:.6},{}",
                m.attempts_failed(),
                m.retries(),
                m.dlq_sent(),
                m.dlq_lost(),
                m.skipped(),
                m.exhausted_skips(),
                m.dropped_on_recovery(),
                m.dropped_on_teardown(),
                m.recovery_count()
            )?;
            for (sample_index, duration_ns) in m.recovery_samples_ns().into_iter().enumerate() {
                writeln!(recovery, "{node_id},{sample_index},{duration_ns}")?;
            }
        }
        Ok(())
    }
}

/// Identity passthrough loop for native nodes without Wasm instances.
///
/// Receives messages, records metrics, and forwards unchanged to all downstreams.
/// Used for native-transform (passthrough/uppercase) and nodes without .wasm.
async fn run_passthrough_loop(
    mut receiver: TrackedReceiver,
    senders: Vec<DownstreamSender>,
    cancel: CancellationToken,
    state: Arc<NodeStateTracker>,
    metrics: Arc<NodeMetrics>,
) {
    use crate::node::ProcessingGuard;

    state.transition_to_running();
    let cancelled = cancel.cancelled();
    tokio::pin!(cancelled);

    loop {
        let envelope = tokio::select! {
            biased;
            () = &mut cancelled => break,
            msg = receiver.recv() => match msg {
                Some(e) => e,
                None => break,
            },
        };

        let guard = ProcessingGuard::enter(&state);
        // Identity: forward unchanged
        send_downstream(&senders, envelope).await;
        drop(guard);
        metrics.record_processed();
    }
}

/// Configured DLQ sink that drains envelopes until cancellation or sender closure.
async fn run_dlq_sink(
    mut receiver: tokio::sync::mpsc::Receiver<DlqEnvelope>,
    cancel: CancellationToken,
    config: crate::config::DeadLetterConfig,
) -> Result<()> {
    match config {
        crate::config::DeadLetterConfig::File { path, .. } => {
            let file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
            let mut writer = BufWriter::new(file);
            while let Some(envelope) = recv_or_cancel(&mut receiver, &cancel).await {
                writer.write_all(&envelope.to_json_bytes().map_err(|error| {
                    WaferError::Runtime(format!("failed to serialize DLQ record: {error}"))
                })?)?;
                writer.write_all(b"\n")?;
            }
            writer.flush()?;
        }
        crate::config::DeadLetterConfig::Mqtt { broker, port, topic, tls, auth, .. } => {
            let mut options = rumqttc::MqttOptions::new("wafer-dlq", broker, port);
            options.set_keep_alive(Duration::from_secs(30));
            if let Some(auth) = auth {
                options.set_credentials(auth.username, auth.password);
            }
            if let Some(tls) = tls {
                let transport = if let Some(ca_path) = tls.ca {
                    let ca = std::fs::read(ca_path)?;
                    let client_auth = match (tls.cert, tls.key) {
                        (Some(cert), Some(key)) => {
                            Some((std::fs::read(cert)?, std::fs::read(key)?))
                        }
                        (None, None) => None,
                        _ => {
                            return Err(WaferError::Config(crate::error::ConfigError::Message(
                                "dead_letter TLS cert and key must be configured together"
                                    .to_string(),
                            )));
                        }
                    };
                    rumqttc::Transport::tls(ca, client_auth, None)
                } else {
                    rumqttc::Transport::tls_with_default_config()
                };
                options.set_transport(transport);
            }
            let (client, mut eventloop) = rumqttc::AsyncClient::new(options, 10);
            let eventloop_handle = tokio::spawn(async move {
                loop {
                    match eventloop.poll().await {
                        Ok(rumqttc::Event::Outgoing(rumqttc::Outgoing::Disconnect)) => break,
                        Ok(_) => {}
                        Err(error) => return Err(error),
                    }
                }
                Ok::<(), rumqttc::ConnectionError>(())
            });
            while let Some(envelope) = recv_or_cancel(&mut receiver, &cancel).await {
                client
                    .publish(
                        &topic,
                        rumqttc::QoS::AtLeastOnce,
                        false,
                        envelope.to_json_bytes().map_err(|error| {
                            WaferError::Runtime(format!("failed to serialize DLQ record: {error}"))
                        })?,
                    )
                    .await
                    .map_err(|error| {
                        WaferError::Runtime(format!("DLQ MQTT publish failed: {error}"))
                    })?;
            }
            client.disconnect().await.map_err(|error| {
                WaferError::Runtime(format!("DLQ MQTT disconnect failed: {error}"))
            })?;
            eventloop_handle
                .await
                .map_err(|error| {
                    WaferError::Runtime(format!("DLQ MQTT event loop panicked: {error}"))
                })?
                .map_err(|error| {
                    WaferError::Runtime(format!("DLQ MQTT event loop failed: {error}"))
                })?;
        }
    }
    Ok(())
}

async fn recv_or_cancel(
    receiver: &mut tokio::sync::mpsc::Receiver<DlqEnvelope>,
    cancel: &CancellationToken,
) -> Option<DlqEnvelope> {
    tokio::select! {
        biased;
        () = cancel.cancelled() => receiver.try_recv().ok(),
        message = receiver.recv() => message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use testcontainers::runners::AsyncRunner;
    use testcontainers_modules::mosquitto::Mosquitto;
    use tokio::sync::{Barrier, mpsc};

    use crate::config::{
        Config, DeadLetterConfig, EdgeDef, EngineConfig, NodeDef, SinkDef, SourceDef,
        StdinSourceConfig, StdoutSinkConfig, WasmNodeDef,
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

    fn ensure_docker_host() {
        if std::env::var_os("DOCKER_HOST").is_some() {
            return;
        }
        let path =
            format!("{}/.colima/default/docker.sock", std::env::var("HOME").unwrap_or_default());
        if std::path::Path::new(&path).exists() {
            // SAFETY: this test setup completes before testcontainers creates its Docker client.
            unsafe { std::env::set_var("DOCKER_HOST", format!("unix://{path}")) };
        }
    }

    fn dlq_test_envelope(payload: &str) -> DlqEnvelope {
        let original = RuntimeEnvelope::from_string("source", payload);
        DlqEnvelope {
            timestamp: 1,
            source_node: "source".into(),
            error_category: None,
            error_message: "destination queue full".to_string(),
            retry_count: 0,
            reason: crate::runner::error_policy::DlqReason::QueueFull {
                edge: "source:default->sink:default".into(),
            },
            trace_id: None,
            parent_id: None,
            original,
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
                ("sink".to_string(), NodeDef::Sink(SinkDef::Stdout(StdoutSinkConfig::default()))),
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
                ("sink".to_string(), NodeDef::Sink(SinkDef::Stdout(StdoutSinkConfig::default()))),
            ]),
            edges: vec![edge("src", "sink")],
            ..Default::default()
        }
    }

    /// Test sink that can fail `init()` or panic on its first message.
    struct FaultySink {
        fail_init: bool,
        hang_on_close: bool,
    }

    impl crate::node::Lifecycle for FaultySink {
        fn id(&self) -> &'static str {
            "sink"
        }

        fn node_type(&self) -> &'static str {
            "faulty-sink"
        }

        fn validate(&self) -> Result<()> {
            Ok(())
        }

        fn init(&mut self) -> std::pin::Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
            let fail = self.fail_init;
            Box::pin(async move {
                if fail {
                    Err(WaferError::PluginInit { message: "broker unreachable".into() })
                } else {
                    Ok(())
                }
            })
        }

        fn close(&mut self) -> std::pin::Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
            let hang = self.hang_on_close;
            Box::pin(async move {
                if hang {
                    std::future::pending::<()>().await;
                }
                Ok(())
            })
        }
    }

    impl crate::node::Sink for FaultySink {
        fn collect(
            &mut self,
            _envelope: RuntimeEnvelope,
        ) -> std::pin::Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
            Box::pin(async { panic!("sink bug") })
        }
    }

    fn faulty_sink_orchestrator(
        sink: FaultySink,
    ) -> (tokio::sync::mpsc::Sender<RuntimeEnvelope>, PipelineOrchestrator) {
        let config = source_sink_config();
        let engine = Arc::new(WaferEngine::new().expect("engine"));
        let (source_tx, source) = ChannelSource::new("src");
        let mut sources: HashMap<String, Box<dyn crate::node::Source + Send>> = HashMap::new();
        sources.insert("src".to_string(), Box::new(source));
        let mut sinks: HashMap<String, Box<dyn crate::node::Sink + Send>> = HashMap::new();
        sinks.insert("sink".to_string(), Box::new(sink));
        let build_output = build_pipeline_with_io(&config, sources, sinks).expect("build");
        (source_tx, PipelineOrchestrator::from_build_output(build_output, config, engine))
    }

    #[tokio::test]
    async fn node_panic_fails_the_run_and_names_the_node() {
        let (source_tx, mut orch) =
            faulty_sink_orchestrator(FaultySink { fail_init: false, hang_on_close: false });
        source_tx.send(RuntimeEnvelope::from_string("test", "boom")).await.expect("send");
        drop(source_tx);

        let err = tokio::time::timeout(Duration::from_secs(5), orch.run_until_complete())
            .await
            .expect("run completes")
            .expect_err("a panicked node must fail the run");

        assert!(err.to_string().contains("node 'sink' panicked"), "{err}");
        assert_eq!(orch.node_state("sink"), Some(NodeState::Error));
    }

    #[tokio::test]
    async fn sink_init_failure_cancels_and_fails_the_run() {
        // The source never reaches EOF, so only the init failure can end the run.
        let (_source_tx, mut orch) =
            faulty_sink_orchestrator(FaultySink { fail_init: true, hang_on_close: false });

        let err = tokio::time::timeout(Duration::from_secs(5), orch.run_until_complete())
            .await
            .expect("init failure must end the run instead of hanging")
            .expect_err("a sink whose init failed must fail the run");

        assert!(err.to_string().contains("sink 'sink' init failed"), "{err}");
        assert_eq!(orch.node_state("sink"), Some(NodeState::Error));
        assert!(orch.cancel_token().is_cancelled());
    }

    #[tokio::test(start_paused = true)]
    async fn node_stuck_at_shutdown_is_aborted_and_fails_the_run() {
        let (_source_tx, mut orch) =
            faulty_sink_orchestrator(FaultySink { fail_init: false, hang_on_close: true });

        let err = orch.shutdown().await.expect_err("an aborted node must fail the shutdown");

        let err = err.to_string();
        let expected =
            format!("node(s) 'sink' did not stop within {}s", SHUTDOWN_TIMEOUT.as_secs());
        assert!(err.contains(&expected), "{err}");
        assert!(!orch.is_running());
    }

    #[tokio::test]
    async fn configured_file_dlq_persists_json_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("dlq.jsonl");
        let (tx, rx) = tokio::sync::mpsc::channel(2);
        let cancel = CancellationToken::new();
        tx.send(dlq_test_envelope("failed-message")).await.expect("enqueue DLQ record");
        drop(tx);

        run_dlq_sink(
            rx,
            cancel,
            DeadLetterConfig::File { path: path.to_string_lossy().into_owned(), queue_capacity: 2 },
        )
        .await
        .expect("configured file DLQ must persist the record");

        let contents = std::fs::read_to_string(path).expect("configured DLQ file");
        let record: serde_json::Value =
            serde_json::from_str(contents.trim()).expect("one JSONL record");
        assert_eq!(record["source_node"], "source");
        assert_eq!(record["reason"]["type"], "queue_full");
        assert_eq!(record["original"]["retry_count"], 0);
    }

    #[tokio::test]
    async fn cancelled_file_dlq_drains_all_buffered_records() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("dlq.jsonl");
        let (tx, rx) = tokio::sync::mpsc::channel(3);
        tx.send(dlq_test_envelope("first")).await.expect("first DLQ record");
        tx.send(dlq_test_envelope("second")).await.expect("second DLQ record");
        drop(tx);
        let cancel = CancellationToken::new();
        cancel.cancel();

        run_dlq_sink(
            rx,
            cancel,
            DeadLetterConfig::File { path: path.to_string_lossy().into_owned(), queue_capacity: 3 },
        )
        .await
        .expect("cancellation must drain buffered file DLQ records");

        let lines: Vec<_> = std::fs::read_to_string(path)
            .expect("configured DLQ file")
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("DLQ JSONL record"))
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["original"]["payload"], "Zmlyc3Q=");
        assert_eq!(lines[1]["original"]["payload"], "c2Vjb25k");
    }

    #[tokio::test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "the broker-backed delivery assertions must fail this integration test"
    )]
    async fn configured_mqtt_dlq_delivers_serialized_record() -> anyhow::Result<()> {
        ensure_docker_host();
        let broker = Mosquitto::default().start().await?;
        let host = broker.get_host().await?;
        let port = broker.get_host_port_ipv4(1883).await?;
        tokio::time::sleep(Duration::from_secs(1)).await;
        let topic = format!("wafer/core-dlq/{}", std::process::id());

        let options = rumqttc::MqttOptions::new(
            format!("wafer-core-dlq-sub-{}", std::process::id()),
            host.to_string(),
            port,
        );
        let (subscriber, mut eventloop) = rumqttc::AsyncClient::new(options, 10);
        subscriber.subscribe(&topic, rumqttc::QoS::AtLeastOnce).await?;
        loop {
            if matches!(
                eventloop.poll().await?,
                rumqttc::Event::Incoming(rumqttc::Packet::SubAck(_))
            ) {
                break;
            }
        }

        let (tx, rx) = tokio::sync::mpsc::channel(1);
        tx.send(dlq_test_envelope("brokered-message")).await?;
        drop(tx);
        let sink = tokio::spawn(run_dlq_sink(
            rx,
            CancellationToken::new(),
            DeadLetterConfig::Mqtt {
                broker: host.to_string(),
                port,
                topic: topic.clone(),
                queue_capacity: 1,
                tls: None,
                auth: None,
            },
        ));

        let payload = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let rumqttc::Event::Incoming(rumqttc::Packet::Publish(message)) =
                    eventloop.poll().await?
                {
                    return Ok::<_, rumqttc::ConnectionError>(message.payload.to_vec());
                }
            }
        })
        .await??;
        sink.await??;

        let record: serde_json::Value = serde_json::from_slice(&payload)?;
        assert_eq!(record["source_node"], "source");
        assert_eq!(record["reason"]["type"], "queue_full");
        assert_eq!(record["error_message"], "destination queue full");
        assert_eq!(record["original"]["payload"], "YnJva2VyZWQtbWVzc2FnZQ==");
        Ok(())
    }

    #[tokio::test]
    async fn configured_mqtt_dlq_surfaces_event_loop_failure() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve loopback port");
        let port = listener.local_addr().expect("loopback address").port();
        drop(listener);

        let (tx, rx) = tokio::sync::mpsc::channel(2);
        tx.send(dlq_test_envelope("failed-message")).await.expect("enqueue DLQ record");
        drop(tx);

        let result = tokio::time::timeout(
            Duration::from_secs(1),
            run_dlq_sink(
                rx,
                CancellationToken::new(),
                DeadLetterConfig::Mqtt {
                    broker: "127.0.0.1".to_string(),
                    port,
                    topic: "wafer/test-dlq".to_string(),
                    queue_capacity: 2,
                    tls: None,
                    auth: None,
                },
            ),
        )
        .await
        .expect("closed loopback broker must not hang the DLQ sink");

        assert!(result.is_err(), "MQTT delivery failure must not report success");
    }

    #[tokio::test]
    async fn test_build_creates_correct_watch_senders() {
        let config = test_config();
        let engine = Arc::new(WaferEngine::new().expect("engine"));
        let build_output = build_pipeline(&config).expect("build");

        let orch = PipelineOrchestrator::from_build_output(build_output, config, engine);

        assert_eq!(orch.wasm_node_count(), 1);
        assert!(orch.swappable_nodes().is_empty(), "unloaded configured roles are not eligible");
    }

    #[tokio::test]
    async fn replacement_eligibility_uses_loaded_wasm_roles() {
        let config = test_config();
        let engine = Arc::new(WaferEngine::new().expect("engine"));
        let mut build_output = build_pipeline(&config).expect("build");
        build_output.replacement_eligible.insert("t1".into());

        let orch = PipelineOrchestrator::from_build_output(build_output, config, engine);
        assert_eq!(orch.swappable_nodes(), vec!["t1"]);
        drop(orch.handle().try_begin_swap("t1").expect("eligible Wasm role"));
        let _ = orch.handle().try_begin_swap("src").expect_err("source rejected");
        let _ = orch.handle().try_begin_swap("sink").expect_err("sink rejected");
    }

    #[tokio::test]
    async fn wasm_runner_uses_tokio_executor() {
        let runtime_thread = std::thread::current().id();
        let mut tasks = JoinSet::new();
        spawn_wasm_runner(&mut tasks, async move {
            assert_eq!(std::thread::current().id(), runtime_thread);
            tokio::task::yield_now().await;
            assert_eq!(std::thread::current().id(), runtime_thread);
        });

        tasks.join_next().await.expect("runner task").expect("runner result").expect("runner exit");
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
    async fn recovery_export_preserves_nanosecond_samples() {
        let config = test_config();
        let engine = Arc::new(WaferEngine::new().expect("engine"));
        let build_output = build_pipeline(&config).expect("build");
        let mut orchestrator =
            PipelineOrchestrator::from_build_output(build_output, config, engine);

        let metrics = orchestrator.node_metrics("src").expect("source metrics");
        metrics.record_recovery(12_345);
        metrics.record_recovery(67_890);
        metrics.record_exhausted_skip();
        let output = tempfile::tempdir().expect("tempdir");
        orchestrator.export_per_node_metrics(output.path()).expect("export metrics");

        let recovery = std::fs::read_to_string(output.path().join("recovery.csv"))
            .expect("read recovery samples");
        assert!(recovery.contains("src,0,12345"));
        assert!(recovery.contains("src,1,67890"));
        let node_metrics = std::fs::read_to_string(output.path().join("per_node_metrics.csv"))
            .expect("read node metrics");
        let mut lines = node_metrics.lines();
        let header: Vec<&str> = lines.next().expect("header").split(',').collect();
        let src: Vec<&str> =
            lines.find(|line| line.starts_with("src,")).expect("src row").split(',').collect();
        assert_eq!(header.len(), src.len(), "every row matches the header");
        let column = |name: &str| src[header.iter().position(|h| *h == name).expect(name)];
        assert_eq!(column("retry_exhausted_skips"), "1");
        assert_eq!(column("error_state_seconds"), "0.000080");
        assert_eq!(column("recovery_count"), "2");

        orchestrator.shutdown().await.expect("shutdown");
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

        let mut sources: HashMap<String, Box<dyn crate::node::Source + Send>> = HashMap::new();
        sources.insert("src".to_string(), Box::new(source));
        let mut sinks: HashMap<String, Box<dyn crate::node::Sink + Send>> = HashMap::new();
        sinks.insert("sink".to_string(), Box::new(sink));

        let build_output = build_pipeline_with_io(&config, sources, sinks).expect("build");
        let mut orch = PipelineOrchestrator::from_build_output(build_output, config, engine);

        assert_eq!(orch.task_count(), 2);

        for i in 0..5 {
            source_tx.send(RuntimeEnvelope::from_string("test", format!("msg-{i}"))).await.unwrap();
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
        assert_eq!(
            orch.handle().queue_snapshots(),
            vec![QueueSnapshot {
                queue: "sink".into(),
                depth: 0,
                capacity: 1024,
                accepted: 5,
                dequeued: 5,
                processed: 5,
                dropped: 0,
                dead_lettered: 0,
                downstream_closed: 0,
                dlq_full: 0,
                dlq_closed: 0,
            }]
        );

        orch.shutdown().await.expect("shutdown");
        assert!(!orch.is_running());
    }

    #[tokio::test]
    async fn test_run_until_complete_finite_pipeline() {
        let config = source_sink_config();
        let engine = Arc::new(WaferEngine::new().expect("engine"));

        let (source_tx, source) = ChannelSource::new("src");
        let (sink, mut sink_rx) = ChannelSink::new("sink");

        let mut sources: HashMap<String, Box<dyn crate::node::Source + Send>> = HashMap::new();
        sources.insert("src".to_string(), Box::new(source));
        let mut sinks: HashMap<String, Box<dyn crate::node::Sink + Send>> = HashMap::new();
        sinks.insert("sink".to_string(), Box::new(sink));

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

        tokio::spawn(async move { while sink_rx.recv().await.is_some() {} });

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
        let err =
            handle.try_begin_swap("transform").expect_err("concurrent second must be rejected");
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn replacement_guard_serializes_swap_and_reconfigure() {
        let handle = Arc::new(PipelineHandle::for_p0_10_test(&["transform"]));
        let start = Arc::new(Barrier::new(3));
        let release = Arc::new(Barrier::new(3));
        let (result_tx, mut result_rx) = mpsc::channel(2);
        let mut claimants = Vec::new();

        for operation in ["hot-swap", "reconfigure"] {
            let handle = Arc::clone(&handle);
            let start = Arc::clone(&start);
            let release = Arc::clone(&release);
            let result_tx = result_tx.clone();
            claimants.push(tokio::spawn(async move {
                start.wait().await;
                let result = handle.try_begin_swap("transform");
                let claim = result.as_ref().map(|_| operation).map_err(ToString::to_string);
                result_tx.send(claim).await.expect("test receiver remains available");
                release.wait().await;
                drop(result);
            }));
        }
        drop(result_tx);

        start.wait().await;
        let first = result_rx.recv().await.expect("first claimant result");
        let second = result_rx.recv().await.expect("second claimant result");
        let results = [first, second];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results.iter().filter(|result| result.is_err()).count(),
            1,
            "one same-node claimant must receive an early conflict",
        );
        assert!(
            results
                .iter()
                .find_map(|result| result.as_ref().err())
                .is_some_and(|error| error.contains("swap-in-progress")),
            "the rejected cross-operation claimant must see swap-in-progress",
        );

        release.wait().await;
        for claimant in claimants {
            claimant.await.expect("claimant task");
        }
    }

    /// Non-swappable / unknown node yields node-not-swappable, distinct
    /// from the in-progress error so the API handler can map it to 404
    /// instead of 409.
    #[test]
    fn swap_guard_reports_unknown_node() {
        let handle = PipelineHandle::for_p0_10_test(&["transform"]);
        let err = handle.try_begin_swap("does-not-exist").expect_err("unknown node id must fail");
        assert!(
            err.to_string().contains("node-not-swappable"),
            "expected node-not-swappable error, got: {err}"
        );
    }

    /// AC1: recording every phase populates six independent series in
    /// the phase histogram. The Prometheus emitter reads from this map.
    #[test]
    #[expect(
        clippy::significant_drop_tightening,
        reason = "RwLockReadGuard held for assertions across the for loop — intentional"
    )]
    fn phase_histogram_records_local_replacement_phases() {
        let handle = PipelineHandle::for_p0_10_test(&["transform"]);

        for (phase, ns) in [
            ("compile", 50_000_000_u64),
            ("instantiate", 5_000_000_u64),
            ("signal", 1_000_u64),
            ("replacement_adopted", 50_000_u64),
            ("first_post_replacement_local_outcome", 200_000_u64),
        ] {
            handle.record_hotswap_phase(phase, "transform", ns);
        }

        let hs = handle.hotswap_metrics();
        let guard = hs.phase_histogram.read().unwrap();
        assert_eq!(guard.len(), 5, "expected exactly five local (phase, node) series");
        for phase in [
            "compile",
            "instantiate",
            "signal",
            "replacement_adopted",
            "first_post_replacement_local_outcome",
        ] {
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
        let err =
            handle.verify_plugin_hash("transform", "deadbeef").expect_err("mismatch must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("plugin-hash-mismatch"),
            "error must start with plugin-hash-mismatch, got: {msg}"
        );

        // Case-insensitive match — hex encoders differ on case.
        assert!(
            handle.verify_plugin_hash("transform", &"A".repeat(64)).is_ok(),
            "hash match must be case-insensitive (hex encoders vary)"
        );

        // Exact-case match still works.
        assert!(
            handle.verify_plugin_hash("transform", &"a".repeat(64)).is_ok(),
            "exact-case hash match must pass"
        );
    }
}
