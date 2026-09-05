//! DAG graph construction from validated pipeline config.

use std::collections::HashMap;

use petgraph::Direction;
use petgraph::algo::toposort;
use petgraph::graph::{DiGraph, NodeIndex};
use wafer_types::config::Config;

use crate::error::ConfigError;

/// A validated, immutable DAG topology built from a [`Config`].
///
/// Construction validates for cycles and unknown node references. Orphan
/// detection and semantic checks are handled by the separate [`crate::validate`]
/// function; `DagGraph::from_config` only guarantees structural correctness.
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
    /// Returns [`ConfigError::UnknownNode`] if an edge references a node ID
    /// that does not exist, [`ConfigError::CycleDetected`] if the graph has a
    /// cycle, or [`ConfigError::EmptyPipeline`] if there are no nodes.
    pub fn from_config(config: &Config) -> Result<Self, ConfigError> {
        if config.nodes.is_empty() {
            return Err(ConfigError::EmptyPipeline);
        }

        let mut graph: DiGraph<String, ()> = DiGraph::new();
        let mut node_indices: HashMap<String, NodeIndex> = HashMap::new();

        for id in config.nodes.keys() {
            let idx = graph.add_node(id.clone());
            node_indices.insert(id.clone(), idx);
        }

        for edge in &config.edges {
            let from_idx = node_indices
                .get(&edge.from)
                .copied()
                .ok_or_else(|| ConfigError::UnknownNode { id: edge.from.clone() })?;
            let to_idx = node_indices
                .get(&edge.to)
                .copied()
                .ok_or_else(|| ConfigError::UnknownNode { id: edge.to.clone() })?;
            graph.add_edge(from_idx, to_idx, ());
        }

        let topo_indices = toposort(&graph, None).map_err(|_cycle| ConfigError::CycleDetected)?;

        let topo_order: Vec<String> = topo_indices.iter().map(|idx| graph[*idx].clone()).collect();

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

    /// Returns the IDs of all nodes with at least one inbound edge from `source_id`.
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

#[cfg(test)]
mod tests {
    use super::*;
    use wafer_types::config::{
        Config, EdgeDef, NodeDef, PluginSpec, SinkDef, SourceDef, StdinSourceConfig,
        StdoutSinkConfig, WasmNodeDef,
    };

    fn stdin_node() -> NodeDef {
        NodeDef::Source(SourceDef::Stdin(StdinSourceConfig::default()))
    }

    fn stdout_node() -> NodeDef {
        NodeDef::Sink(SinkDef::Stdout(StdoutSinkConfig::default()))
    }

    fn transform_node() -> NodeDef {
        NodeDef::Transform(WasmNodeDef {
            plugin: PluginSpec::WasmPath("t.wasm".to_string()),
            ..Default::default()
        })
    }

    fn edge(from: &str, to: &str) -> EdgeDef {
        EdgeDef {
            from: from.to_string(),
            to: to.to_string(),
            port: None,
            capacity: None,
            overflow: None,
        }
    }

    fn config(nodes: Vec<(&str, NodeDef)>, edges: Vec<EdgeDef>) -> Config {
        Config {
            nodes: nodes.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
            edges,
            ..Default::default()
        }
    }

    #[test]
    fn test_topo_order_linear() {
        let cfg = config(
            vec![("a", stdin_node()), ("b", transform_node()), ("c", stdout_node())],
            vec![edge("a", "b"), edge("b", "c")],
        );
        let dag = DagGraph::from_config(&cfg).unwrap();
        let order = dag.topo_order();
        // a before b before c
        let pos_a = order.iter().position(|s| s == "a").unwrap();
        let pos_b = order.iter().position(|s| s == "b").unwrap();
        let pos_c = order.iter().position(|s| s == "c").unwrap();
        assert!(pos_a < pos_b);
        assert!(pos_b < pos_c);
    }

    #[test]
    fn test_topo_order_diamond() {
        // a → b, a → c, b → d, c → d
        let cfg = config(
            vec![
                ("a", stdin_node()),
                ("b", transform_node()),
                ("c", transform_node()),
                ("d", stdout_node()),
            ],
            vec![edge("a", "b"), edge("a", "c"), edge("b", "d"), edge("c", "d")],
        );
        let dag = DagGraph::from_config(&cfg).unwrap();
        let order = dag.topo_order();
        let pos = |name: &str| order.iter().position(|s| s == name).unwrap();
        // a must come before b and c; b and c must come before d
        assert!(pos("a") < pos("b"));
        assert!(pos("a") < pos("c"));
        assert!(pos("b") < pos("d"));
        assert!(pos("c") < pos("d"));
    }

    #[test]
    fn test_from_config_builds_graph() {
        let cfg = config(
            vec![("source", stdin_node()), ("t", transform_node()), ("sink", stdout_node())],
            vec![edge("source", "t"), edge("t", "sink")],
        );
        let dag = DagGraph::from_config(&cfg).unwrap();
        assert_eq!(dag.node_count(), 3);
        assert_eq!(dag.edge_count(), 2);
        assert!(dag.contains_node("source"));
        assert!(dag.contains_node("t"));
        assert!(dag.contains_node("sink"));
    }

    #[test]
    fn test_cycle_detection() {
        let cfg = config(
            vec![("a", transform_node()), ("b", transform_node()), ("c", transform_node())],
            vec![edge("a", "b"), edge("b", "c"), edge("c", "a")],
        );
        let result = DagGraph::from_config(&cfg);
        assert!(matches!(result, Err(ConfigError::CycleDetected)));
    }

    #[test]
    fn test_empty_pipeline_is_rejected() {
        let cfg = Config::default();
        let result = DagGraph::from_config(&cfg);
        assert!(matches!(result, Err(ConfigError::EmptyPipeline)));
    }

    #[test]
    fn test_unknown_node_in_edge_is_rejected() {
        let cfg = config(vec![("source", stdin_node())], vec![edge("source", "nonexistent")]);
        let result = DagGraph::from_config(&cfg);
        assert!(matches!(result, Err(ConfigError::UnknownNode { .. })));
    }
}
