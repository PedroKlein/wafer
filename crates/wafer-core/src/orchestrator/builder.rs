//! Builder for pipeline construction: Config → validate → wire → bundle.
//!
//! The new builder produces `BuildOutput` containing `NodeBundle`s ready for
//! spawning into independent tokio tasks. Queue wiring uses the receiver-keyed
//! algorithm from Session 5 D2.
//!
//! See docs/decisions/2025-07-12-orchestrator-runtime-simplification.md.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::config::{Config, DagConfig, EdgeDefinition, NodeType, OverflowPolicy};
use crate::dag::graph::DagGraph;
use crate::error::{ConfigError, Result, WaferError};
use crate::node::{NodeMetrics, NodeStateTracker};
use crate::queue::RuntimeEnvelope;
use crate::runner::error_policy::{DlqEnvelope, ErrorPolicyExecutor, ResolvedErrorPolicy};
use crate::runner::{DownstreamSender, SwapPayload};

// =============================================================================
// Public Types
// =============================================================================

/// Complete output from the builder — ready for orchestrator to spawn.
pub struct BuildOutput {
    /// Per-node bundles containing everything needed to run.
    pub node_bundles: Vec<NodeBundle>,
    /// Watch channel senders for hot-swap signaling (Wasm nodes only).
    pub watch_senders: HashMap<Box<str>, watch::Sender<Option<SwapPayload>>>,
    /// Shared cancellation token for graceful shutdown.
    pub cancel_token: CancellationToken,
    /// DLQ receiver — orchestrator spawns DLQ sink task with this.
    pub dlq_receiver: Option<mpsc::Receiver<DlqEnvelope>>,
    /// Per-node state trackers (atomic reads for status queries).
    pub state_trackers: HashMap<Box<str>, Arc<NodeStateTracker>>,
    /// Per-node metrics (atomic reads for Prometheus exposition).
    pub metrics_map: HashMap<Box<str>, Arc<NodeMetrics>>,
    /// Validated DAG graph (for topo order and structural queries).
    pub dag_graph: DagGraph,
}

/// Everything a single node task needs to run.
pub struct NodeBundle {
    /// Node identifier.
    pub node_id: Box<str>,
    /// What kind of node this is and its type-specific state.
    pub kind: NodeBundleKind,
    /// Child token for this node (derived from shared cancel).
    pub cancel: CancellationToken,
    /// Shared state tracker (also held by orchestrator for status reads).
    pub state: Arc<NodeStateTracker>,
    /// Shared metrics (also held by orchestrator for exposition).
    pub metrics: Arc<NodeMetrics>,
}

/// Type-specific bundle content.
///
/// Each variant carries the receiver, senders, and node-type-specific state
/// needed by its corresponding runner loop.
pub enum NodeBundleKind {
    Transform {
        receiver: mpsc::Receiver<RuntimeEnvelope>,
        senders: Vec<DownstreamSender>,
        swap_rx: watch::Receiver<Option<SwapPayload>>,
        policy: ErrorPolicyExecutor,
    },
    Filter {
        receiver: mpsc::Receiver<RuntimeEnvelope>,
        senders: Vec<DownstreamSender>,
        swap_rx: watch::Receiver<Option<SwapPayload>>,
        policy: ErrorPolicyExecutor,
    },
    Router {
        receiver: mpsc::Receiver<RuntimeEnvelope>,
        senders: Vec<DownstreamSender>,
        swap_rx: watch::Receiver<Option<SwapPayload>>,
        policy: ErrorPolicyExecutor,
    },
    Source {
        senders: Vec<DownstreamSender>,
    },
    Sink {
        receiver: mpsc::Receiver<RuntimeEnvelope>,
    },
}

/// An output edge sender with metadata for overflow handling.
#[derive(Debug, Clone)]
#[expect(dead_code, reason = "used when orchestrator spawns tasks from NodeBundles")]
pub struct EdgeSender {
    pub from_node: Box<str>,
    pub from_port: Box<str>,
    pub to_node: Box<str>,
    pub to_port: Box<str>,
    pub sender: mpsc::Sender<RuntimeEnvelope>,
    pub overflow: OverflowPolicy,
}

// =============================================================================
// Builder Implementation
// =============================================================================

/// Build pipeline infrastructure from a validated Config.
///
/// This function:
/// 1. Validates the DAG topology
/// 2. Wires channels (receiver-keyed: one channel per destination)
/// 3. Creates control infrastructure (cancel, watch, state, metrics)
/// 4. Creates DLQ channel
/// 5. Produces NodeBundles ready for spawning
///
/// Wasm compilation and instantiation are NOT done here — that's handled by the
/// orchestrator which uses WaferEngine to create node instances before spawning.
#[expect(dead_code, reason = "entry point for new orchestrator, not yet wired to main.rs")]
pub fn build_pipeline(config: &Config) -> Result<BuildOutput> {
    let dag_config = config.dag_config();
    let dag_graph = DagGraph::from_config(&dag_config)?;

    // --- Queue Wiring (Receiver-Keyed — Session 5 D2) ---
    let mut wiring = wire_queues(&config.edges, config.default_queue_capacity)?;

    // --- Control Infrastructure ---
    let cancel_token = CancellationToken::new();
    let mut watch_senders: HashMap<Box<str>, watch::Sender<Option<SwapPayload>>> = HashMap::new();
    let mut state_trackers: HashMap<Box<str>, Arc<NodeStateTracker>> = HashMap::new();
    let mut metrics_map: HashMap<Box<str>, Arc<NodeMetrics>> = HashMap::new();

    // --- DLQ Channel ---
    let dlq_capacity = config
        .dead_letter
        .as_ref()
        .map_or(1024, |dl| dl.queue_capacity);
    let (dlq_tx, dlq_rx) = mpsc::channel(dlq_capacity);

    // --- Build NodeBundles ---
    let mut node_bundles = Vec::with_capacity(config.nodes.len());

    for node_def in &config.nodes {
        let node_id: Box<str> = node_def.id.clone().into_boxed_str();
        let state = Arc::new(NodeStateTracker::new());
        let metrics = Arc::new(NodeMetrics::new());

        state_trackers.insert(node_id.clone(), Arc::clone(&state));
        metrics_map.insert(node_id.clone(), Arc::clone(&metrics));

        let node_cancel = cancel_token.child_token();

        let kind = match node_def.node_type {
            NodeType::Transform | NodeType::Filter | NodeType::Router => {
                // Wasm nodes get watch channels for hot-swap
                let (watch_tx, watch_rx) = watch::channel(None);
                watch_senders.insert(node_id.clone(), watch_tx);

                // Get receiver for this node (from wiring)
                let receiver = wiring.take_receiver(&node_def.id);

                // Get senders for this node's outputs
                let senders = wiring.collect_downstream_senders(&node_def.id);

                // Resolve error policy
                let policy_config = resolve_error_policy(config, &node_def.id);
                let policy = ErrorPolicyExecutor::new(
                    policy_config,
                    Some(dlq_tx.clone()),
                    node_id.clone(),
                );

                match node_def.node_type {
                    NodeType::Transform => NodeBundleKind::Transform {
                        receiver,
                        senders,
                        swap_rx: watch_rx,
                        policy,
                    },
                    NodeType::Filter => NodeBundleKind::Filter {
                        receiver,
                        senders,
                        swap_rx: watch_rx,
                        policy,
                    },
                    NodeType::Router => NodeBundleKind::Router {
                        receiver,
                        senders,
                        swap_rx: watch_rx,
                        policy,
                    },
                    _ => unreachable!(),
                }
            }
            NodeType::Source => {
                let senders = wiring.collect_downstream_senders(&node_def.id);
                NodeBundleKind::Source { senders }
            }
            NodeType::Sink => {
                let receiver = wiring.take_receiver(&node_def.id);
                NodeBundleKind::Sink { receiver }
            }
            NodeType::Joiner => {
                // Joiner removed in Session 3 A1 — merge is handled by multi-sender.
                // Treat as a transform for bundle purposes (has receiver + senders).
                let (watch_tx, watch_rx) = watch::channel(None);
                watch_senders.insert(node_id.clone(), watch_tx);
                let receiver = wiring.take_receiver(&node_def.id);
                let senders = wiring.collect_downstream_senders(&node_def.id);
                let policy_config = resolve_error_policy(config, &node_def.id);
                let policy = ErrorPolicyExecutor::new(
                    policy_config,
                    Some(dlq_tx.clone()),
                    node_id.clone(),
                );
                NodeBundleKind::Transform {
                    receiver,
                    senders,
                    swap_rx: watch_rx,
                    policy,
                }
            }
        };

        node_bundles.push(NodeBundle {
            node_id,
            kind,
            cancel: node_cancel,
            state,
            metrics,
        });
    }

    Ok(BuildOutput {
        node_bundles,
        watch_senders,
        cancel_token,
        dlq_receiver: Some(dlq_rx),
        state_trackers,
        metrics_map,
        dag_graph,
    })
}

// =============================================================================
// Queue Wiring (Receiver-Keyed)
// =============================================================================

/// Internal wiring state — groups edges by destination to create shared receivers.
struct QueueWiring {
    /// One receiver per unique (to_node, to_port) pair.
    receivers: HashMap<(String, String), mpsc::Receiver<RuntimeEnvelope>>,
    /// All edge senders, grouped by source node for collecting downstream outputs.
    edge_senders: Vec<EdgeSender>,
}

impl QueueWiring {
    /// Take the receiver for a given node (default port).
    ///
    /// Returns a dummy channel if no edges point to this node (e.g., source nodes).
    fn take_receiver(&mut self, node_id: &str) -> mpsc::Receiver<RuntimeEnvelope> {
        // Try "default" port first (most common case)
        let key = (node_id.to_string(), "default".to_string());
        if let Some(rx) = self.receivers.remove(&key) {
            return rx;
        }

        // Try any port for this node
        let matching_key = self
            .receivers
            .keys()
            .find(|(to_node, _)| to_node == node_id)
            .cloned();

        if let Some(k) = matching_key {
            return self.receivers.remove(&k).unwrap_or_else(|| {
                let (_tx, rx) = mpsc::channel(1);
                rx
            });
        }

        // No inbound edges — create a dummy receiver (for sources)
        let (_tx, rx) = mpsc::channel(1);
        rx
    }

    /// Collect all downstream senders from this node, converted to DownstreamSender.
    fn collect_downstream_senders(&self, node_id: &str) -> Vec<DownstreamSender> {
        self.edge_senders
            .iter()
            .filter(|e| &*e.from_node == node_id)
            .map(|e| DownstreamSender {
                sender: e.sender.clone(),
                port: e.from_port.clone(),
            })
            .collect()
    }
}

/// Wire queues using the receiver-keyed algorithm.
///
/// Groups edges by (to_node, to_port). Creates ONE mpsc channel per unique
/// destination. Multiple edges to the same destination (merge topology)
/// clone the sender — this is how tokio mpsc multi-producer works.
///
/// Capacity conflict on merge: uses the maximum (most permissive).
fn wire_queues(
    edges: &[EdgeDefinition],
    default_capacity: usize,
) -> Result<QueueWiring> {
    // Group edges by destination (to_node, to_port)
    let mut edges_by_dest: HashMap<(String, String), Vec<&EdgeDefinition>> = HashMap::new();
    for edge in edges {
        let to_port = edge.to_port.as_deref().unwrap_or("default").to_string();
        edges_by_dest
            .entry((edge.to.clone(), to_port))
            .or_default()
            .push(edge);
    }

    let mut receivers = HashMap::new();
    let mut edge_senders = Vec::new();

    // Create ONE channel per unique destination, clone sender per source edge
    for ((to_node, to_port), edges_to_dest) in &edges_by_dest {
        // Capacity: max of all edges pointing here (most permissive on merge)
        let capacity = edges_to_dest
            .iter()
            .filter_map(|e| e.queue_capacity)
            .max()
            .unwrap_or(default_capacity);

        let (sender, receiver) = mpsc::channel(capacity);
        receivers.insert((to_node.clone(), to_port.clone()), receiver);

        // One sender clone per source edge
        for edge in edges_to_dest {
            let from_port = edge.from_port.as_deref().unwrap_or("default");
            edge_senders.push(EdgeSender {
                from_node: edge.from.clone().into_boxed_str(),
                from_port: from_port.to_string().into_boxed_str(),
                to_node: to_node.clone().into_boxed_str(),
                to_port: to_port.clone().into_boxed_str(),
                sender: sender.clone(),
                overflow: edge.overflow,
            });
        }
        // Drop the original sender — only clones remain in edge_senders.
        // The receiver will see close when all EdgeSenders are dropped.
    }

    Ok(QueueWiring { receivers, edge_senders })
}

// =============================================================================
// Error Policy Resolution
// =============================================================================

/// Resolve error policy for a node: per-node overrides beat pipeline defaults.
///
/// Currently uses global defaults since per-node error_policy config is not
/// yet in the TOML schema. When added, this function merges them.
fn resolve_error_policy(_config: &Config, _node_id: &str) -> ResolvedErrorPolicy {
    // TODO: When config schema gets per-node error_policy fields,
    // merge pipeline defaults with per-node overrides here.
    ResolvedErrorPolicy::default()
}

