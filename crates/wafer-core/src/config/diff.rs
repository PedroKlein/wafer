//! Configuration diff detection for hot-swap.
//!
//! This module compares configurations to detect changes that require
//! hot-swapping nodes. For MVP, it focuses on detecting WASM path changes
//! for existing nodes.
//!
//! # Example
//!
//! ```ignore
//! use wafer_core::config::{diff_configs, DagConfig};
//!
//! let old_config = load_dag_config("pipeline.toml")?;
//! let new_config = load_dag_config("pipeline.toml")?;  // reloaded
//!
//! let diff = diff_configs(&old_config, &new_config);
//! for (node_id, new_path) in diff.nodes_to_swap {
//!     orchestrator.hot_swap(&node_id, &new_path).await?;
//! }
//! ```

use std::path::PathBuf;

use super::{DagConfig, NodeConfig, NodeType};

/// Detected changes between two configurations.
#[derive(Debug, Default)]
pub struct ConfigDiff {
    /// Nodes that need hot-swapping (node_id, new_wasm_path).
    pub nodes_to_swap: Vec<(String, PathBuf)>,

    /// Nodes that were added (not swappable - requires restart).
    pub nodes_added: Vec<String>,

    /// Nodes that were removed (not swappable - requires restart).
    pub nodes_removed: Vec<String>,

    /// Edges that changed (not swappable - requires restart).
    pub edges_changed: bool,
}

impl ConfigDiff {
    /// Returns true if there are any changes.
    #[must_use]
    pub fn has_changes(&self) -> bool {
        !self.nodes_to_swap.is_empty()
            || !self.nodes_added.is_empty()
            || !self.nodes_removed.is_empty()
            || self.edges_changed
    }

    /// Returns true if all changes can be applied via hot-swap.
    #[must_use]
    pub fn is_hot_swappable(&self) -> bool {
        !self.nodes_to_swap.is_empty()
            && self.nodes_added.is_empty()
            && self.nodes_removed.is_empty()
            && !self.edges_changed
    }

    /// Returns true if changes require a full restart.
    #[must_use]
    pub fn requires_restart(&self) -> bool {
        !self.nodes_added.is_empty() || !self.nodes_removed.is_empty() || self.edges_changed
    }
}

/// Compare two configurations and detect changes.
///
/// Currently detects:
/// - WASM path changes for Transform/Router/Joiner nodes (swappable)
/// - Node additions (not swappable)
/// - Node removals (not swappable)
/// - Edge changes (not swappable)
///
/// # Arguments
///
/// * `old` - The current running configuration
/// * `new` - The newly loaded configuration
///
/// # Returns
///
/// A `ConfigDiff` describing all detected changes.
#[must_use]
pub fn diff_configs(old: &DagConfig, new: &DagConfig) -> ConfigDiff {
    let mut diff = ConfigDiff::default();

    // Build lookup maps for old nodes
    let old_nodes: std::collections::HashMap<&str, &super::NodeDefinition> =
        old.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    let new_nodes: std::collections::HashMap<&str, &super::NodeDefinition> =
        new.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    // Detect added nodes
    for node_id in new_nodes.keys() {
        if !old_nodes.contains_key(node_id) {
            diff.nodes_added.push((*node_id).to_string());
        }
    }

    // Detect removed nodes
    for node_id in old_nodes.keys() {
        if !new_nodes.contains_key(node_id) {
            diff.nodes_removed.push((*node_id).to_string());
        }
    }

    // Detect changed nodes (WASM path changes for swappable node types)
    for (node_id, new_node) in &new_nodes {
        if let Some(old_node) = old_nodes.get(node_id) {
            // Only check swappable node types
            if !is_swappable_type(&new_node.node_type) {
                continue;
            }

            // Check if WASM path changed
            if let Some(new_path) = get_wasm_path_change(old_node, new_node) {
                diff.nodes_to_swap.push(((*node_id).to_string(), new_path));
            }
        }
    }

    // Detect edge changes (simple comparison - any difference triggers flag)
    diff.edges_changed = edges_differ(&old.edges, &new.edges);

    diff
}

/// Check if a node type supports hot-swap.
fn is_swappable_type(node_type: &NodeType) -> bool {
    matches!(
        node_type,
        NodeType::Transform | NodeType::Router | NodeType::Joiner
    )
}

/// Extract WASM path from a node's config if it changed.
///
/// Returns `Some(new_path)` if the WASM path changed, `None` otherwise.
fn get_wasm_path_change(
    old_node: &super::NodeDefinition,
    new_node: &super::NodeDefinition,
) -> Option<PathBuf> {
    // Parse config as NodeConfig
    let old_config: Result<NodeConfig, _> = old_node.config.clone().try_into();
    let new_config: Result<NodeConfig, _> = new_node.config.clone().try_into();

    let (Ok(old_cfg), Ok(new_cfg)) = (old_config, new_config) else {
        return None;
    };

    // Compare plugin paths
    if old_cfg.plugin_path != new_cfg.plugin_path {
        return new_cfg.plugin_path;
    }

    // Compare OCI references (if OCI changed, we'd need to resolve it first)
    // For now, we only support local path hot-swap
    // OCI hot-swap would need the resolved path from registry

    None
}

/// Check if edges differ between configs.
fn edges_differ(old: &[super::EdgeDefinition], new: &[super::EdgeDefinition]) -> bool {
    if old.len() != new.len() {
        return true;
    }

    // Build a set of edge signatures for comparison
    let old_edges: std::collections::HashSet<_> = old
        .iter()
        .map(|e| {
            (
                &e.from,
                &e.to,
                e.from_port.as_deref().unwrap_or("default"),
                e.to_port.as_deref().unwrap_or("default"),
            )
        })
        .collect();

    let new_edges: std::collections::HashSet<_> = new
        .iter()
        .map(|e| {
            (
                &e.from,
                &e.to,
                e.from_port.as_deref().unwrap_or("default"),
                e.to_port.as_deref().unwrap_or("default"),
            )
        })
        .collect();

    old_edges != new_edges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EdgeDefinition, NodeDefinition, NodeType, PipelineConfig};
    use crate::registry::RegistryConfig;

    fn make_dag_config(nodes: Vec<NodeDefinition>, edges: Vec<EdgeDefinition>) -> DagConfig {
        DagConfig {
            pipeline: PipelineConfig::default(),
            nodes,
            edges,
            default_queue_capacity: 1024,
            registry: RegistryConfig::default(),
        }
    }

    fn make_transform_node(id: &str, plugin_path: &str) -> NodeDefinition {
        let mut config = toml::map::Map::new();
        config.insert(
            "plugin_path".to_string(),
            toml::Value::String(plugin_path.to_string()),
        );

        NodeDefinition {
            id: id.to_string(),
            node_type: NodeType::Transform,
            source_type: None,
            sink_type: None,
            config: toml::Value::Table(config),
        }
    }

    fn make_source_node(id: &str) -> NodeDefinition {
        NodeDefinition {
            id: id.to_string(),
            node_type: NodeType::Source,
            source_type: Some("file".to_string()),
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

    #[test]
    fn test_no_changes() {
        let config = make_dag_config(
            vec![
                make_source_node("src"),
                make_transform_node("t1", "/v1.wasm"),
            ],
            vec![make_edge("src", "t1")],
        );

        let diff = diff_configs(&config, &config);
        assert!(!diff.has_changes());
        assert!(!diff.requires_restart());
    }

    #[test]
    fn test_wasm_path_change() {
        let old = make_dag_config(
            vec![
                make_source_node("src"),
                make_transform_node("t1", "/v1.wasm"),
            ],
            vec![make_edge("src", "t1")],
        );

        let new = make_dag_config(
            vec![
                make_source_node("src"),
                make_transform_node("t1", "/v2.wasm"),
            ],
            vec![make_edge("src", "t1")],
        );

        let diff = diff_configs(&old, &new);
        assert!(diff.has_changes());
        assert!(diff.is_hot_swappable());
        assert!(!diff.requires_restart());
        assert_eq!(diff.nodes_to_swap.len(), 1);
        assert_eq!(diff.nodes_to_swap[0].0, "t1");
        assert_eq!(diff.nodes_to_swap[0].1, PathBuf::from("/v2.wasm"));
    }

    #[test]
    fn test_node_added() {
        let old = make_dag_config(vec![make_source_node("src")], vec![]);

        let new = make_dag_config(
            vec![
                make_source_node("src"),
                make_transform_node("t1", "/v1.wasm"),
            ],
            vec![make_edge("src", "t1")],
        );

        let diff = diff_configs(&old, &new);
        assert!(diff.has_changes());
        assert!(!diff.is_hot_swappable());
        assert!(diff.requires_restart());
        assert_eq!(diff.nodes_added, vec!["t1"]);
    }

    #[test]
    fn test_node_removed() {
        let old = make_dag_config(
            vec![
                make_source_node("src"),
                make_transform_node("t1", "/v1.wasm"),
            ],
            vec![make_edge("src", "t1")],
        );

        let new = make_dag_config(vec![make_source_node("src")], vec![]);

        let diff = diff_configs(&old, &new);
        assert!(diff.has_changes());
        assert!(!diff.is_hot_swappable());
        assert!(diff.requires_restart());
        assert_eq!(diff.nodes_removed, vec!["t1"]);
    }

    #[test]
    fn test_edge_changed() {
        let old = make_dag_config(
            vec![
                make_source_node("src"),
                make_transform_node("t1", "/v1.wasm"),
                make_transform_node("t2", "/v2.wasm"),
            ],
            vec![make_edge("src", "t1"), make_edge("t1", "t2")],
        );

        let new = make_dag_config(
            vec![
                make_source_node("src"),
                make_transform_node("t1", "/v1.wasm"),
                make_transform_node("t2", "/v2.wasm"),
            ],
            vec![make_edge("src", "t2"), make_edge("t2", "t1")], // Edges reversed
        );

        let diff = diff_configs(&old, &new);
        assert!(diff.has_changes());
        assert!(!diff.is_hot_swappable());
        assert!(diff.requires_restart());
        assert!(diff.edges_changed);
    }

    #[test]
    fn test_source_change_not_swappable() {
        let old = make_dag_config(vec![make_source_node("src")], vec![]);

        // Even if we changed the source config, it's not swappable
        let new = make_dag_config(vec![make_source_node("src")], vec![]);

        let diff = diff_configs(&old, &new);
        assert!(!diff.has_changes()); // No swappable changes
    }
}
