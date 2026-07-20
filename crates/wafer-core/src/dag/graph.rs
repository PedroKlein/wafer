//! DAG graph construction from validated pipeline config.

use std::collections::HashMap;

use petgraph::Direction;
use petgraph::algo::toposort;
use petgraph::graph::{DiGraph, NodeIndex};

use crate::config::Config;
use crate::error::{ConfigError, Result, WaferError};

/// A validated, immutable DAG topology.
///
/// Built from a [`Config`] and validated at construction time (no cycles, no
/// unknown edge endpoints, non-empty). Semantic config checks live in
/// `wafer-config`.
#[derive(Debug)]
pub struct DagGraph {
    graph: DiGraph<String, ()>,
    node_indices: HashMap<String, NodeIndex>,
    topo_order: Vec<String>,
}

impl DagGraph {
    /// Build a `DagGraph` from a deserialized config.
    ///
    /// # Errors
    ///
    /// Returns an error if an edge references an unknown node, the graph
    /// contains a cycle, or the config has no nodes.
    pub fn from_config(config: &Config) -> Result<Self> {
        if config.nodes.is_empty() {
            return Err(WaferError::Config(ConfigError::Message(
                "pipeline must contain at least one node".to_string(),
            )));
        }

        let mut graph = DiGraph::new();
        let mut node_indices = HashMap::new();

        for id in config.nodes.keys() {
            let idx = graph.add_node(id.clone());
            node_indices.insert(id.clone(), idx);
        }

        for edge in &config.edges {
            let from_idx = node_indices.get(&edge.from).copied().ok_or_else(|| {
                WaferError::Config(ConfigError::Message(format!(
                    "edge references unknown source node '{}'",
                    edge.from
                )))
            })?;
            let to_idx = node_indices.get(&edge.to).copied().ok_or_else(|| {
                WaferError::Config(ConfigError::Message(format!(
                    "edge references unknown destination node '{}'",
                    edge.to
                )))
            })?;
            graph.add_edge(from_idx, to_idx, ());
        }

        let topo_indices = toposort(&graph, None).map_err(|cycle| {
            let node_id = &graph[cycle.node_id()];
            WaferError::Config(ConfigError::Message(format!(
                "pipeline DAG contains a cycle involving node '{node_id}'"
            )))
        })?;

        let topo_order = topo_indices.iter().map(|idx| graph[*idx].clone()).collect();

        Ok(Self { graph, node_indices, topo_order })
    }

    /// Returns node IDs in topological order (sources before sinks).
    #[must_use]
    pub fn topo_order(&self) -> &[String] {
        &self.topo_order
    }

    /// Returns true if the graph contains a node with this ID.
    #[must_use]
    pub fn contains_node(&self, id: &str) -> bool {
        self.node_indices.contains_key(id)
    }

    /// Returns the number of nodes.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.node_indices.len()
    }

    /// Returns the number of directed edges.
    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    /// Returns the IDs of all nodes with at least one inbound edge from `id`.
    #[must_use]
    pub fn successors(&self, id: &str) -> Vec<String> {
        let Some(&idx) = self.node_indices.get(id) else {
            return Vec::new();
        };
        self.graph
            .neighbors_directed(idx, Direction::Outgoing)
            .map(|n| self.graph[n].clone())
            .collect()
    }
}
