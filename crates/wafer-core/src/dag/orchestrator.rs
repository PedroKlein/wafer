// Duration nanosecond casts: 2^64 ns = ~585 years, truncation is acceptable
#![allow(clippy::cast_possible_truncation)]

//! DAG orchestrator for multi-node pipeline execution.
//!
//! Manages graph topology, node lifecycle, queue wiring, and coordinated
//! async execution of all nodes in the pipeline.
//!
//! # Graceful Shutdown
//!
//! The orchestrator supports graceful shutdown via `CancellationToken` from
//! `tokio_util`. All node loops (`run_source_loop`, `run_transform_loop`,
//! `run_sink_loop`) use `tokio::select!` to check for cancellation at each
//! iteration, allowing clean shutdown on SIGTERM/SIGINT.
//!
//! To trigger shutdown externally:
//! ```no_run
//! # use wafer_core::dag::DagOrchestrator;
//! # async fn example(orchestrator: &DagOrchestrator) {
//! // Option 1: Get token for external use
//! let token = orchestrator.cancel_token();
//! token.cancel();
//!
//! // Option 2: Use shutdown() method
//! orchestrator.shutdown();
//! # }
//! ```
//!
//! # Concurrent Access
//!
//! The orchestrator uses internal mutability to allow concurrent access from
//! both the execution loop and control operations (e.g., HTTP API handlers).
//! The `run()` method takes `&self` (not `&mut self`), enabling the orchestrator
//! to be wrapped in `Arc` for sharing across tasks.
//!
//! # Module Organization
//!
//! The orchestrator implementation is split across multiple files:
//! - `orchestrator.rs` (this file): Core struct, run(), and public API
//! - `builder.rs`: Construction from config and validation
//! - `runner.rs`: Node execution loops (source, transform, sink)
//! - `control.rs`: PipelineControl trait implementation

use petgraph::graph::{DiGraph, NodeIndex};
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

use super::hotswap::{HotSwapCoordinator, SwapError, SwapMetrics};

/// Information about an output edge for sending messages.
///
/// Bundles the sender, overflow policy, and edge name for use in node loops
/// when handling queue overflow scenarios.
#[derive(Clone)]
pub struct EdgeSendInfo {
    /// Output port name (e.g., "default", "high", "low")
    pub port: String,
    /// Queue sender for this edge
    pub sender: QueueSender<RuntimeEnvelope>,
    /// Overflow policy for this edge
    pub overflow_policy: OverflowPolicy,
    /// Edge identifier for DLQ metadata (format: "from_node:port->to_node:port")
    pub edge_name: String,
}

/// State consumed during `run()` - can only be used once.
///
/// This struct holds the queue senders and receivers that are moved into
/// spawned tasks during pipeline execution. Once `run()` is called, this
/// state is consumed and cannot be reused.
pub struct RunState {
    /// Queue senders for each edge (from_node:port, to_node:port)
    pub queue_senders: HashMap<(String, String), QueueSender<RuntimeEnvelope>>,
    /// Queue receivers for each edge (from_node:port, to_node:port)
    pub queue_receivers: HashMap<(String, String), QueueReceiver<RuntimeEnvelope>>,
}

/// Shared state for control operations and metrics.
///
/// This state is accessible throughout the orchestrator's lifetime and
/// supports the `PipelineControl` trait implementation.
pub struct ControlState {
    /// Pipeline name from config
    pub name: String,
    /// When the pipeline was created
    pub created_at: Instant,
    /// Current pipeline state
    pub state: Mutex<PipelineState>,
    /// Total messages processed across all nodes
    pub messages_processed: AtomicU64,
    /// Total messages that failed processing
    pub messages_failed: AtomicU64,
    /// Event broadcaster for subscribers
    pub event_tx: broadcast::Sender<PipelineEvent>,
    /// Dead Letter Queue sender (if DLQ is enabled).
    /// Messages wrapped as RuntimeEnvelope with JSON-serialized DlqEnvelope payload.
    /// Uses Mutex to allow late initialization after orchestrator construction.
    pub dlq_sender: Mutex<Option<QueueSender<RuntimeEnvelope>>>,
    /// Prometheus metrics registry (only with http-api feature)
    #[cfg(feature = "http-api")]
    pub metrics_registry: MetricsRegistry,
}

impl ControlState {
    /// Create new control state with the given pipeline name.
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
    #[allow(dead_code)] // API for pipeline configuration with labels
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

    /// Get uptime in seconds since creation.
    pub fn uptime_secs(&self) -> u64 {
        self.created_at.elapsed().as_secs()
    }
}

/// DAG orchestrator that manages graph topology, node execution, and inter-node communication.
///
/// The orchestrator handles:
/// - Graph topology validation (cycle detection, source/sink constraints)
/// - Node instance registration and lifecycle management
/// - Queue wiring for inter-node message passing
/// - Coordinated async execution of all nodes with proper shutdown
/// - Graceful shutdown via cancellation token
/// - Control operations via `PipelineControl` trait
///
/// # Concurrent Access
///
/// The orchestrator uses internal mutability to support concurrent access.
/// The `run()` method takes `&self`, allowing the orchestrator to be shared
/// via `Arc` between the execution task and API handlers.
///
/// # Example
///
/// ```ignore
/// use wafer_core::dag::DagOrchestrator;
/// use wafer_core::config::DagConfig;
/// use std::sync::Arc;
///
/// let config = DagConfig { /* ... */ };
/// let orchestrator = Arc::new(DagOrchestrator::from_config(config)?);
///
/// // Share with API server
/// let api_orchestrator = Arc::clone(&orchestrator);
/// tokio::spawn(async move {
///     api_server.run(api_orchestrator).await;
/// });
///
/// // Run pipeline (takes &self, not &mut self)
/// orchestrator.run().await?;
/// ```
pub struct DagOrchestrator {
    // Immutable topology state
    pub(super) graph: DiGraph<String, ()>,
    pub(super) node_indices: HashMap<String, NodeIndex>,
    pub(super) config: DagConfig,
    pub(super) topo_order: Vec<String>,

    /// Path to the configuration file (for reload_config).
    /// `None` if constructed programmatically without a file.
    pub(super) config_path: Option<PathBuf>,

    /// Node instances keyed by node ID.
    /// Protected by Mutex for interior mutability during setup.
    pub(super) nodes: Mutex<HashMap<String, Arc<Mutex<AnyNode>>>>,

    /// Run state - consumed once during run()
    /// `None` after `run()` has been called.
    pub(super) run_state: Mutex<Option<RunState>>,

    /// Cancellation token for graceful shutdown
    pub(super) cancel_token: CancellationToken,

    /// Control state for PipelineControl operations
    pub(super) control_state: Arc<ControlState>,

    /// Factory context for cleanup (epoch tickers, etc.)
    /// Uses Mutex for interior mutability since we set it after construction.
    pub(super) factory_ctx: Mutex<Option<FactoryContext>>,

    /// Per-node swap locks to prevent concurrent swaps on the same node.
    /// Key is node ID, value is true if swap is in progress.
    pub(super) swap_locks: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl fmt::Debug for DagOrchestrator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Note: We can't access nodes via Mutex in sync Debug, so we just show count from config
        f.debug_struct("DagOrchestrator")
            .field("name", &self.control_state.name)
            .field("node_count", &self.node_indices.len())
            .field("edge_count", &self.graph.edge_count())
            .field("topo_order", &self.topo_order)
            .finish_non_exhaustive()
    }
}

impl DagOrchestrator {
    fn parse_node_port(key: &str) -> (&str, &str) {
        key.split_once(':').unwrap_or((key, "default"))
    }

    /// Find the overflow policy for an edge given its from and to keys.
    ///
    /// Keys are in the format "node_id:port" or just "node_id" (default port).
    /// Looks up the edge in the config to get the overflow policy.
    /// Returns `OverflowPolicy::Slow` (default) if the edge is not found.
    fn find_edge_overflow_policy(&self, from_key: &str, to_key: &str) -> OverflowPolicy {
        let (from_node, from_port) = Self::parse_node_port(from_key);
        let (to_node, to_port) = Self::parse_node_port(to_key);

        // Find matching edge in config
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

        // Default if not found (shouldn't happen in a valid config)
        OverflowPolicy::Slow
    }

    /// Run the DAG pipeline.
    ///
    /// This method:
    /// 1. Initializes all nodes in topological order
    /// 2. Spawns async tasks for each node
    /// 3. Waits for all tasks to complete
    /// 4. Closes all nodes in reverse topological order
    ///
    /// # Concurrent Access
    ///
    /// This method takes `&self` (not `&mut self`), allowing the orchestrator
    /// to be shared via `Arc` with API handlers while the pipeline is running.
    ///
    /// # Single Execution
    ///
    /// This method can only be called once. Subsequent calls will return an error.
    /// The run state (queue senders/receivers) is consumed during the first call.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Not all nodes are registered
    /// - Node initialization fails
    /// - `run()` has already been called (state consumed)
    pub async fn run(&self) -> Result<()> {
        self.validate_nodes_registered().await?;

        // Take the run state - this can only succeed once
        let mut run_state = {
            let mut guard = self.run_state.lock().await;
            guard.take().ok_or_else(|| {
                WaferError::Runtime("Pipeline has already been run (run state consumed)".into())
            })?
        };

        self.set_pipeline_state(PipelineState::Running).await;

        // Get a snapshot of nodes for iteration (we need to hold the lock briefly)
        let nodes_snapshot: HashMap<String, Arc<Mutex<AnyNode>>> = {
            let nodes = self.nodes.lock().await;
            nodes.clone()
        };

        // Initialize nodes in topological order
        self.init_nodes_in_order(&nodes_snapshot).await?;

        // Spawn node tasks and get handles
        let handles = self.spawn_node_tasks(&nodes_snapshot, &mut run_state);

        // Clear remaining senders so receivers will see channel close
        run_state.queue_senders.clear();
        drop(run_state);

        // Wait for all node tasks to complete
        self.wait_for_tasks(handles).await;

        // Shutdown: close nodes and update state
        self.shutdown_nodes(&nodes_snapshot).await;

        Ok(())
    }

    /// Set the pipeline state.
    async fn set_pipeline_state(&self, new_state: PipelineState) {
        let mut state = self.control_state.state.lock().await;
        *state = new_state;
    }

    /// Initialize nodes in topological order.
    async fn init_nodes_in_order(
        &self,
        nodes_snapshot: &HashMap<String, Arc<Mutex<AnyNode>>>,
    ) -> Result<()> {
        for node_id in &self.topo_order {
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

    /// Spawn node execution tasks and return their handles.
    fn spawn_node_tasks(
        &self,
        nodes_snapshot: &HashMap<String, Arc<Mutex<AnyNode>>>,
        run_state: &mut RunState,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        let mut handles = Vec::new();

        for node_id in &self.topo_order {
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

    /// Collect output senders for a node from the run state.
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

    /// Collect input receivers for a node from the run state.
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

    /// Wait for all node tasks to complete.
    async fn wait_for_tasks(&self, handles: Vec<tokio::task::JoinHandle<()>>) {
        for handle in handles {
            if let Err(e) = handle.await {
                tracing::error!(error = %e, "Node task panicked");
            }
        }
    }

    /// Shutdown nodes: close in reverse order and update state.
    async fn shutdown_nodes(&self, nodes_snapshot: &HashMap<String, Arc<Mutex<AnyNode>>>) {
        self.set_pipeline_state(PipelineState::Draining).await;

        // Close nodes in reverse topological order
        for node_id in self.topo_order.iter().rev() {
            if let Some(node) = nodes_snapshot.get(node_id) {
                let mut locked = node.lock().await;
                if let Err(e) = locked.close().await {
                    tracing::warn!(node = %node_id, error = %e, "Node close failed");
                }
                tracing::debug!(node = %node_id, "Node closed");
            }
        }

        // Clean up epoch tickers from factory context
        if let Some(ref ctx) = *self.factory_ctx.lock().await {
            ctx.abort_tickers();
        }

        self.set_pipeline_state(PipelineState::Stopped).await;
    }

    /// Get the topological order of nodes.
    #[must_use]
    pub fn topo_order(&self) -> &[String] {
        &self.topo_order
    }

    /// Get the DAG configuration.
    #[must_use]
    pub fn config(&self) -> &DagConfig {
        &self.config
    }

    /// Get the number of nodes in the DAG.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.node_indices.len()
    }

    /// Get the number of edges in the DAG.
    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    /// Get a clone of the cancellation token for external shutdown triggering.
    ///
    /// This token can be used to trigger graceful shutdown from signal handlers
    /// or other external sources.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use wafer_core::dag::DagOrchestrator;
    ///
    /// # async fn example(mut orchestrator: DagOrchestrator) {
    /// let cancel_token = orchestrator.cancel_token();
    ///
    /// // In a signal handler or another task:
    /// tokio::spawn(async move {
    ///     tokio::signal::ctrl_c().await.ok();
    ///     cancel_token.cancel();
    /// });
    ///
    /// // Run the pipeline - will stop when cancelled
    /// orchestrator.run().await.ok();
    /// # }
    /// ```
    #[must_use]
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel_token.clone()
    }

    /// Request graceful shutdown of all running nodes.
    ///
    /// This will signal all node loops to stop processing at their next
    /// cancellation check point. Nodes will complete their current operation
    /// before stopping.
    pub fn shutdown(&self) {
        tracing::info!("Shutdown requested");
        self.cancel_token.cancel();
    }

    /// Hot-swap a WASM node with a new component.
    ///
    /// This performs a live replacement of a WASM node without stopping the
    /// pipeline, using the drain-and-flip algorithm:
    ///
    /// 1. **PREPARE**: Load and validate the new WASM component
    /// 2. **DRAIN**: Disable routing, wait for in-flight messages
    /// 3. **FLIP**: Atomically swap the node reference
    /// 4. **RETIRE**: Close the old node
    ///
    /// # Arguments
    ///
    /// * `node_id` - ID of the node to swap
    /// * `new_wasm_path` - Path to the new WASM component
    ///
    /// # Returns
    ///
    /// Returns [`SwapMetrics`] with timing and status information.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Node not found
    /// - Node type does not support hot-swap (Source, Sink)
    /// - Another swap is already in progress for this node
    /// - New component fails to load or validate
    ///
    /// # Example
    ///
    /// ```ignore
    /// let metrics = orchestrator.hot_swap("my-transform", "/path/to/new.wasm").await?;
    /// println!("Swap completed in {:?}", metrics.total_duration);
    /// ```
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

    /// Hot-swap a WASM node with a custom drain timeout.
    ///
    /// See [`hot_swap`](Self::hot_swap) for details.
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

        // Validate node exists and is swappable
        let (old_tracker, swap_lock) = {
            let nodes = self.nodes.lock().await;
            let node_arc = nodes
                .get(node_id)
                .ok_or_else(|| WaferError::from(SwapError::NodeNotFound(node_id.to_string())))?;

            let node = node_arc.lock().await;
            if !node.is_swappable() {
                return Err(WaferError::from(SwapError::NotSwappable(
                    node_id.to_string(),
                )));
            }

            let tracker = node.state_tracker_clone();
            drop(node);

            // Get or create the swap lock for this node
            let mut swap_locks = self.swap_locks.lock().await;
            let lock = swap_locks
                .entry(node_id.to_string())
                .or_insert_with(|| Arc::new(AtomicBool::new(false)))
                .clone();

            (tracker, lock)
        };

        // Create the coordinator
        let mut coordinator =
            HotSwapCoordinator::new(node_id.to_string(), new_wasm_path, old_tracker, swap_lock)
                .with_drain_timeout(drain_timeout);

        // Get factory context for creating new node
        let mut ctx = {
            let mut factory_ctx_guard = self.factory_ctx.lock().await;
            factory_ctx_guard.take().ok_or_else(|| {
                WaferError::Runtime("factory context not available for hot-swap".into())
            })?
        };

        // Phase 1: PREPARE
        let new_node = match coordinator.prepare(&mut ctx).await {
            Ok(node) => node,
            Err(e) => {
                // Put factory context back
                *self.factory_ctx.lock().await = Some(ctx);
                tracing::error!(node = %node_id, error = %e, "Hot-swap prepare failed");
                #[cfg(feature = "http-api")]
                self.control_state.metrics_registry.record_hotswap_failure();
                return Err(e);
            }
        };

        // Phase 2: DRAIN
        let drain_timed_out = match coordinator.drain().await {
            Ok(timed_out) => timed_out,
            Err(e) => {
                // Put factory context back and abort
                *self.factory_ctx.lock().await = Some(ctx);
                tracing::error!(node = %node_id, error = %e, "Hot-swap drain failed");
                #[cfg(feature = "http-api")]
                self.control_state.metrics_registry.record_hotswap_failure();
                // Coordinator drop will release the lock
                return Err(e);
            }
        };

        if drain_timed_out {
            tracing::warn!(
                node = %node_id,
                "Drain timed out, proceeding with forced swap"
            );
        }

        // Phase 3: FLIP
        let old_node = coordinator.flip(&self.nodes, new_node).await?;

        // Phase 4: RETIRE
        coordinator.retire(old_node).await?;

        // Put factory context back
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

        // Record hot-swap metrics
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

    /// Resync the pipeline with a reloaded configuration file.
    ///
    /// This method:
    /// 1. Reloads the configuration from the stored config path
    /// 2. Diffs the new config against the current config
    /// 3. Hot-swaps any nodes with changed WASM paths
    ///
    /// # Returns
    ///
    /// A list of node IDs that were hot-swapped.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - No config path is stored (orchestrator created programmatically)
    /// - Config file cannot be read or parsed
    /// - Config changes require restart (node add/remove, topology change)
    /// - Any hot-swap operation fails
    pub async fn resync(&self) -> Result<Vec<String>> {
        use crate::config::{diff_configs, load_dag_config};

        let config_path = self.config_path.as_ref().ok_or_else(|| {
            WaferError::Runtime(
                "Cannot resync: no config path stored. Use from_config_with_path() to enable reload."
                    .into(),
            )
        })?;

        tracing::info!(path = %config_path.display(), "Reloading configuration");

        // Load the new config
        let new_config = load_dag_config(config_path)?;

        // Diff against current config
        let diff = diff_configs(&self.config, &new_config);

        if !diff.has_changes() {
            tracing::info!("No configuration changes detected");
            return Ok(Vec::new());
        }

        // Check for changes that require restart
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

        // Hot-swap changed nodes
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

    /// Get the config file path if one was provided.
    #[must_use]
    pub fn config_path(&self) -> Option<&Path> {
        self.config_path.as_deref()
    }
}
