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
}

impl PipelineHandle {
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
                        plugin: "test.wasm".to_string(),
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
}
