use petgraph::algo::toposort;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::Direction;
use std::collections::HashMap;

use crate::config::DagConfig;
use crate::error::{ConfigError, Result, WaferError};

/// DAG orchestrator that manages graph topology and validation.
#[derive(Debug)]
pub struct DagOrchestrator {
    graph: DiGraph<String, ()>,
    node_indices: HashMap<String, NodeIndex>,
    config: DagConfig,
    topo_order: Vec<String>,
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
        };
        orchestrator.validate()?;
        Ok(orchestrator)
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
                "Multiple source nodes found (expected 1): {:?}",
                sources
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
                "Multiple sink nodes found (expected 1): {:?}",
                sinks
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
                "Orphan nodes found (no connections): {:?}",
                orphans
            ))))
        }
    }

    pub fn topo_order(&self) -> &[String] {
        &self.topo_order
    }

    pub fn config(&self) -> &DagConfig {
        &self.config
    }

    pub fn node_count(&self) -> usize {
        self.node_indices.len()
    }

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
}
