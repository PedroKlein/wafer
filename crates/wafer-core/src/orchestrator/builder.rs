//! Builder for pipeline construction: Config → validate → wire → bundle.
//!
//! The builder produces `BuildOutput` containing `NodeBundle`s ready for
//! spawning into independent tokio tasks. Queue wiring uses one receiver per
//! destination node and clones senders for fan-in.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::config::{Config, EdgeDef, NodeCategory, NodeDef, OverflowPolicy};
use crate::dag::graph::DagGraph;
use crate::error::Result;
use crate::node::wasm::WasmRouterNode;
use crate::node::{FilterNode, TransformNode};
use crate::node::{NodeMetrics, NodeStateTracker, Sink, Source};
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
        /// Compiled transform node (Wasm or native). None in unit tests without .wasm.
        node: Option<TransformNode>,
    },
    Filter {
        receiver: mpsc::Receiver<RuntimeEnvelope>,
        senders: Vec<DownstreamSender>,
        swap_rx: watch::Receiver<Option<SwapPayload>>,
        policy: ErrorPolicyExecutor,
        /// Compiled filter node (Wasm or native). None in unit tests without .wasm.
        node: Option<FilterNode>,
    },
    Router {
        receiver: mpsc::Receiver<RuntimeEnvelope>,
        senders: Vec<DownstreamSender>,
        swap_rx: watch::Receiver<Option<SwapPayload>>,
        policy: ErrorPolicyExecutor,
        /// Compiled Wasm node instance (None in unit tests without .wasm).
        node: Option<WasmRouterNode>,
    },
    Source {
        /// Constructed source instance (None when builder is used without I/O construction).
        source: Option<Box<dyn Source + Send>>,
        senders: Vec<DownstreamSender>,
    },
    Sink {
        /// Constructed sink instance (None when builder is used without I/O construction).
        sink: Option<Box<dyn Sink + Send>>,
        receiver: mpsc::Receiver<RuntimeEnvelope>,
    },
}

/// An output edge sender with metadata for overflow handling.
#[derive(Debug, Clone)]
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
pub fn build_pipeline(config: &Config) -> Result<BuildOutput> {
    build_pipeline_inner(config, HashMap::new(), HashMap::new())
}

/// Build pipeline with externally-provided source/sink instances.
///
/// Used by integration tests to inject ChannelSource/ChannelSink without
/// needing real I/O. The topology wiring (channels, control infrastructure)
/// is identical to `build_pipeline()`, but source/sink bundles carry the
/// provided instances instead of `None`.
///
/// # Errors
///
/// Returns error if DAG validation fails or wiring encounters issues.
pub fn build_pipeline_with_io(
    config: &Config,
    sources: HashMap<String, Box<dyn Source + Send>>,
    sinks: HashMap<String, Box<dyn Sink + Send>>,
) -> Result<BuildOutput> {
    build_pipeline_inner(config, sources, sinks)
}

fn build_pipeline_inner(
    config: &Config,
    mut sources: HashMap<String, Box<dyn Source + Send>>,
    mut sinks: HashMap<String, Box<dyn Sink + Send>>,
) -> Result<BuildOutput> {
    let dag_graph = DagGraph::from_config(config)?;

    let mut wiring = wire_queues(&config.edges, config.engine.default_queue_capacity)?;

    let cancel_token = CancellationToken::new();
    let mut watch_senders: HashMap<Box<str>, watch::Sender<Option<SwapPayload>>> = HashMap::new();
    let mut state_trackers: HashMap<Box<str>, Arc<NodeStateTracker>> = HashMap::new();
    let mut metrics_map: HashMap<Box<str>, Arc<NodeMetrics>> = HashMap::new();

    let dlq_capacity = config.dead_letter.as_ref().map_or(1024, dead_letter_capacity);
    let (dlq_tx, dlq_rx) = mpsc::channel(dlq_capacity);

    let mut node_bundles = Vec::with_capacity(config.nodes.len());

    for (node_id, node_def) in &config.nodes {
        let node_id_box: Box<str> = node_id.clone().into_boxed_str();
        let state = Arc::new(NodeStateTracker::new());
        let metrics = Arc::new(NodeMetrics::new());

        state_trackers.insert(node_id_box.clone(), Arc::clone(&state));
        metrics_map.insert(node_id_box.clone(), Arc::clone(&metrics));

        let node_cancel = cancel_token.child_token();

        let kind = match node_def.category() {
            NodeCategory::Transform | NodeCategory::Filter | NodeCategory::Router => {
                let (watch_tx, watch_rx) = watch::channel(None);
                watch_senders.insert(node_id_box.clone(), watch_tx);

                let receiver = wiring.take_receiver(node_id);
                let senders = wiring.collect_downstream_senders(node_id);

                let policy_config = resolve_error_policy(config, node_id);
                let policy = ErrorPolicyExecutor::new(
                    policy_config,
                    Some(dlq_tx.clone()),
                    node_id_box.clone(),
                );

                match node_def.category() {
                    NodeCategory::Transform => NodeBundleKind::Transform {
                        receiver,
                        senders,
                        swap_rx: watch_rx,
                        policy,
                        node: None,
                    },
                    NodeCategory::Filter => NodeBundleKind::Filter {
                        receiver,
                        senders,
                        swap_rx: watch_rx,
                        policy,
                        node: None,
                    },
                    NodeCategory::Router => NodeBundleKind::Router {
                        receiver,
                        senders,
                        swap_rx: watch_rx,
                        policy,
                        node: None,
                    },
                    #[expect(clippy::unreachable, reason = "Source/Sink categories are handled in the outer match; inner match only sees Transform/Filter/Router")]
                    _ => unreachable!("Source/Sink should not reach wasm node builder"),
                }
            }
            NodeCategory::Source => {
                let senders = wiring.collect_downstream_senders(node_id);
                let source = sources.remove(node_id);
                NodeBundleKind::Source { source, senders }
            }
            NodeCategory::Sink => {
                let receiver = wiring.take_receiver(node_id);
                let sink = sinks.remove(node_id);
                NodeBundleKind::Sink { sink, receiver }
            }
        };

        node_bundles.push(NodeBundle {
            node_id: node_id_box,
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

const fn dead_letter_capacity(config: &crate::config::DeadLetterConfig) -> usize {
    match config {
        crate::config::DeadLetterConfig::Mqtt { queue_capacity, .. }
        | crate::config::DeadLetterConfig::File { queue_capacity, .. } => *queue_capacity,
    }
}

// =============================================================================
// Queue Wiring (Receiver-Keyed)
// =============================================================================

/// Internal wiring state — groups edges by destination to create shared receivers.
struct QueueWiring {
    /// One receiver per destination node.
    receivers: HashMap<String, mpsc::Receiver<RuntimeEnvelope>>,
    /// All edge senders, grouped by source node for collecting downstream outputs.
    edge_senders: Vec<EdgeSender>,
}

impl QueueWiring {
    /// Take the receiver for a given node.
    fn take_receiver(&mut self, node_id: &str) -> mpsc::Receiver<RuntimeEnvelope> {
        self.receivers.remove(node_id).unwrap_or_else(|| {
            let (_tx, rx) = mpsc::channel(1);
            rx
        })
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
/// Groups edges by destination node. Creates ONE mpsc channel per destination.
/// Multiple edges to the same destination clone the sender — this is how tokio
/// mpsc multi-producer fan-in works. Capacity conflicts on merge use the max.
fn wire_queues(edges: &[EdgeDef], default_capacity: usize) -> Result<QueueWiring> {
    let mut edges_by_dest: HashMap<String, Vec<&EdgeDef>> = HashMap::new();
    for edge in edges {
        edges_by_dest.entry(edge.to.clone()).or_default().push(edge);
    }

    let mut receivers = HashMap::new();
    let mut edge_senders = Vec::new();

    for (to_node, edges_to_dest) in &edges_by_dest {
        let capacity = edges_to_dest
            .iter()
            .filter_map(|e| e.capacity)
            .max()
            .unwrap_or(default_capacity);

        let (sender, receiver) = mpsc::channel(capacity);
        receivers.insert(to_node.clone(), receiver);

        for edge in edges_to_dest {
            let from_port = edge.port.as_deref().unwrap_or("default");
            edge_senders.push(EdgeSender {
                from_node: edge.from.clone().into_boxed_str(),
                from_port: from_port.to_string().into_boxed_str(),
                to_node: to_node.clone().into_boxed_str(),
                to_port: "default".into(),
                sender: sender.clone(),
                overflow: edge.overflow.unwrap_or_default(),
            });
        }
    }

    Ok(QueueWiring { receivers, edge_senders })
}

// =============================================================================
// Error Policy Resolution
// =============================================================================

/// Resolve error policy for a node: per-node overrides beat pipeline defaults.
fn resolve_error_policy(config: &Config, node_id: &str) -> ResolvedErrorPolicy {
    let base = config.error_policy.clone();
    let override_policy = match config.nodes.get(node_id) {
        Some(NodeDef::Transform(wasm) | NodeDef::Filter(wasm) | NodeDef::Router(wasm)) => {
            wasm.error_policy.clone()
        }
        _ => None,
    };

    override_policy.unwrap_or(base).into()
}
