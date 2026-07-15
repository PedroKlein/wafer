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

// =============================================================================
// Legacy Builder (kept for PipelineOrchestrator compatibility)
// =============================================================================

use super::pipeline::PipelineOrchestrator;

use std::path::{Path, PathBuf};
use tokio::sync::Mutex;

use super::assembler::{NodeAssembler, create_dlq_sink, create_node};
use crate::engine::WaferEngine;
use crate::node::AnyNode;
use crate::queue::BoundedQueue;
use super::pipeline::{ControlState, RunState};

/// Legacy builder methods on PipelineOrchestrator.
///
/// These will be removed in Phase 8 cleanup once the orchestrator is rewritten
/// to use `build_pipeline()` + `BuildOutput`.
impl PipelineOrchestrator {
    pub async fn from_config(config: Config, use_cache: bool) -> Result<Self> {
        Self::from_config_with_path(config, use_cache, None::<PathBuf>).await
    }

    pub async fn from_config_with_path(
        config: Config,
        use_cache: bool,
        config_path: Option<impl AsRef<Path>>,
    ) -> Result<Self> {
        let mut orchestrator = Self::from_config_inner(config)?;

        orchestrator.config_path = config_path.map(|p| p.as_ref().to_path_buf());

        let engine = Arc::new(WaferEngine::from_engine_config(&orchestrator.config.engine)?);
        engine.ensure_epoch_ticker();

        let mut registry_config = orchestrator.config.registry.clone();
        if !use_cache {
            registry_config.no_cache = true;
        }

        let mut assembler = NodeAssembler::new(Arc::clone(&engine), registry_config)?;

        for node_def in &orchestrator.config.nodes.clone() {
            let any_node = create_node(node_def, &mut assembler).await?;
            orchestrator.register_node(&node_def.id, any_node).await?;
        }

        orchestrator.wire_queues().await?;

        if let Some(ref dlq_config) = orchestrator.config.dead_letter {
            if dlq_config.enabled {
                orchestrator.dlq_config = Some(dlq_config.clone());
            }
        }

        orchestrator.factory_ctx = Some(Mutex::new(assembler));

        Ok(orchestrator)
    }

    pub(crate) async fn initialize_dlq(
        &self,
        dlq_config: &crate::config::DeadLetterConfig,
    ) -> Result<()> {
        let mut dlq_sink = create_dlq_sink(dlq_config)?;
        dlq_sink.init().await?;

        let queue: BoundedQueue<RuntimeEnvelope> = BoundedQueue::new(dlq_config.queue_capacity);
        let (sender, mut receiver) = queue.split();
        let cancel_token = self.cancel_token.clone();

        tokio::spawn(async move {
            tracing::info!("DLQ processing task started");
            loop {
                tokio::select! {
                    biased;
                    () = cancel_token.cancelled() => {
                        tracing::info!("DLQ processing task shutting down");
                        break;
                    }
                    Some(envelope) = receiver.recv() => {
                        if let Err(e) = dlq_sink.collect(envelope).await {
                            tracing::error!(error = %e, "DLQ sink failed to collect message");
                        }
                    }
                }
            }
            if let Err(e) = dlq_sink.close().await {
                tracing::warn!(error = %e, "DLQ sink close failed");
            }
        });

        self.control_state.set_dlq_sender(sender).await;
        tracing::info!(
            capacity = dlq_config.queue_capacity,
            sink_type = %dlq_config.sink_type,
            "Dead Letter Queue initialized"
        );
        Ok(())
    }

    /// Create from a DagConfig (legacy path — validates DAG only).
    #[must_use = "creating an orchestrator without using it is likely a bug"]
    pub fn from_dag_config(config: DagConfig) -> Result<Self> {
        use crate::config::{ApiServerConfig, EngineConfig, MetricsConfig};
        let full_config = Config {
            pipeline: config.pipeline,
            engine: EngineConfig::default(),
            api: ApiServerConfig::default(),
            metrics: MetricsConfig::default(),
            nodes: config.nodes,
            edges: config.edges,
            default_queue_capacity: config.default_queue_capacity,
            registry: Default::default(),
            dead_letter: None,
        };
        Self::from_config_inner(full_config)
    }

    /// Create the orchestrator skeleton from a full `Config`.
    fn from_config_inner(config: Config) -> Result<Self> {
        let dag_config = config.dag_config();
        let dag_graph = DagGraph::from_config(&dag_config)?;

        let pipeline_name = config.pipeline.name.clone();
        let orchestrator = Self {
            dag_graph,
            config,
            dlq_config: None,
            config_path: None,
            nodes: Mutex::new(HashMap::new()),
            run_state: Mutex::new(Some(RunState {
                queue_senders: HashMap::new(),
                queue_receivers: HashMap::new(),
            })),
            cancel_token: CancellationToken::new(),
            control_state: Arc::new(ControlState::new(pipeline_name)),
            factory_ctx: None,
            swap_locks: Mutex::new(HashMap::new()),
        };
        Ok(orchestrator)
    }

    pub async fn register_node(&self, id: &str, node: AnyNode) -> Result<()> {
        if !self.dag_graph.contains_node(id) {
            return Err(WaferError::Config(ConfigError::Message(format!("Unknown node ID: {id}"))));
        }
        #[cfg(feature = "http-api")]
        {
            self.control_state.metrics_registry.register_node(id, node.to_string());
        }
        let mut nodes = self.nodes.lock().await;
        nodes.insert(id.to_string(), Arc::new(Mutex::new(node)));
        Ok(())
    }

    /// Create bounded queues for each edge.
    pub async fn wire_queues(&self) -> Result<()> {
        let mut run_state_guard = self.run_state.lock().await;
        let run_state = run_state_guard.as_mut().ok_or_else(|| {
            WaferError::Runtime("Cannot wire queues: run state already consumed".into())
        })?;

        for edge in &self.config.edges {
            let capacity = edge.queue_capacity.unwrap_or(self.config.default_queue_capacity);
            let queue = BoundedQueue::new(capacity);
            let (sender, receiver) = queue.split();

            let from_port = edge.from_port.as_deref().unwrap_or("default");
            let to_port = edge.to_port.as_deref().unwrap_or("default");
            let key = (format!("{}:{}", edge.from, from_port), format!("{}:{}", edge.to, to_port));
            run_state.queue_senders.insert(key.clone(), sender);
            run_state.queue_receivers.insert(key, receiver);

            #[cfg(feature = "http-api")]
            {
                self.control_state.metrics_registry.register_queue(
                    &edge.from,
                    &edge.to,
                    capacity as u64,
                );
            }
        }
        Ok(())
    }

    pub(crate) async fn validate_nodes_registered(&self) -> Result<()> {
        let nodes = self.nodes.lock().await;
        let missing: Vec<_> =
            self.dag_graph.node_indices().keys().filter(|id| !nodes.contains_key(*id)).collect();

        if !missing.is_empty() {
            return Err(WaferError::Config(ConfigError::Message(format!(
                "Missing node registrations: {missing:?}"
            ))));
        }
        Ok(())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EdgeDefinition, NodeDefinition, NodeType, OverflowPolicy, PipelineConfig};

    fn make_node(id: &str, node_type: NodeType) -> NodeDefinition {
        NodeDefinition {
            id: id.to_string(),
            node_type,
            source_type: None,
            sink_type: None,
            config: toml::Value::Table(toml::map::Map::new()),
            capabilities: Default::default(),
        }
    }

    fn make_edge(from: &str, to: &str) -> EdgeDefinition {
        EdgeDefinition {
            from: from.to_string(),
            to: to.to_string(),
            from_port: None,
            to_port: None,
            queue_capacity: None,
            overflow: OverflowPolicy::default(),
        }
    }

    fn make_config(nodes: Vec<NodeDefinition>, edges: Vec<EdgeDefinition>) -> Config {
        Config {
            pipeline: PipelineConfig::default(),
            engine: crate::config::EngineConfig::default(),
            api: crate::config::ApiServerConfig::default(),
            metrics: crate::config::MetricsConfig::default(),
            nodes,
            edges,
            default_queue_capacity: 1024,
            registry: Default::default(),
            dead_letter: None,
        }
    }

    fn make_dag_config(nodes: Vec<NodeDefinition>, edges: Vec<EdgeDefinition>) -> DagConfig {
        DagConfig {
            pipeline: PipelineConfig::default(),
            nodes,
            edges,
            default_queue_capacity: 1024,
        }
    }

    // =========================================================================
    // DAG Validation Tests (kept from original — test via from_dag_config)
    // =========================================================================

    #[test]
    fn test_dag_valid_linear() {
        let config = make_dag_config(
            vec![
                make_node("source", NodeType::Source),
                make_node("transform", NodeType::Transform),
                make_node("sink", NodeType::Sink),
            ],
            vec![make_edge("source", "transform"), make_edge("transform", "sink")],
        );

        let orchestrator = PipelineOrchestrator::from_dag_config(config).unwrap();
        assert_eq!(orchestrator.topo_order(), &["source", "transform", "sink"]);
        assert_eq!(orchestrator.node_count(), 3);
        assert_eq!(orchestrator.edge_count(), 2);
    }

    #[test]
    fn test_dag_cycle_rejected() {
        let config = make_dag_config(
            vec![
                make_node("a", NodeType::Transform),
                make_node("b", NodeType::Transform),
                make_node("c", NodeType::Transform),
            ],
            vec![make_edge("a", "b"), make_edge("b", "c"), make_edge("c", "a")],
        );

        let result = PipelineOrchestrator::from_dag_config(config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Cycle"));
    }

    #[test]
    fn test_dag_no_source() {
        let config = make_dag_config(
            vec![make_node("transform", NodeType::Transform), make_node("sink", NodeType::Sink)],
            vec![make_edge("sink", "transform"), make_edge("transform", "sink")],
        );

        let result = PipelineOrchestrator::from_dag_config(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_dag_multiple_sources() {
        let config = make_dag_config(
            vec![
                make_node("source1", NodeType::Source),
                make_node("source2", NodeType::Source),
                make_node("sink", NodeType::Sink),
            ],
            vec![make_edge("source1", "sink"), make_edge("source2", "sink")],
        );

        let orchestrator = PipelineOrchestrator::from_dag_config(config).unwrap();
        assert_eq!(orchestrator.node_count(), 3);
        assert_eq!(orchestrator.edge_count(), 2);
    }

    #[test]
    fn test_dag_orphan_node() {
        let config = make_dag_config(
            vec![
                make_node("source", NodeType::Source),
                make_node("sink", NodeType::Sink),
                make_node("orphan", NodeType::Transform),
            ],
            vec![make_edge("source", "sink")],
        );

        let result = PipelineOrchestrator::from_dag_config(config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Orphan"));
    }

    #[test]
    fn test_dag_unknown_node_in_edge() {
        let config = make_dag_config(
            vec![make_node("source", NodeType::Source)],
            vec![make_edge("source", "nonexistent")],
        );

        let result = PipelineOrchestrator::from_dag_config(config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown"));
    }

    #[test]
    fn test_dag_single_node() {
        let config = make_dag_config(vec![make_node("single", NodeType::Source)], vec![]);

        let orchestrator = PipelineOrchestrator::from_dag_config(config).unwrap();
        assert_eq!(orchestrator.topo_order(), &["single"]);
    }

    #[test]
    fn test_dag_empty_rejected() {
        let config = make_dag_config(vec![], vec![]);

        let result = PipelineOrchestrator::from_dag_config(config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no nodes"));
    }

    #[test]
    fn test_dag_multiple_sinks() {
        let config = make_dag_config(
            vec![
                make_node("source", NodeType::Source),
                make_node("sink1", NodeType::Sink),
                make_node("sink2", NodeType::Sink),
            ],
            vec![make_edge("source", "sink1"), make_edge("source", "sink2")],
        );

        let orchestrator = PipelineOrchestrator::from_dag_config(config).unwrap();
        assert_eq!(orchestrator.node_count(), 3);
        assert_eq!(orchestrator.edge_count(), 2);
    }

    // =========================================================================
    // New Builder Tests — Queue Wiring
    // =========================================================================

    #[test]
    fn test_build_pipeline_linear() {
        let config = make_config(
            vec![
                make_node("source", NodeType::Source),
                make_node("transform", NodeType::Transform),
                make_node("sink", NodeType::Sink),
            ],
            vec![make_edge("source", "transform"), make_edge("transform", "sink")],
        );

        let output = build_pipeline(&config).unwrap();
        assert_eq!(output.node_bundles.len(), 3);
        assert_eq!(output.dag_graph.node_count(), 3);
        assert_eq!(output.dag_graph.edge_count(), 2);
    }

    #[test]
    fn test_build_pipeline_creates_watch_for_wasm_nodes() {
        let config = make_config(
            vec![
                make_node("source", NodeType::Source),
                make_node("t1", NodeType::Transform),
                make_node("f1", NodeType::Filter),
                make_node("r1", NodeType::Router),
                make_node("sink", NodeType::Sink),
            ],
            vec![
                make_edge("source", "t1"),
                make_edge("t1", "f1"),
                make_edge("f1", "r1"),
                make_edge("r1", "sink"),
            ],
        );

        let output = build_pipeline(&config).unwrap();
        // Watch channels for transform, filter, router (3 Wasm nodes)
        assert_eq!(output.watch_senders.len(), 3);
        assert!(output.watch_senders.contains_key("t1"));
        assert!(output.watch_senders.contains_key("f1"));
        assert!(output.watch_senders.contains_key("r1"));
        // Source and Sink don't get watch channels
        assert!(!output.watch_senders.contains_key("source"));
        assert!(!output.watch_senders.contains_key("sink"));
    }

    #[test]
    fn test_build_pipeline_state_and_metrics_for_all_nodes() {
        let config = make_config(
            vec![
                make_node("source", NodeType::Source),
                make_node("transform", NodeType::Transform),
                make_node("sink", NodeType::Sink),
            ],
            vec![make_edge("source", "transform"), make_edge("transform", "sink")],
        );

        let output = build_pipeline(&config).unwrap();
        assert_eq!(output.state_trackers.len(), 3);
        assert_eq!(output.metrics_map.len(), 3);
    }

    #[test]
    fn test_queue_wiring_merge_shared_receiver() {
        // Two sources pointing to same dest = merge (multi-sender, shared receiver)
        let config = make_config(
            vec![
                make_node("source1", NodeType::Source),
                make_node("source2", NodeType::Source),
                make_node("sink", NodeType::Sink),
            ],
            vec![make_edge("source1", "sink"), make_edge("source2", "sink")],
        );

        let output = build_pipeline(&config).unwrap();

        // Verify: sink bundle has a receiver, and both sources have senders to it
        let sink_bundle = output
            .node_bundles
            .iter()
            .find(|b| &*b.node_id == "sink")
            .unwrap();
        assert!(matches!(sink_bundle.kind, NodeBundleKind::Sink { .. }));

        let s1_bundle = output
            .node_bundles
            .iter()
            .find(|b| &*b.node_id == "source1")
            .unwrap();
        if let NodeBundleKind::Source { senders } = &s1_bundle.kind {
            assert_eq!(senders.len(), 1);
        } else {
            panic!("Expected Source kind");
        }

        let s2_bundle = output
            .node_bundles
            .iter()
            .find(|b| &*b.node_id == "source2")
            .unwrap();
        if let NodeBundleKind::Source { senders } = &s2_bundle.kind {
            assert_eq!(senders.len(), 1);
        } else {
            panic!("Expected Source kind");
        }
    }

    #[test]
    fn test_queue_wiring_fanout_separate_channels() {
        // One source with edges to two different destinations = fan-out
        let config = make_config(
            vec![
                make_node("source", NodeType::Source),
                make_node("sink1", NodeType::Sink),
                make_node("sink2", NodeType::Sink),
            ],
            vec![make_edge("source", "sink1"), make_edge("source", "sink2")],
        );

        let output = build_pipeline(&config).unwrap();

        // Source should have 2 downstream senders (to different destinations)
        let src_bundle = output
            .node_bundles
            .iter()
            .find(|b| &*b.node_id == "source")
            .unwrap();
        if let NodeBundleKind::Source { senders } = &src_bundle.kind {
            assert_eq!(senders.len(), 2);
        } else {
            panic!("Expected Source kind");
        }
    }

    #[test]
    fn test_queue_wiring_capacity_merge_uses_max() {
        // Two edges to same dest with different capacities — max wins
        let config = Config {
            pipeline: PipelineConfig::default(),
            engine: crate::config::EngineConfig::default(),
            api: crate::config::ApiServerConfig::default(),
            metrics: crate::config::MetricsConfig::default(),
            nodes: vec![
                make_node("s1", NodeType::Source),
                make_node("s2", NodeType::Source),
                make_node("sink", NodeType::Sink),
            ],
            edges: vec![
                EdgeDefinition {
                    from: "s1".to_string(),
                    to: "sink".to_string(),
                    from_port: None,
                    to_port: None,
                    queue_capacity: Some(64),
                    overflow: OverflowPolicy::default(),
                },
                EdgeDefinition {
                    from: "s2".to_string(),
                    to: "sink".to_string(),
                    from_port: None,
                    to_port: None,
                    queue_capacity: Some(256),
                    overflow: OverflowPolicy::default(),
                },
            ],
            default_queue_capacity: 1024,
            registry: Default::default(),
            dead_letter: None,
        };

        let wiring = wire_queues(&config.edges, config.default_queue_capacity).unwrap();
        // The receiver for (sink, default) should have capacity = max(64, 256) = 256
        // We can verify by checking the sender permits (channel capacity)
        let sender = wiring
            .edge_senders
            .iter()
            .find(|e| &*e.to_node == "sink")
            .unwrap();
        // mpsc::channel capacity is checked via max_capacity or we use indirect verification
        assert_eq!(sender.sender.max_capacity(), 256);
    }

    #[test]
    fn test_queue_wiring_with_ports() {
        // Router with port-based fan-out
        let config = make_config(
            vec![
                make_node("source", NodeType::Source),
                make_node("router", NodeType::Router),
                make_node("sink_a", NodeType::Sink),
                make_node("sink_b", NodeType::Sink),
            ],
            vec![
                make_edge("source", "router"),
                EdgeDefinition {
                    from: "router".to_string(),
                    to: "sink_a".to_string(),
                    from_port: Some("port_a".to_string()),
                    to_port: None,
                    queue_capacity: None,
                    overflow: OverflowPolicy::default(),
                },
                EdgeDefinition {
                    from: "router".to_string(),
                    to: "sink_b".to_string(),
                    from_port: Some("port_b".to_string()),
                    to_port: None,
                    queue_capacity: None,
                    overflow: OverflowPolicy::default(),
                },
            ],
        );

        let output = build_pipeline(&config).unwrap();

        // Router should have 2 downstream senders with different ports
        let router_bundle = output
            .node_bundles
            .iter()
            .find(|b| &*b.node_id == "router")
            .unwrap();
        if let NodeBundleKind::Router { senders, .. } = &router_bundle.kind {
            assert_eq!(senders.len(), 2);
            let ports: Vec<&str> = senders.iter().map(|s| &*s.port).collect();
            assert!(ports.contains(&"port_a"));
            assert!(ports.contains(&"port_b"));
        } else {
            panic!("Expected Router kind");
        }
    }

    #[test]
    fn test_build_pipeline_dlq_receiver_present() {
        let config = make_config(
            vec![make_node("source", NodeType::Source)],
            vec![],
        );

        let output = build_pipeline(&config).unwrap();
        assert!(output.dlq_receiver.is_some());
    }

    #[test]
    fn test_build_pipeline_cancel_token_shared() {
        let config = make_config(
            vec![
                make_node("source", NodeType::Source),
                make_node("sink", NodeType::Sink),
            ],
            vec![make_edge("source", "sink")],
        );

        let output = build_pipeline(&config).unwrap();

        // All bundles should have child tokens of the shared parent
        output.cancel_token.cancel();
        for bundle in &output.node_bundles {
            assert!(bundle.cancel.is_cancelled());
        }
    }

    // =========================================================================
    // Legacy Tests (feature-gated — reference old PipelineOrchestrator internals)
    // =========================================================================

    #[cfg(feature = "phase2-tests")]
    mod legacy {
        use super::*;
        use crate::node::FileSource;

        #[tokio::test]
        async fn test_wire_queues_creates_correct_count() {
            let config = make_dag_config(
                vec![
                    make_node("source", NodeType::Source),
                    make_node("transform", NodeType::Transform),
                    make_node("sink", NodeType::Sink),
                ],
                vec![make_edge("source", "transform"), make_edge("transform", "sink")],
            );

            let orchestrator = PipelineOrchestrator::from_dag_config(config).unwrap();
            orchestrator.wire_queues().await.unwrap();

            let run_state = orchestrator.run_state.lock().await;
            let run_state = run_state.as_ref().unwrap();
            assert_eq!(run_state.queue_senders.len(), 2);
            assert_eq!(run_state.queue_receivers.len(), 2);
        }

        #[tokio::test]
        async fn test_wire_queues_uses_custom_capacity() {
            let config = DagConfig {
                pipeline: PipelineConfig::default(),
                nodes: vec![make_node("source", NodeType::Source), make_node("sink", NodeType::Sink)],
                edges: vec![EdgeDefinition {
                    from: "source".to_string(),
                    to: "sink".to_string(),
                    from_port: None,
                    to_port: None,
                    queue_capacity: Some(42),
                    overflow: OverflowPolicy::default(),
                }],
                default_queue_capacity: 1024,
            };

            let orchestrator = PipelineOrchestrator::from_dag_config(config).unwrap();
            orchestrator.wire_queues().await.unwrap();

            let run_state = orchestrator.run_state.lock().await;
            let run_state = run_state.as_ref().unwrap();
            let receiver = run_state
                .queue_receivers
                .get(&("source:default".to_string(), "sink:default".to_string()))
                .unwrap();
            assert_eq!(receiver.capacity(), 42);
        }

        #[tokio::test]
        async fn test_register_node_unknown_id_fails() {
            let config = make_dag_config(vec![make_node("source", NodeType::Source)], vec![]);

            let orchestrator = PipelineOrchestrator::from_dag_config(config).unwrap();
            let node = AnyNode::from_source(FileSource::new("wrong-id", "/tmp/test.txt"));
            let result = orchestrator.register_node("unknown", node).await;

            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(err.contains("Unknown node ID"));
        }

        #[tokio::test]
        async fn test_dlq_initialization_with_file_sink() {
            use crate::config::DeadLetterConfig;

            let temp_dir = std::env::temp_dir().join("wafer_dlq_test");
            std::fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
            let dlq_path = temp_dir.join("dlq-output.jsonl");

            let mut dlq_config_table = toml::map::Map::new();
            dlq_config_table.insert(
                "path".to_string(),
                toml::Value::String(dlq_path.to_string_lossy().to_string()),
            );

            let dlq_config = DeadLetterConfig {
                enabled: true,
                sink_type: "file".to_string(),
                config: toml::Value::Table(dlq_config_table),
                queue_capacity: 100,
            };

            let config = DagConfig {
                pipeline: PipelineConfig::default(),
                nodes: vec![make_node("source", NodeType::Source)],
                edges: vec![],
                default_queue_capacity: 1024,
            };

            let orchestrator = PipelineOrchestrator::from_dag_config(config).unwrap();
            orchestrator.initialize_dlq(&dlq_config).await.expect("Failed to initialize DLQ");

            {
                let dlq_sender = orchestrator.control_state.dlq_sender.lock().await;
                assert!(dlq_sender.is_some());
            }
            let _ = std::fs::remove_dir_all(&temp_dir);
        }

        #[tokio::test]
        async fn test_dlq_initialization_with_stdout_sink() {
            use crate::config::DeadLetterConfig;

            let dlq_config = DeadLetterConfig {
                enabled: true,
                sink_type: "stdout".to_string(),
                config: toml::Value::Table(toml::map::Map::new()),
                queue_capacity: 50,
            };

            let config = DagConfig {
                pipeline: PipelineConfig::default(),
                nodes: vec![make_node("source", NodeType::Source)],
                edges: vec![],
                default_queue_capacity: 1024,
            };

            let orchestrator = PipelineOrchestrator::from_dag_config(config).unwrap();
            orchestrator.initialize_dlq(&dlq_config).await.expect("DLQ init failed");

            {
                let dlq_sender = orchestrator.control_state.dlq_sender.lock().await;
                assert!(dlq_sender.is_some());
            }
        }

        #[tokio::test]
        async fn test_dlq_initialization_invalid_sink_type() {
            use crate::config::DeadLetterConfig;

            let dlq_config = DeadLetterConfig {
                enabled: true,
                sink_type: "unknown_sink".to_string(),
                config: toml::Value::Table(toml::map::Map::new()),
                queue_capacity: 100,
            };

            let config = make_dag_config(vec![make_node("source", NodeType::Source)], vec![]);
            let orchestrator = PipelineOrchestrator::from_dag_config(config).unwrap();
            let result = orchestrator.initialize_dlq(&dlq_config).await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn test_dlq_file_sink_missing_path() {
            use crate::config::DeadLetterConfig;

            let dlq_config = DeadLetterConfig {
                enabled: true,
                sink_type: "file".to_string(),
                config: toml::Value::Table(toml::map::Map::new()),
                queue_capacity: 100,
            };

            let config = make_dag_config(vec![make_node("source", NodeType::Source)], vec![]);
            let orchestrator = PipelineOrchestrator::from_dag_config(config).unwrap();
            let result = orchestrator.initialize_dlq(&dlq_config).await;
            assert!(result.is_err());
        }
    }
}
