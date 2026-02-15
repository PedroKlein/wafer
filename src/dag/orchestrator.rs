//! DAG orchestrator for multi-node pipeline execution.
//!
//! Manages graph topology, node lifecycle, queue wiring, and coordinated
//! async execution of all nodes in the pipeline.

use petgraph::algo::toposort;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::Direction;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::config::DagConfig;
use crate::error::{ConfigError, Result, WaferError};
use crate::node::{AnyNode, ProcessResult, Sink, Source, Transform};
use crate::queue::{BoundedQueue, QueueReceiver, QueueSender, RuntimeEnvelope};

/// DAG orchestrator that manages graph topology, node execution, and inter-node communication.
///
/// The orchestrator handles:
/// - Graph topology validation (cycle detection, source/sink constraints)
/// - Node instance registration and lifecycle management
/// - Queue wiring for inter-node message passing
/// - Coordinated async execution of all nodes with proper shutdown
pub struct DagOrchestrator {
    graph: DiGraph<String, ()>,
    node_indices: HashMap<String, NodeIndex>,
    config: DagConfig,
    topo_order: Vec<String>,
    /// Node instances keyed by node ID
    nodes: HashMap<String, Arc<Mutex<AnyNode>>>,
    /// Queue senders for each edge (from_node, to_node)
    queue_senders: HashMap<(String, String), QueueSender<RuntimeEnvelope>>,
    /// Queue receivers for each edge (from_node, to_node)
    queue_receivers: HashMap<(String, String), QueueReceiver<RuntimeEnvelope>>,
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
    /// Build a DAG orchestrator from configuration.
    ///
    /// Validates the topology at construction time:
    /// - No cycles (would fail topological sort)
    /// - Exactly one source (no incoming edges)
    /// - Exactly one sink (no outgoing edges)
    /// - No orphan nodes (all connected to main graph)
    pub fn from_config(config: DagConfig) -> Result<Self> {
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

        let orchestrator = Self {
            graph,
            node_indices,
            config,
            topo_order,
            nodes: HashMap::new(),
            queue_senders: HashMap::new(),
            queue_receivers: HashMap::new(),
        };
        orchestrator.validate()?;
        Ok(orchestrator)
    }

    /// Register a node instance for execution.
    pub fn register_node(&mut self, id: &str, node: AnyNode) -> Result<()> {
        if !self.node_indices.contains_key(id) {
            return Err(WaferError::Config(ConfigError::Message(format!(
                "Unknown node ID: {id}"
            ))));
        }
        self.nodes
            .insert(id.to_string(), Arc::new(Mutex::new(node)));
        Ok(())
    }

    /// Create queues for each edge in the DAG.
    pub fn wire_queues(&mut self) -> Result<()> {
        for edge in &self.config.edges {
            let capacity = edge
                .queue_capacity
                .unwrap_or(self.config.default_queue_capacity);
            let queue = BoundedQueue::new(capacity);
            let (sender, receiver) = queue.split();
            let key = (edge.from.clone(), edge.to.clone());
            self.queue_senders.insert(key.clone(), sender);
            self.queue_receivers.insert(key, receiver);
        }
        Ok(())
    }

    /// Run the DAG pipeline.
    ///
    /// This method:
    /// 1. Initializes all nodes in topological order
    /// 2. Spawns async tasks for each node
    /// 3. Waits for all tasks to complete
    /// 4. Closes all nodes in reverse topological order
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

            // Collect output senders for this node
            let output_senders: Vec<_> = self
                .queue_senders
                .iter()
                .filter(|((from, _), _)| from == node_id)
                .map(|((_, to), sender)| (to.clone(), sender.clone()))
                .collect();

            // Take ownership of input receivers for this node
            let input_receivers: Vec<_> = self
                .queue_receivers
                .keys()
                .filter(|(_, to)| to == node_id)
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .filter_map(|key| self.queue_receivers.remove(&key))
                .collect();

            if let Some(node_arc) = node {
                let handle = tokio::spawn(async move {
                    Self::run_node_loop(node_id_owned, node_arc, input_receivers, output_senders)
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

    async fn run_node_loop(
        node_id: String,
        node: Arc<Mutex<AnyNode>>,
        input_receivers: Vec<QueueReceiver<RuntimeEnvelope>>,
        output_senders: Vec<(String, QueueSender<RuntimeEnvelope>)>,
    ) {
        let mut locked = node.lock().await;

        match &mut *locked {
            AnyNode::Source(source) => {
                Self::run_source_loop(&node_id, source.as_mut(), &output_senders).await;
            }
            AnyNode::Transform(transform) => {
                if let Some(receiver) = input_receivers.into_iter().next() {
                    Self::run_transform_loop(
                        &node_id,
                        transform.as_mut(),
                        receiver,
                        &output_senders,
                    )
                    .await;
                }
            }
            AnyNode::Sink(sink) => {
                if let Some(receiver) = input_receivers.into_iter().next() {
                    Self::run_sink_loop(&node_id, sink.as_mut(), receiver).await;
                }
            }
        }
    }

    async fn run_source_loop(
        node_id: &str,
        source: &mut dyn Source,
        output_senders: &[(String, QueueSender<RuntimeEnvelope>)],
    ) {
        loop {
            match source.poll().await {
                Ok(Some(envelope)) => {
                    // Optimization: avoid clone for single downstream
                    if output_senders.len() == 1 {
                        if let Err(e) = output_senders[0].1.send(envelope).await {
                            tracing::warn!(node = %node_id, error = %e, "Failed to send to downstream");
                        }
                    } else {
                        // Clone for all downstream senders
                        // Note: We clone for all senders since we're iterating. Future optimization
                        // could use ownership tracking to avoid the final clone.
                        for (_, sender) in output_senders {
                            if let Err(e) = sender.send(envelope.clone()).await {
                                tracing::warn!(node = %node_id, error = %e, "Failed to send to downstream");
                            }
                        }
                    }
                }
                Ok(None) => {
                    tracing::debug!(node = %node_id, "Source reached EOF");
                    break;
                }
                Err(e) => {
                    tracing::error!(node = %node_id, error = %e, "Source poll error");
                    break;
                }
            }
        }
    }

    async fn run_transform_loop(
        node_id: &str,
        transform: &mut dyn Transform,
        mut receiver: QueueReceiver<RuntimeEnvelope>,
        output_senders: &[(String, QueueSender<RuntimeEnvelope>)],
    ) {
        // Async receive - properly yields to tokio runtime
        while let Some(envelope) = receiver.recv().await {
            match transform.process(envelope).await {
                Ok(ProcessResult::Emit(output)) => {
                    // Optimization: avoid clone for single downstream
                    if output_senders.len() == 1 {
                        if let Err(e) = output_senders[0].1.send(output).await {
                            tracing::warn!(node = %node_id, error = %e, "Failed to send to downstream");
                        }
                    } else {
                        for (_, sender) in output_senders {
                            if let Err(e) = sender.send(output.clone()).await {
                                tracing::warn!(node = %node_id, error = %e, "Failed to send to downstream");
                            }
                        }
                    }
                }
                Ok(ProcessResult::Filter) => {}
                Ok(ProcessResult::Error(e)) => {
                    tracing::warn!(
                        node = %node_id,
                        code = %e.code,
                        message = %e.message,
                        "Transform error - continuing"
                    );
                }
                Err(e) => {
                    tracing::error!(node = %node_id, error = %e, "Transform process failed");
                }
            }
        }
        tracing::debug!(node = %node_id, "Input queue closed");
    }

    async fn run_sink_loop(
        node_id: &str,
        sink: &mut dyn Sink,
        mut receiver: QueueReceiver<RuntimeEnvelope>,
    ) {
        // Async receive - properly yields to tokio runtime
        while let Some(envelope) = receiver.recv().await {
            if let Err(e) = sink.collect(envelope).await {
                tracing::error!(node = %node_id, error = %e, "Sink collect failed");
            }
        }
        tracing::debug!(node = %node_id, "Input queue closed");
    }

    fn validate_nodes_registered(&self) -> Result<()> {
        let missing: Vec<_> = self
            .node_indices
            .keys()
            .filter(|id| !self.nodes.contains_key(*id))
            .collect();

        if !missing.is_empty() {
            return Err(WaferError::Config(ConfigError::Message(format!(
                "Missing node registrations: {missing:?}"
            ))));
        }
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        self.validate_not_empty()?;
        self.validate_single_source()?;
        self.validate_single_sink()?;
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

    fn validate_single_source(&self) -> Result<()> {
        let sources: Vec<&str> = self
            .node_indices
            .iter()
            .filter(|(_, idx)| {
                self.graph
                    .neighbors_directed(**idx, Direction::Incoming)
                    .count()
                    == 0
            })
            .map(|(id, _)| id.as_str())
            .collect();

        match sources.len() {
            0 => Err(WaferError::Config(ConfigError::Message(
                "No source node found (node with no incoming edges)".into(),
            ))),
            1 => Ok(()),
            _ => Err(WaferError::Config(ConfigError::Message(format!(
                "Multiple source nodes found (expected 1): {sources:?}"
            )))),
        }
    }

    fn validate_single_sink(&self) -> Result<()> {
        let sinks: Vec<&str> = self
            .node_indices
            .iter()
            .filter(|(_, idx)| {
                self.graph
                    .neighbors_directed(**idx, Direction::Outgoing)
                    .count()
                    == 0
            })
            .map(|(id, _)| id.as_str())
            .collect();

        match sinks.len() {
            0 => Err(WaferError::Config(ConfigError::Message(
                "No sink node found (node with no outgoing edges)".into(),
            ))),
            1 => Ok(()),
            _ => Err(WaferError::Config(ConfigError::Message(format!(
                "Multiple sink nodes found (expected 1): {sinks:?}"
            )))),
        }
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

    /// Get the topological order of nodes.
    pub fn topo_order(&self) -> &[String] {
        &self.topo_order
    }

    /// Get the DAG configuration.
    pub fn config(&self) -> &DagConfig {
        &self.config
    }

    /// Get the number of nodes in the DAG.
    pub fn node_count(&self) -> usize {
        self.node_indices.len()
    }

    /// Get the number of edges in the DAG.
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EdgeDefinition, NodeDefinition, NodeType};

    fn make_node(id: &str, node_type: NodeType) -> NodeDefinition {
        NodeDefinition {
            id: id.to_string(),
            node_type,
            config: toml::Value::Table(toml::map::Map::new()),
        }
    }

    fn make_edge(from: &str, to: &str) -> EdgeDefinition {
        EdgeDefinition {
            from: from.to_string(),
            to: to.to_string(),
            queue_capacity: None,
        }
    }

    #[test]
    fn test_dag_valid_linear() {
        let config = DagConfig {
            nodes: vec![
                make_node("source", NodeType::Source),
                make_node("transform", NodeType::Transform),
                make_node("sink", NodeType::Sink),
            ],
            edges: vec![
                make_edge("source", "transform"),
                make_edge("transform", "sink"),
            ],
            default_queue_capacity: 1024,
        };

        let orchestrator = DagOrchestrator::from_config(config).unwrap();
        assert_eq!(orchestrator.topo_order(), &["source", "transform", "sink"]);
        assert_eq!(orchestrator.node_count(), 3);
        assert_eq!(orchestrator.edge_count(), 2);
    }

    #[test]
    fn test_dag_cycle_rejected() {
        let config = DagConfig {
            nodes: vec![
                make_node("a", NodeType::Transform),
                make_node("b", NodeType::Transform),
                make_node("c", NodeType::Transform),
            ],
            edges: vec![
                make_edge("a", "b"),
                make_edge("b", "c"),
                make_edge("c", "a"),
            ],
            default_queue_capacity: 1024,
        };

        let result = DagOrchestrator::from_config(config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Cycle"));
    }

    #[test]
    fn test_dag_no_source() {
        let config = DagConfig {
            nodes: vec![
                make_node("transform", NodeType::Transform),
                make_node("sink", NodeType::Sink),
            ],
            edges: vec![
                make_edge("sink", "transform"),
                make_edge("transform", "sink"),
            ],
            default_queue_capacity: 1024,
        };

        let result = DagOrchestrator::from_config(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_dag_multiple_sources() {
        let config = DagConfig {
            nodes: vec![
                make_node("source1", NodeType::Source),
                make_node("source2", NodeType::Source),
                make_node("sink", NodeType::Sink),
            ],
            edges: vec![make_edge("source1", "sink"), make_edge("source2", "sink")],
            default_queue_capacity: 1024,
        };

        let result = DagOrchestrator::from_config(config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Multiple source"));
    }

    #[test]
    fn test_dag_orphan_node() {
        let config = DagConfig {
            nodes: vec![
                make_node("source", NodeType::Source),
                make_node("sink", NodeType::Sink),
                make_node("orphan", NodeType::Transform),
            ],
            edges: vec![make_edge("source", "sink")],
            default_queue_capacity: 1024,
        };

        let result = DagOrchestrator::from_config(config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Orphan") || err.contains("Multiple source"));
    }

    #[test]
    fn test_dag_unknown_node_in_edge() {
        let config = DagConfig {
            nodes: vec![make_node("source", NodeType::Source)],
            edges: vec![make_edge("source", "nonexistent")],
            default_queue_capacity: 1024,
        };

        let result = DagOrchestrator::from_config(config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown"));
    }

    #[test]
    fn test_dag_single_node() {
        let config = DagConfig {
            nodes: vec![make_node("single", NodeType::Source)],
            edges: vec![],
            default_queue_capacity: 1024,
        };

        let orchestrator = DagOrchestrator::from_config(config).unwrap();
        assert_eq!(orchestrator.topo_order(), &["single"]);
    }

    #[test]
    fn test_dag_empty_rejected() {
        let config = DagConfig {
            nodes: vec![],
            edges: vec![],
            default_queue_capacity: 1024,
        };

        let result = DagOrchestrator::from_config(config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no nodes"));
    }

    #[test]
    fn test_dag_multiple_sinks() {
        let config = DagConfig {
            nodes: vec![
                make_node("source", NodeType::Source),
                make_node("sink1", NodeType::Sink),
                make_node("sink2", NodeType::Sink),
            ],
            edges: vec![make_edge("source", "sink1"), make_edge("source", "sink2")],
            default_queue_capacity: 1024,
        };

        let result = DagOrchestrator::from_config(config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Multiple sink"));
    }

    #[test]
    fn test_wire_queues_creates_correct_count() {
        let config = DagConfig {
            nodes: vec![
                make_node("source", NodeType::Source),
                make_node("transform", NodeType::Transform),
                make_node("sink", NodeType::Sink),
            ],
            edges: vec![
                make_edge("source", "transform"),
                make_edge("transform", "sink"),
            ],
            default_queue_capacity: 1024,
        };

        let mut orchestrator = DagOrchestrator::from_config(config).unwrap();
        orchestrator.wire_queues().unwrap();

        assert_eq!(orchestrator.queue_senders.len(), 2);
        assert_eq!(orchestrator.queue_receivers.len(), 2);
        assert!(orchestrator
            .queue_senders
            .contains_key(&("source".to_string(), "transform".to_string())));
        assert!(orchestrator
            .queue_senders
            .contains_key(&("transform".to_string(), "sink".to_string())));
    }

    #[test]
    fn test_wire_queues_uses_custom_capacity() {
        let config = DagConfig {
            nodes: vec![
                make_node("source", NodeType::Source),
                make_node("sink", NodeType::Sink),
            ],
            edges: vec![EdgeDefinition {
                from: "source".to_string(),
                to: "sink".to_string(),
                queue_capacity: Some(42),
            }],
            default_queue_capacity: 1024,
        };

        let mut orchestrator = DagOrchestrator::from_config(config).unwrap();
        orchestrator.wire_queues().unwrap();

        let receiver = orchestrator
            .queue_receivers
            .get(&("source".to_string(), "sink".to_string()))
            .unwrap();
        assert_eq!(receiver.capacity(), 42);
    }

    #[test]
    fn test_register_node_unknown_id_fails() {
        let config = DagConfig {
            nodes: vec![make_node("source", NodeType::Source)],
            edges: vec![],
            default_queue_capacity: 1024,
        };

        let mut orchestrator = DagOrchestrator::from_config(config).unwrap();
        use crate::node::FileSource;
        let node = AnyNode::from_source(FileSource::new("wrong-id", "/tmp/test.txt"));
        let result = orchestrator.register_node("unknown", node);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown node ID"));
    }
}
