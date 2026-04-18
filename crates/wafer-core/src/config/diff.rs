//! Configuration diff detection for hot-swap.

use std::path::PathBuf;

use super::{Config, NodeConfig, NodeType};

/// Detected changes between two configurations.
#[derive(Debug, Default)]
pub struct ConfigDiff {
    pub nodes_to_swap: Vec<(String, PathBuf)>,
    pub nodes_added: Vec<String>,
    pub nodes_removed: Vec<String>,
    pub edges_changed: bool,
}

impl ConfigDiff {
    /// returns true if there are any changes.
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
#[must_use]
pub fn diff_configs(old: &Config, new: &Config) -> ConfigDiff {
    let mut diff = ConfigDiff::default();

    let old_nodes: std::collections::HashMap<&str, &super::NodeDefinition> =
        old.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    let new_nodes: std::collections::HashMap<&str, &super::NodeDefinition> =
        new.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    for node_id in new_nodes.keys() {
        if !old_nodes.contains_key(node_id) {
            diff.nodes_added.push((*node_id).to_string());
        }
    }

    for node_id in old_nodes.keys() {
        if !new_nodes.contains_key(node_id) {
            diff.nodes_removed.push((*node_id).to_string());
        }
    }

    for (node_id, new_node) in &new_nodes {
        if let Some(old_node) = old_nodes.get(node_id) {
            if !is_swappable_type(&new_node.node_type) {
                continue;
            }

            if let Some(new_path) = get_wasm_path_change(old_node, new_node) {
                diff.nodes_to_swap.push(((*node_id).to_string(), new_path));
            }
        }
    }

    diff.edges_changed = edges_differ(&old.edges, &new.edges);

    diff
}

/// Check if a node type supports hot-swap.
const fn is_swappable_type(node_type: &NodeType) -> bool {
    matches!(node_type, NodeType::Transform | NodeType::Router | NodeType::Joiner)
}

/// Extract WASM path from a node's config if it changed.
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

    if old_cfg.plugin_path != new_cfg.plugin_path {
        return new_cfg.plugin_path;
    }

    // OCI hot-swap would need the resolved path from registry
    // For now, we only support local path hot-swap

    None
}

/// Check if edges differ between configs.
fn edges_differ(old: &[super::EdgeDefinition], new: &[super::EdgeDefinition]) -> bool {
    if old.len() != new.len() {
        return true;
    }

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
    use crate::config::{EdgeDefinition, NodeDefinition, NodeType, OverflowPolicy, PipelineConfig};

    fn make_config(nodes: Vec<NodeDefinition>, edges: Vec<EdgeDefinition>) -> Config {
        Config {
            pipeline: PipelineConfig::default(),
            engine: Default::default(),
            api: Default::default(),
            metrics: Default::default(),
            nodes,
            edges,
            default_queue_capacity: 1024,
            registry: Default::default(),
            dead_letter: None,
        }
    }

    fn make_transform_node(id: &str, plugin_path: &str) -> NodeDefinition {
        let mut config = toml::map::Map::new();
        config.insert("plugin_path".to_string(), toml::Value::String(plugin_path.to_string()));

        NodeDefinition {
            id: id.to_string(),
            node_type: NodeType::Transform,
            source_type: None,
            sink_type: None,
            config: toml::Value::Table(config),
            capabilities: Default::default(),
        }
    }

    fn make_source_node(id: &str) -> NodeDefinition {
        NodeDefinition {
            id: id.to_string(),
            node_type: NodeType::Source,
            source_type: Some("file".to_string()),
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

    #[test]
    fn test_no_changes() {
        let config = make_config(
            vec![make_source_node("src"), make_transform_node("t1", "/v1.wasm")],
            vec![make_edge("src", "t1")],
        );

        let diff = diff_configs(&config, &config);
        assert!(!diff.has_changes());
        assert!(!diff.requires_restart());
    }

    #[test]
    fn test_wasm_path_change() {
        let old = make_config(
            vec![make_source_node("src"), make_transform_node("t1", "/v1.wasm")],
            vec![make_edge("src", "t1")],
        );

        let new = make_config(
            vec![make_source_node("src"), make_transform_node("t1", "/v2.wasm")],
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
        let old = make_config(vec![make_source_node("src")], vec![]);

        let new = make_config(
            vec![make_source_node("src"), make_transform_node("t1", "/v1.wasm")],
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
        let old = make_config(
            vec![make_source_node("src"), make_transform_node("t1", "/v1.wasm")],
            vec![make_edge("src", "t1")],
        );

        let new = make_config(vec![make_source_node("src")], vec![]);

        let diff = diff_configs(&old, &new);
        assert!(diff.has_changes());
        assert!(!diff.is_hot_swappable());
        assert!(diff.requires_restart());
        assert_eq!(diff.nodes_removed, vec!["t1"]);
    }

    #[test]
    fn test_edge_changed() {
        let old = make_config(
            vec![
                make_source_node("src"),
                make_transform_node("t1", "/v1.wasm"),
                make_transform_node("t2", "/v2.wasm"),
            ],
            vec![make_edge("src", "t1"), make_edge("t1", "t2")],
        );

        let new = make_config(
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
        let old = make_config(vec![make_source_node("src")], vec![]);

        // Even if we changed the source config, it's not swappable
        let new = make_config(vec![make_source_node("src")], vec![]);

        let diff = diff_configs(&old, &new);
        assert!(!diff.has_changes()); // No swappable changes
    }
}
