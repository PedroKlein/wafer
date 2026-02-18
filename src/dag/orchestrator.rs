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
//! # use wafer_poc::dag::DagOrchestrator;
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
//! # Module Organization
//!
//! The orchestrator implementation is split across multiple files:
//! - `orchestrator.rs` (this file): Core struct, run(), and public API
//! - `builder.rs`: Construction from config and validation
//! - `runner.rs`: Node execution loops (source, transform, sink)

use petgraph::graph::{DiGraph, NodeIndex};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::config::DagConfig;
use crate::error::Result;
use crate::node::AnyNode;
use crate::queue::{QueueReceiver, QueueSender, RuntimeEnvelope};

/// DAG orchestrator that manages graph topology, node execution, and inter-node communication.
///
/// The orchestrator handles:
/// - Graph topology validation (cycle detection, source/sink constraints)
/// - Node instance registration and lifecycle management
/// - Queue wiring for inter-node message passing
/// - Coordinated async execution of all nodes with proper shutdown
/// - Graceful shutdown via cancellation token
///
/// # Example
///
/// ```ignore
/// use wafer_poc::dag::DagOrchestrator;
/// use wafer_poc::config::DagConfig;
///
/// let config = DagConfig { /* ... */ };
/// let mut orchestrator = DagOrchestrator::from_config(config)?;
///
/// // Register node instances
/// orchestrator.register_node("source", source_node)?;
/// orchestrator.register_node("transform", transform_node)?;
/// orchestrator.register_node("sink", sink_node)?;
///
/// // Wire queues and run
/// orchestrator.wire_queues()?;
/// orchestrator.run().await?;
/// ```
pub struct DagOrchestrator {
    pub(super) graph: DiGraph<String, ()>,
    pub(super) node_indices: HashMap<String, NodeIndex>,
    pub(super) config: DagConfig,
    pub(super) topo_order: Vec<String>,
    /// Node instances keyed by node ID
    pub(super) nodes: HashMap<String, Arc<Mutex<AnyNode>>>,
    /// Queue senders for each edge (from_node, to_node)
    pub(super) queue_senders: HashMap<(String, String), QueueSender<RuntimeEnvelope>>,
    /// Queue receivers for each edge (from_node, to_node)
    pub(super) queue_receivers: HashMap<(String, String), QueueReceiver<RuntimeEnvelope>>,
    /// Cancellation token for graceful shutdown
    pub(super) cancel_token: CancellationToken,
}

impl fmt::Debug for DagOrchestrator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DagOrchestrator")
            .field("node_count", &self.node_indices.len())
            .field("edge_count", &self.graph.edge_count())
            .field("topo_order", &self.topo_order)
            .field("registered_nodes", &self.nodes.keys().collect::<Vec<_>>())
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
    /// # Errors
    ///
    /// Returns an error if:
    /// - Not all nodes are registered
    /// - Node initialization fails
    pub async fn run(&mut self) -> Result<()> {
        self.validate_nodes_registered()?;

        // Initialize nodes in topological order
        for node_id in &self.topo_order.clone() {
            if let Some(node) = self.nodes.get(node_id) {
                let mut locked = node.lock().await;
                if let Err(e) = locked.init().await {
                    tracing::error!(node = %node_id, error = %e, "Node init failed");
                    return Err(e);
                }
                tracing::debug!(node = %node_id, "Node initialized");
            }
        }

        let topo_order = self.topo_order.clone();
        let mut handles = Vec::new();

        for node_id in &topo_order {
            let node = self.nodes.get(node_id).cloned();
            let node_id_owned = node_id.clone();
            let cancel_token = self.cancel_token.clone();

            let output_senders: Vec<_> = self
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

            let input_keys_with_ports: Vec<_> = self
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
                    self.queue_receivers.remove(&key).map(|rx| (port, rx))
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
        self.queue_senders.clear();

        // Wait for all node tasks to complete
        for handle in handles {
            if let Err(e) = handle.await {
                tracing::error!(error = %e, "Node task panicked");
            }
        }

        // Close nodes in reverse topological order
        for node_id in self.topo_order.iter().rev() {
            if let Some(node) = self.nodes.get(node_id) {
                let mut locked = node.lock().await;
                if let Err(e) = locked.close().await {
                    tracing::warn!(node = %node_id, error = %e, "Node close failed");
                }
                tracing::debug!(node = %node_id, "Node closed");
            }
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
    /// use wafer_poc::dag::DagOrchestrator;
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
