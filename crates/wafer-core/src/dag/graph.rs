//! Pure domain struct for DAG graph topology.
//!
//! `DagGraph` owns the validated graph structure and provides synchronous
//! queries over it. No async, no tokio, no WASM knowledge.

use petgraph::algo::toposort;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::Direction;
use std::collections::HashMap;

use crate::config::DagConfig;
use crate::error::{ConfigError, Result, WaferError};

/// A validated, immutable DAG topology.
///
/// Built from a [`DagConfig`] and validated at construction time
/// (no cycles, no orphans, non-empty). All methods are synchronous.
#[derive(Debug)]
pub struct DagGraph {
    graph: DiGraph<String, ()>,
    node_indices: HashMap<String, NodeIndex>,
    topo_order: Vec<String>,
}

impl DagGraph {
    /// Build and validate a `DagGraph` from configuration.
    ///
    /// Returns an error if the graph contains cycles, orphan nodes,
    /// unknown node references in edges, or is empty.
    pub fn from_config(config: &DagConfig) -> Result<Self> {
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

        let dag = Self { graph, node_indices, topo_order };
        dag.validate()?;
        Ok(dag)
    }

    /// Returns the topologically sorted node IDs.
    pub fn topo_order(&self) -> &[String] {
        &self.topo_order
    }

    /// Returns the number of nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.node_indices.len()
    }

    /// Returns the number of edges in the graph.
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    /// Returns whether a node ID exists in the graph.
    pub fn contains_node(&self, id: &str) -> bool {
        self.node_indices.contains_key(id)
    }

    /// Returns the `NodeIndex` for a given node ID.
    pub fn node_index(&self, id: &str) -> Option<NodeIndex> {
        self.node_indices.get(id).copied()
    }

    /// Returns the internal node indices map.
    pub(crate) const fn node_indices(&self) -> &HashMap<String, NodeIndex> {
        &self.node_indices
    }

    fn validate(&self) -> Result<()> {
        self.validate_not_empty()?;
        self.validate_no_orphans()?;
        Ok(())
    }

    fn validate_not_empty(&self) -> Result<()> {
        if self.node_indices.is_empty() {
            return Err(WaferError::Config(ConfigError::Message("DAG has no nodes".into())));
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
                let incoming = self.graph.neighbors_directed(**idx, Direction::Incoming).count();
                let outgoing = self.graph.neighbors_directed(**idx, Direction::Outgoing).count();
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

    fn make_dag_config(nodes: Vec<NodeDefinition>, edges: Vec<EdgeDefinition>) -> DagConfig {
        DagConfig {
            pipeline: PipelineConfig::default(),
            nodes,
            edges,
            default_queue_capacity: 1024,
        }
    }

    // --- Task 1.1: DagGraph::from_config tests ---

    #[test]
    fn valid_linear_pipeline_returns_correct_topology() {
        let config = make_dag_config(
            vec![
                make_node("source", NodeType::Source),
                make_node("transform", NodeType::Transform),
                make_node("sink", NodeType::Sink),
            ],
            vec![make_edge("source", "transform"), make_edge("transform", "sink")],
        );

        let graph = DagGraph::from_config(&config).unwrap();
        assert_eq!(graph.topo_order(), &["source", "transform", "sink"]);
        assert_eq!(graph.node_count(), 3);
        assert_eq!(graph.edge_count(), 2);
    }

    #[test]
    fn contains_node_returns_true_for_existing_nodes() {
        let config = make_dag_config(
            vec![make_node("source", NodeType::Source), make_node("sink", NodeType::Sink)],
            vec![make_edge("source", "sink")],
        );

        let graph = DagGraph::from_config(&config).unwrap();
        assert!(graph.contains_node("source"));
        assert!(graph.contains_node("sink"));
        assert!(!graph.contains_node("nonexistent"));
    }

    // --- Task 1.2: Validation tests ---

    #[test]
    fn cycle_is_rejected() {
        let config = make_dag_config(
            vec![
                make_node("a", NodeType::Transform),
                make_node("b", NodeType::Transform),
                make_node("c", NodeType::Transform),
            ],
            vec![make_edge("a", "b"), make_edge("b", "c"), make_edge("c", "a")],
        );

        let result = DagGraph::from_config(&config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Cycle"));
    }

    #[test]
    fn orphan_node_is_rejected() {
        let config = make_dag_config(
            vec![
                make_node("source", NodeType::Source),
                make_node("sink", NodeType::Sink),
                make_node("orphan", NodeType::Transform),
            ],
            vec![make_edge("source", "sink")],
        );

        let result = DagGraph::from_config(&config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Orphan"));
    }

    #[test]
    fn empty_graph_is_rejected() {
        let config = make_dag_config(vec![], vec![]);

        let result = DagGraph::from_config(&config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no nodes"));
    }

    #[test]
    fn unknown_node_in_edge_is_rejected() {
        let config = make_dag_config(
            vec![make_node("source", NodeType::Source)],
            vec![make_edge("source", "nonexistent")],
        );

        let result = DagGraph::from_config(&config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown"));
    }

    #[test]
    fn single_node_graph_is_accepted() {
        let config = make_dag_config(vec![make_node("single", NodeType::Source)], vec![]);

        let graph = DagGraph::from_config(&config).unwrap();
        assert_eq!(graph.topo_order(), &["single"]);
        assert_eq!(graph.node_count(), 1);
        assert_eq!(graph.edge_count(), 0);
    }

    #[test]
    fn fan_out_topology_is_accepted() {
        let config = make_dag_config(
            vec![
                make_node("source", NodeType::Source),
                make_node("sink1", NodeType::Sink),
                make_node("sink2", NodeType::Sink),
            ],
            vec![make_edge("source", "sink1"), make_edge("source", "sink2")],
        );

        let graph = DagGraph::from_config(&config).unwrap();
        assert_eq!(graph.node_count(), 3);
        assert_eq!(graph.edge_count(), 2);
    }

    #[test]
    fn multiple_sources_topology_is_accepted() {
        let config = make_dag_config(
            vec![
                make_node("source1", NodeType::Source),
                make_node("source2", NodeType::Source),
                make_node("sink", NodeType::Sink),
            ],
            vec![make_edge("source1", "sink"), make_edge("source2", "sink")],
        );

        let graph = DagGraph::from_config(&config).unwrap();
        assert_eq!(graph.node_count(), 3);
        assert_eq!(graph.edge_count(), 2);
    }
}
