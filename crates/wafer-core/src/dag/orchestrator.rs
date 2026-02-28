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
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;
use wafer_types::{PipelineEvent, PipelineState};

use crate::config::DagConfig;
use crate::error::{Result, WaferError};
use crate::factory::FactoryContext;
use crate::node::AnyNode;
use crate::queue::{QueueReceiver, QueueSender, RuntimeEnvelope};

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

        // Update state to Running
        {
            let mut state = self.control_state.state.lock().await;
            *state = PipelineState::Running;
        }

        // Get a snapshot of nodes for iteration (we need to hold the lock briefly)
        let nodes_snapshot: HashMap<String, Arc<Mutex<AnyNode>>> = {
            let nodes = self.nodes.lock().await;
            nodes.clone()
        };

        // Initialize nodes in topological order
        for node_id in &self.topo_order {
            if let Some(node) = nodes_snapshot.get(node_id) {
                let mut locked = node.lock().await;
                if let Err(e) = locked.init().await {
                    tracing::error!(node = %node_id, error = %e, "Node init failed");
                    // Update state to Error
                    let mut state = self.control_state.state.lock().await;
                    *state = PipelineState::Error;
                    return Err(e);
                }
                tracing::debug!(node = %node_id, "Node initialized");
            }
        }

        let topo_order = self.topo_order.clone();
        let mut handles = Vec::new();

        for node_id in &topo_order {
            let node = nodes_snapshot.get(node_id).cloned();
            let node_id_owned = node_id.clone();
            let cancel_token = self.cancel_token.clone();

            let output_senders: Vec<_> = run_state
                .queue_senders
                .iter()
                .filter_map(|((from_key, _to_key), sender)| {
                    let (from_node, from_port) = Self::parse_node_port(from_key);
                    if from_node == node_id {
                        Some((from_port.to_string(), sender.clone()))
                    } else {
                        None
                    }
                })
                .collect();

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

            let input_receivers: Vec<(String, _)> = input_keys_with_ports
                .into_iter()
                .filter_map(|(key, port)| {
                    run_state.queue_receivers.remove(&key).map(|rx| (port, rx))
                })
                .collect();

            if let Some(node_arc) = node {
                let handle = tokio::spawn(async move {
                    Self::run_node_loop(
                        node_id_owned,
                        node_arc,
                        input_receivers,
                        output_senders,
                        cancel_token,
                    )
                    .await;
                });
                handles.push(handle);
            }
        }

        // Clear remaining senders so receivers will see channel close
        run_state.queue_senders.clear();
        // Drop run_state - we're done with it
        drop(run_state);

        // Wait for all node tasks to complete
        for handle in handles {
            if let Err(e) = handle.await {
                tracing::error!(error = %e, "Node task panicked");
            }
        }

        // Update state to Draining while closing nodes
        {
            let mut state = self.control_state.state.lock().await;
            *state = PipelineState::Draining;
        }

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

        // Update state to Stopped
        {
            let mut state = self.control_state.state.lock().await;
            *state = PipelineState::Stopped;
        }

        Ok(())
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
}
