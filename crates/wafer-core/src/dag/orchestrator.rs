// Duration nanosecond casts: 2^64 ns = ~585 years, truncation is acceptable
#![allow(clippy::cast_possible_truncation)]

//! DAG orchestrator for multi-node pipeline execution.
//!
//! Manages graph topology, node lifecycle, queue wiring, and coordinated
//! async execution. Supports graceful shutdown via `CancellationToken`.
//!
//! The `run()` method takes `&self` (not `&mut self`), enabling the orchestrator
//! to be wrapped in `Arc` for sharing with API handlers.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;
use wafer_types::{PipelineEvent, PipelineState};

#[cfg(feature = "http-api")]
use crate::metrics::MetricsRegistry;

use crate::config::{DagConfig, OverflowPolicy};
use crate::error::{Result, WaferError};
use crate::factory::FactoryContext;
use crate::node::AnyNode;
use crate::queue::{QueueReceiver, QueueSender, RuntimeEnvelope};

use super::graph::DagGraph;
use super::hotswap::{HotSwapCoordinator, SwapError, SwapMetrics};

/// Bundles sender, overflow policy, and edge name for queue overflow handling.
#[derive(Clone)]
pub struct EdgeSendInfo {
    /// Output port name (e.g., "default", "high", "low")
    pub port: String,
    pub sender: QueueSender<RuntimeEnvelope>,
    pub overflow_policy: OverflowPolicy,
    /// Format: "from_node:port->to_node:port"
    pub edge_name: String,
}

/// State consumed during `run()` - can only be used once.
pub struct RunState {
    pub queue_senders: HashMap<(String, String), QueueSender<RuntimeEnvelope>>,
    pub queue_receivers: HashMap<(String, String), QueueReceiver<RuntimeEnvelope>>,
}

/// Shared state for control operations and metrics.
pub struct ControlState {
    pub name: String,
    pub created_at: Instant,
    pub state: Mutex<PipelineState>,
    pub messages_processed: AtomicU64,
    pub messages_failed: AtomicU64,
    pub event_tx: broadcast::Sender<PipelineEvent>,
    /// DLQ sender, late-initialized after orchestrator construction.
    pub dlq_sender: Mutex<Option<QueueSender<RuntimeEnvelope>>>,
    #[cfg(feature = "http-api")]
    pub metrics_registry: MetricsRegistry,
}

impl ControlState {
    #[must_use]
    pub fn new(name: String) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            name,
            created_at: Instant::now(),
            state: Mutex::new(PipelineState::Starting),
            messages_processed: AtomicU64::new(0),
            messages_failed: AtomicU64::new(0),
            event_tx,
            dlq_sender: Mutex::new(None),
            #[cfg(feature = "http-api")]
            metrics_registry: MetricsRegistry::new(),
        }
    }

    /// Set the DLQ sender for routing failed messages.
    pub async fn set_dlq_sender(&self, sender: QueueSender<RuntimeEnvelope>) {
        let mut dlq = self.dlq_sender.lock().await;
        *dlq = Some(sender);
    }

    /// Create new control state with global labels for metrics.
    #[cfg(feature = "http-api")]
    #[must_use]
    #[expect(dead_code, reason = "public API for metrics/inspection")]
    pub fn with_labels(name: String, labels: HashMap<String, String>) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            name,
            created_at: Instant::now(),
            state: Mutex::new(PipelineState::Starting),
            messages_processed: AtomicU64::new(0),
            messages_failed: AtomicU64::new(0),
            event_tx,
            dlq_sender: Mutex::new(None),
            metrics_registry: MetricsRegistry::with_labels(labels),
        }
    }

    pub fn uptime_secs(&self) -> u64 {
        self.created_at.elapsed().as_secs()
    }
}

/// DAG orchestrator managing graph topology, node execution, and inter-node communication.
///
/// Uses internal mutability (`run()` takes `&self`) so it can be shared via `Arc`
/// between the execution task and API handlers.
pub struct DagOrchestrator {
    pub(super) dag_graph: DagGraph,
    pub(super) config: DagConfig,

    /// `None` if constructed programmatically without a file.
    pub(super) config_path: Option<PathBuf>,

    /// Protected by Mutex for interior mutability (WASM stores are not thread-safe).
    pub(super) nodes: Mutex<HashMap<String, Arc<Mutex<AnyNode>>>>,

    /// `None` after `run()` has been called.
    pub(super) run_state: Mutex<Option<RunState>>,

    pub(super) cancel_token: CancellationToken,
    pub(super) control_state: Arc<ControlState>,

    /// Uses Mutex because we set it after construction.
    pub(super) factory_ctx: Mutex<Option<FactoryContext>>,

    /// Prevents concurrent swaps on the same node.
    pub(super) swap_locks: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl fmt::Debug for DagOrchestrator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DagOrchestrator")
            .field("name", &self.control_state.name)
            .field("node_count", &self.dag_graph.node_count())
            .field("edge_count", &self.dag_graph.edge_count())
            .field("topo_order", &self.dag_graph.topo_order())
            .finish_non_exhaustive()
    }
}

impl DagOrchestrator {
    fn parse_node_port(key: &str) -> (&str, &str) {
        key.split_once(':').unwrap_or((key, "default"))
    }

    /// Find the overflow policy for an edge. Returns `OverflowPolicy::Slow` if not found.
    fn find_edge_overflow_policy(&self, from_key: &str, to_key: &str) -> OverflowPolicy {
        let (from_node, from_port) = Self::parse_node_port(from_key);
        let (to_node, to_port) = Self::parse_node_port(to_key);

        for edge in &self.config.edges {
            let edge_from_port = edge.from_port.as_deref().unwrap_or("default");
            let edge_to_port = edge.to_port.as_deref().unwrap_or("default");

            if edge.from == from_node
                && edge.to == to_node
                && edge_from_port == from_port
                && edge_to_port == to_port
            {
                return edge.overflow;
            }
        }

        // Default if not found
        OverflowPolicy::Slow
    }

    /// Run the DAG pipeline. Can only be called once (run state is consumed).
    ///
    /// Initializes nodes in topo order, spawns async tasks, waits for completion,
    /// then closes nodes in reverse order.
    pub async fn run(&self) -> Result<()> {
        self.validate_nodes_registered().await?;

        // Take the run state - can only succeed once
        let mut run_state = {
            let mut guard = self.run_state.lock().await;
            guard.take().ok_or_else(|| {
                WaferError::Runtime("Pipeline has already been run (run state consumed)".into())
            })?
        };

        self.set_pipeline_state(PipelineState::Running).await;

        let nodes_snapshot: HashMap<String, Arc<Mutex<AnyNode>>> = {
            let nodes = self.nodes.lock().await;
            nodes.clone()
        };

        self.init_nodes_in_order(&nodes_snapshot).await?;
        let handles = self.spawn_node_tasks(&nodes_snapshot, &mut run_state);

        // Clear remaining senders so receivers see channel close
        run_state.queue_senders.clear();
        drop(run_state);

        self.wait_for_tasks(handles).await;
        self.shutdown_nodes(&nodes_snapshot).await;

        Ok(())
    }

    async fn set_pipeline_state(&self, new_state: PipelineState) {
        let mut state = self.control_state.state.lock().await;
        *state = new_state;
    }

    async fn init_nodes_in_order(
        &self,
        nodes_snapshot: &HashMap<String, Arc<Mutex<AnyNode>>>,
    ) -> Result<()> {
        for node_id in self.dag_graph.topo_order() {
            if let Some(node) = nodes_snapshot.get(node_id) {
                let mut locked = node.lock().await;
                if let Err(e) = locked.init().await {
                    tracing::error!(node = %node_id, error = %e, "Node init failed");
                    self.set_pipeline_state(PipelineState::Error).await;
                    return Err(e);
                }
                tracing::debug!(node = %node_id, "Node initialized");
            }
        }
        Ok(())
    }

    fn spawn_node_tasks(
        &self,
        nodes_snapshot: &HashMap<String, Arc<Mutex<AnyNode>>>,
        run_state: &mut RunState,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        let mut handles = Vec::new();

        for node_id in self.dag_graph.topo_order() {
            let output_senders = self.collect_output_senders(node_id, run_state);
            let input_receivers = Self::collect_input_receivers(node_id, run_state);

            if let Some(node_arc) = nodes_snapshot.get(node_id).cloned() {
                let node_id_owned = node_id.clone();
                let cancel_token = self.cancel_token.clone();
                let control_state = Arc::clone(&self.control_state);

                let handle = tokio::spawn(async move {
                    Self::run_node_loop(
                        node_id_owned,
                        node_arc,
                        input_receivers,
                        output_senders,
                        cancel_token,
                        control_state,
                    )
                    .await;
                });
                handles.push(handle);
            }
        }

        handles
    }

    fn collect_output_senders(&self, node_id: &str, run_state: &RunState) -> Vec<EdgeSendInfo> {
        run_state
            .queue_senders
            .iter()
            .filter_map(|((from_key, to_key), sender)| {
                let (from_node, from_port) = Self::parse_node_port(from_key);
                if from_node == node_id {
                    let overflow_policy = self.find_edge_overflow_policy(from_key, to_key);
                    let edge_name = format!("{from_key}->{to_key}");
                    Some(EdgeSendInfo {
                        port: from_port.to_string(),
                        sender: sender.clone(),
                        overflow_policy,
                        edge_name,
                    })
                } else {
                    None
                }
            })
            .collect()
    }

    fn collect_input_receivers(
        node_id: &str,
        run_state: &mut RunState,
    ) -> Vec<(String, QueueReceiver<RuntimeEnvelope>)> {
        let input_keys_with_ports: Vec<_> = run_state
            .queue_receivers
            .keys()
            .filter_map(|(from_key, to_key)| {
                let (to_node, to_port) = Self::parse_node_port(to_key);
                if to_node == node_id {
                    Some(((from_key.clone(), to_key.clone()), to_port.to_string()))
                } else {
                    None
                }
            })
            .collect();

        input_keys_with_ports
            .into_iter()
            .filter_map(|(key, port)| run_state.queue_receivers.remove(&key).map(|rx| (port, rx)))
            .collect()
    }

    async fn wait_for_tasks(&self, handles: Vec<tokio::task::JoinHandle<()>>) {
        for handle in handles {
            if let Err(e) = handle.await {
                tracing::error!(error = %e, "Node task panicked");
            }
        }
    }

    async fn shutdown_nodes(&self, nodes_snapshot: &HashMap<String, Arc<Mutex<AnyNode>>>) {
        self.set_pipeline_state(PipelineState::Draining).await;

        for node_id in self.dag_graph.topo_order().iter().rev() {
            if let Some(node) = nodes_snapshot.get(node_id) {
                let mut locked = node.lock().await;
                if let Err(e) = locked.close().await {
                    tracing::warn!(node = %node_id, error = %e, "Node close failed");
                }
                tracing::debug!(node = %node_id, "Node closed");
            }
        }

        // Clean up epoch tickers
        if let Some(ref ctx) = *self.factory_ctx.lock().await {
            ctx.abort_tickers();
        }

        self.set_pipeline_state(PipelineState::Stopped).await;
    }

    #[must_use]
    pub fn topo_order(&self) -> &[String] {
        self.dag_graph.topo_order()
    }

    #[must_use]
    pub fn config(&self) -> &DagConfig {
        &self.config
    }

    #[must_use]
    pub fn node_count(&self) -> usize {
        self.dag_graph.node_count()
    }

    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.dag_graph.edge_count()
    }

    /// Clone of the cancellation token for external shutdown triggering.
    #[must_use]
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel_token.clone()
    }

    /// Request graceful shutdown. Nodes complete their current operation before stopping.
    pub fn shutdown(&self) {
        tracing::info!("Shutdown requested");
        self.cancel_token.cancel();
    }

    /// Hot-swap a WASM node with a new component using drain-and-flip.
    pub async fn hot_swap(
        &self,
        node_id: &str,
        new_wasm_path: impl AsRef<Path>,
    ) -> Result<SwapMetrics> {
        self.hot_swap_with_timeout(
            node_id,
            new_wasm_path,
            Duration::from_millis(super::hotswap::DEFAULT_DRAIN_TIMEOUT_MS),
        )
        .await
    }

    /// Hot-swap with a custom drain timeout.
    pub async fn hot_swap_with_timeout(
        &self,
        node_id: &str,
        new_wasm_path: impl AsRef<Path>,
        drain_timeout: Duration,
    ) -> Result<SwapMetrics> {
        let new_wasm_path = new_wasm_path.as_ref().to_path_buf();

        tracing::info!(
            node = %node_id,
            path = %new_wasm_path.display(),
            timeout_ms = %drain_timeout.as_millis(),
            "Hot-swap requested"
        );

        let (old_tracker, swap_lock) = {
            let nodes = self.nodes.lock().await;
            let node_arc = nodes
                .get(node_id)
                .ok_or_else(|| WaferError::from(SwapError::NodeNotFound(node_id.to_string())))?;

            let node = node_arc.lock().await;
            if !node.is_swappable() {
                return Err(WaferError::from(SwapError::NotSwappable(node_id.to_string())));
            }

            let tracker = node.state_tracker_clone();
            drop(node);

            let mut swap_locks = self.swap_locks.lock().await;
            let lock = swap_locks
                .entry(node_id.to_string())
                .or_insert_with(|| Arc::new(AtomicBool::new(false)))
                .clone();

            (tracker, lock)
        };

        let mut coordinator =
            HotSwapCoordinator::new(node_id.to_string(), new_wasm_path, old_tracker, swap_lock)
                .with_drain_timeout(drain_timeout);

        let mut ctx = {
            let mut factory_ctx_guard = self.factory_ctx.lock().await;
            factory_ctx_guard.take().ok_or_else(|| {
                WaferError::Runtime("factory context not available for hot-swap".into())
            })?
        };

        // PREPARE
        let new_node = match coordinator.prepare(&mut ctx).await {
            Ok(node) => node,
            Err(e) => {
                *self.factory_ctx.lock().await = Some(ctx);
                tracing::error!(node = %node_id, error = %e, "Hot-swap prepare failed");
                #[cfg(feature = "http-api")]
                self.control_state.metrics_registry.record_hotswap_failure();
                return Err(e);
            }
        };

        // DRAIN
        let drain_timed_out = match coordinator.drain().await {
            Ok(timed_out) => timed_out,
            Err(e) => {
                *self.factory_ctx.lock().await = Some(ctx);
                tracing::error!(node = %node_id, error = %e, "Hot-swap drain failed");
                #[cfg(feature = "http-api")]
                self.control_state.metrics_registry.record_hotswap_failure();
                return Err(e);
            }
        };

        if drain_timed_out {
            tracing::warn!(
                node = %node_id,
                "Drain timed out, proceeding with forced swap"
            );
        }

        // FLIP
        let old_node = coordinator.flip(&self.nodes, new_node).await?;

        // RETIRE
        coordinator.retire(old_node).await?;

        *self.factory_ctx.lock().await = Some(ctx);

        let metrics = coordinator.into_metrics();
        tracing::info!(
            node = %node_id,
            total_ms = %metrics.total_duration.as_millis(),
            prepare_ms = %metrics.prepare_duration.as_millis(),
            drain_ms = %metrics.drain_duration.as_millis(),
            flip_ms = %metrics.flip_duration.as_millis(),
            retire_ms = %metrics.retire_duration.as_millis(),
            drain_timed_out = %metrics.drain_timed_out,
            "Hot-swap complete"
        );

        #[cfg(feature = "http-api")]
        self.control_state.metrics_registry.record_hotswap_success(
            metrics.prepare_duration.as_nanos() as u64,
            metrics.drain_duration.as_nanos() as u64,
            metrics.flip_duration.as_nanos() as u64,
            metrics.retire_duration.as_nanos() as u64,
            metrics.messages_drained,
            metrics.drain_timed_out,
        );

        Ok(metrics)
    }

    /// Resync the pipeline by reloading config and hot-swapping changed nodes.
    ///
    /// Returns the list of node IDs that were hot-swapped.
    /// Errors if changes require restart (node add/remove, topology change).
    pub async fn resync(&self) -> Result<Vec<String>> {
        use crate::config::{diff_configs, load_dag_config};

        let config_path = self.config_path.as_ref().ok_or_else(|| {
            WaferError::Runtime(
                "Cannot resync: no config path stored. Use from_config_with_path() to enable reload."
                    .into(),
            )
        })?;

        tracing::info!(path = %config_path.display(), "Reloading configuration");

        let new_config = load_dag_config(config_path)?;
        let diff = diff_configs(&self.config, &new_config);

        if !diff.has_changes() {
            tracing::info!("No configuration changes detected");
            return Ok(Vec::new());
        }

        if diff.requires_restart() {
            let mut reasons = Vec::new();
            if !diff.nodes_added.is_empty() {
                reasons.push(format!("nodes added: {:?}", diff.nodes_added));
            }
            if !diff.nodes_removed.is_empty() {
                reasons.push(format!("nodes removed: {:?}", diff.nodes_removed));
            }
            if diff.edges_changed {
                reasons.push("edges changed".to_string());
            }
            return Err(WaferError::Runtime(format!(
                "Configuration changes require restart: {}",
                reasons.join(", ")
            )));
        }

        let mut swapped = Vec::new();
        for (node_id, new_path) in &diff.nodes_to_swap {
            tracing::info!(
                node = %node_id,
                new_path = %new_path.display(),
                "Hot-swapping node due to config change"
            );

            match self.hot_swap(node_id, new_path).await {
                Ok(metrics) => {
                    tracing::info!(
                        node = %node_id,
                        duration_ms = %metrics.total_duration.as_millis(),
                        "Node hot-swapped successfully"
                    );
                    swapped.push(node_id.clone());
                }
                Err(e) => {
                    tracing::error!(
                        node = %node_id,
                        error = %e,
                        "Failed to hot-swap node"
                    );
                    return Err(e);
                }
            }
        }

        tracing::info!(
            swapped_count = swapped.len(),
            nodes = ?swapped,
            "Configuration resync complete"
        );

        Ok(swapped)
    }

    #[must_use]
    pub fn config_path(&self) -> Option<&Path> {
        self.config_path.as_deref()
    }
}
