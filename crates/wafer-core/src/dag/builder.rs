//! Builder and validation methods for DAG orchestrator.
//!
//! This module contains the construction and validation logic for [`DagOrchestrator`].

use petgraph::algo::toposort;
use petgraph::graph::DiGraph;
use petgraph::Direction;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::config::{Config, DagConfig};
use crate::error::{ConfigError, Result, WaferError};
use crate::factory::{create_node, FactoryContext};
use crate::node::AnyNode;
use crate::queue::BoundedQueue;

use super::orchestrator::{ControlState, RunState};
use super::DagOrchestrator;

impl DagOrchestrator {
    /// Build a fully-configured DAG orchestrator from a Config.
    ///
    /// This is the high-level constructor that:
    /// 1. Validates the topology
    /// 2. Creates all nodes using the factory
    /// 3. Wires queues between nodes
    ///
    /// # Arguments
    ///
    /// * `config` - Full pipeline configuration
    /// * `use_cache` - Whether to use OCI registry cache for remote plugins
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Topology validation fails
    /// - Node creation fails
    /// - Queue wiring fails
    pub async fn from_config(config: Config, use_cache: bool) -> Result<Self> {
        let dag_config = config.to_dag_config();
        let orchestrator = Self::from_dag_config(dag_config.clone())?;

        // Apply cache setting
        let mut registry_config = dag_config.registry.clone();
        if !use_cache {
            registry_config.no_cache = true;
        }

        let mut factory_ctx = FactoryContext::new(registry_config)?;

        for node_def in &dag_config.nodes {
            let any_node = create_node(node_def, &mut factory_ctx).await?;
            orchestrator.register_node(&node_def.id, any_node).await?;
        }

        orchestrator.wire_queues().await?;

        // Store factory context for epoch ticker cleanup
        {
            let mut ctx_guard = orchestrator.factory_ctx.lock().await;
            *ctx_guard = Some(factory_ctx);
        }

        Ok(orchestrator)
    }

    /// Build a DAG orchestrator from DagConfig (low-level).
    ///
    /// This creates the orchestrator without creating nodes. You must call
    /// `register_node` for each node and then `wire_queues` before running.
    ///
    /// For most use cases, prefer `from_config` which handles all setup.
    ///
    /// Validates the topology at construction time:
    /// - No cycles (would fail topological sort)
    /// - Exactly one source (no incoming edges)
    /// - Exactly one sink (no outgoing edges)
    /// - No orphan nodes (all connected to main graph)
    #[must_use = "creating an orchestrator without using it is likely a bug"]
    pub fn from_dag_config(config: DagConfig) -> Result<Self> {
        let mut graph = DiGraph::new();
        let mut node_indices = HashMap::new();

        for node_def in &config.nodes {
            let idx = graph.add_node(node_def.id.clone());
            node_indices.insert(node_def.id.clone(), idx);
        }

        for edge_def in &config.edges {
            let from_idx = node_indices.get(&edge_def.from).ok_or_else(|| {
                WaferError::Config(ConfigError::Message(format!(
                    "Unknown source node in edge: {}",
                    edge_def.from
                )))
            })?;
            let to_idx = node_indices.get(&edge_def.to).ok_or_else(|| {
                WaferError::Config(ConfigError::Message(format!(
                    "Unknown destination node in edge: {}",
                    edge_def.to
                )))
            })?;
            graph.add_edge(*from_idx, *to_idx, ());
        }

        let topo_indices = toposort(&graph, None).map_err(|_| {
            WaferError::Config(ConfigError::Message("Cycle detected in DAG".into()))
        })?;
        let topo_order: Vec<String> = topo_indices.iter().map(|idx| graph[*idx].clone()).collect();

        let pipeline_name = config.pipeline.name.clone();
        let orchestrator = Self {
            graph,
            node_indices,
            config,
            topo_order,
            nodes: Mutex::new(HashMap::new()),
            run_state: Mutex::new(Some(RunState {
                queue_senders: HashMap::new(),
                queue_receivers: HashMap::new(),
            })),
            cancel_token: CancellationToken::new(),
            control_state: Arc::new(ControlState::new(pipeline_name)),
            factory_ctx: Mutex::new(None),
            swap_locks: Mutex::new(HashMap::new()),
        };
        orchestrator.validate()?;
        Ok(orchestrator)
    }

    /// Register a node instance for execution.
    ///
    /// # Errors
    ///
    /// Returns an error if the node ID is not defined in the DAG configuration.
    pub async fn register_node(&self, id: &str, node: AnyNode) -> Result<()> {
        if !self.node_indices.contains_key(id) {
            return Err(WaferError::Config(ConfigError::Message(format!(
                "Unknown node ID: {id}"
            ))));
        }
        let mut nodes = self.nodes.lock().await;
        nodes.insert(id.to_string(), Arc::new(Mutex::new(node)));
        Ok(())
    }

    /// Create queues for each edge in the DAG.
    ///
    /// Each edge gets a bounded queue with capacity from the edge config
    /// or the default queue capacity.
    ///
    /// Queue keys include port information for router/joiner support:
    /// - Key format: `(\"from_node:from_port\", \"to_node:to_port\")`
    /// - Default port is \"default\" when not specified
    ///
    /// # Note
    ///
    /// This method is async because the run_state is protected by a Mutex.
    /// It must be called before `run()`.
    pub async fn wire_queues(&self) -> Result<()> {
        let mut run_state_guard = self.run_state.lock().await;
        let run_state = run_state_guard.as_mut().ok_or_else(|| {
            WaferError::Runtime("Cannot wire queues: run state already consumed".into())
        })?;

        for edge in &self.config.edges {
            let capacity = edge
                .queue_capacity
                .unwrap_or(self.config.default_queue_capacity);
            let queue = BoundedQueue::new(capacity);
            let (sender, receiver) = queue.split();

            let from_port = edge.from_port.as_deref().unwrap_or("default");
            let to_port = edge.to_port.as_deref().unwrap_or("default");
            let key = (
                format!("{}:{}", edge.from, from_port),
                format!("{}:{}", edge.to, to_port),
            );
            run_state.queue_senders.insert(key.clone(), sender);
            run_state.queue_receivers.insert(key, receiver);
        }
        Ok(())
    }

    /// Validate that all nodes defined in config are registered.
    pub(super) async fn validate_nodes_registered(&self) -> Result<()> {
        let nodes = self.nodes.lock().await;
        let missing: Vec<_> = self
            .node_indices
            .keys()
            .filter(|id| !nodes.contains_key(*id))
            .collect();

        if !missing.is_empty() {
            return Err(WaferError::Config(ConfigError::Message(format!(
                "Missing node registrations: {missing:?}"
            ))));
        }
        Ok(())
    }

    /// Validate the DAG topology.
    fn validate(&self) -> Result<()> {
        self.validate_not_empty()?;
        self.validate_no_orphans()?;
        Ok(())
    }

    fn validate_not_empty(&self) -> Result<()> {
        if self.node_indices.is_empty() {
            return Err(WaferError::Config(ConfigError::Message(
                "DAG has no nodes".into(),
            )));
        }
        Ok(())
    }

    fn validate_no_orphans(&self) -> Result<()> {
        if self.node_indices.len() == 1 {
            return Ok(());
        }

        let orphans: Vec<&str> = self
            .node_indices
            .iter()
            .filter(|(_, idx)| {
                let incoming = self
                    .graph
                    .neighbors_directed(**idx, Direction::Incoming)
                    .count();
                let outgoing = self
                    .graph
                    .neighbors_directed(**idx, Direction::Outgoing)
                    .count();
                incoming == 0 && outgoing == 0
            })
            .map(|(id, _)| id.as_str())
            .collect();

        if orphans.is_empty() {
            Ok(())
        } else {
            Err(WaferError::Config(ConfigError::Message(format!(
                "Orphan nodes found (no connections): {orphans:?}"
            ))))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EdgeDefinition, NodeDefinition, NodeType, PipelineConfig};
    use crate::node::FileSource;
    use crate::registry::RegistryConfig;

    fn make_node(id: &str, node_type: NodeType) -> NodeDefinition {
        NodeDefinition {
            id: id.to_string(),
            node_type,
            source_type: None,
            sink_type: None,
            config: toml::Value::Table(toml::map::Map::new()),
        }
    }

    fn make_edge(from: &str, to: &str) -> EdgeDefinition {
        EdgeDefinition {
            from: from.to_string(),
            to: to.to_string(),
            from_port: None,
            to_port: None,
            queue_capacity: None,
        }
    }

    fn make_dag_config(nodes: Vec<NodeDefinition>, edges: Vec<EdgeDefinition>) -> DagConfig {
        DagConfig {
            pipeline: PipelineConfig::default(),
            nodes,
            edges,
            default_queue_capacity: 1024,
            registry: RegistryConfig::default(),
        }
    }

    #[test]
    fn test_dag_valid_linear() {
        let config = make_dag_config(
            vec![
                make_node("source", NodeType::Source),
                make_node("transform", NodeType::Transform),
                make_node("sink", NodeType::Sink),
            ],
            vec![
                make_edge("source", "transform"),
                make_edge("transform", "sink"),
            ],
        );

        let orchestrator = DagOrchestrator::from_dag_config(config).unwrap();
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
            vec![
                make_edge("a", "b"),
                make_edge("b", "c"),
                make_edge("c", "a"),
            ],
        );

        let result = DagOrchestrator::from_dag_config(config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Cycle"));
    }

    #[test]
    fn test_dag_no_source() {
        let config = make_dag_config(
            vec![
                make_node("transform", NodeType::Transform),
                make_node("sink", NodeType::Sink),
            ],
            vec![
                make_edge("sink", "transform"),
                make_edge("transform", "sink"),
            ],
        );

        let result = DagOrchestrator::from_dag_config(config);
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

        let orchestrator = DagOrchestrator::from_dag_config(config).unwrap();
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

        let result = DagOrchestrator::from_dag_config(config);
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

        let result = DagOrchestrator::from_dag_config(config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown"));
    }

    #[test]
    fn test_dag_single_node() {
        let config = make_dag_config(vec![make_node("single", NodeType::Source)], vec![]);

        let orchestrator = DagOrchestrator::from_dag_config(config).unwrap();
        assert_eq!(orchestrator.topo_order(), &["single"]);
    }

    #[test]
    fn test_dag_empty_rejected() {
        let config = make_dag_config(vec![], vec![]);

        let result = DagOrchestrator::from_dag_config(config);
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

        let orchestrator = DagOrchestrator::from_dag_config(config).unwrap();
        assert_eq!(orchestrator.node_count(), 3);
        assert_eq!(orchestrator.edge_count(), 2);
    }

    #[tokio::test]
    async fn test_wire_queues_creates_correct_count() {
        let config = make_dag_config(
            vec![
                make_node("source", NodeType::Source),
                make_node("transform", NodeType::Transform),
                make_node("sink", NodeType::Sink),
            ],
            vec![
                make_edge("source", "transform"),
                make_edge("transform", "sink"),
            ],
        );

        let orchestrator = DagOrchestrator::from_dag_config(config).unwrap();
        orchestrator.wire_queues().await.unwrap();

        let run_state = orchestrator.run_state.lock().await;
        let run_state = run_state.as_ref().unwrap();
        assert_eq!(run_state.queue_senders.len(), 2);
        assert_eq!(run_state.queue_receivers.len(), 2);
        assert!(run_state.queue_senders.contains_key(&(
            "source:default".to_string(),
            "transform:default".to_string()
        )));
        assert!(run_state
            .queue_senders
            .contains_key(&("transform:default".to_string(), "sink:default".to_string())));
    }

    #[tokio::test]
    async fn test_wire_queues_uses_custom_capacity() {
        let config = DagConfig {
            pipeline: PipelineConfig::default(),
            nodes: vec![
                make_node("source", NodeType::Source),
                make_node("sink", NodeType::Sink),
            ],
            edges: vec![EdgeDefinition {
                from: "source".to_string(),
                to: "sink".to_string(),
                from_port: None,
                to_port: None,
                queue_capacity: Some(42),
            }],
            default_queue_capacity: 1024,
            registry: RegistryConfig::default(),
        };

        let orchestrator = DagOrchestrator::from_dag_config(config).unwrap();
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

        let orchestrator = DagOrchestrator::from_dag_config(config).unwrap();
        let node = AnyNode::from_source(FileSource::new("wrong-id", "/tmp/test.txt"));
        let result = orchestrator.register_node("unknown", node).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown node ID"));
    }
}
