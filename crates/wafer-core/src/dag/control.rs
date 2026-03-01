//! PipelineControl trait implementation for DagOrchestrator.
//!
//! This module implements the `PipelineControl` trait for `DagOrchestrator`,
//! using the shared `ControlState` defined in `orchestrator.rs`.

use std::sync::atomic::Ordering;
use wafer_types::{
    ControlError, HotSwapResult, MetricsSnapshot, NodeInfo, NodeState, NodeType, PipelineState,
    PipelineStatus, ReloadResult,
};

use super::DagOrchestrator;
use crate::control::{EventReceiver, PipelineControl};

impl PipelineControl for DagOrchestrator {
    async fn hot_swap(&self, node_id: &str) -> Result<HotSwapResult, ControlError> {
        // Check if node exists
        if !self.node_indices.contains_key(node_id) {
            return Err(ControlError::NodeNotFound {
                node_id: node_id.to_string(),
            });
        }

        // Check if node is swappable (only transforms are swappable)
        let node_config = self.config.nodes.iter().find(|n| n.id == node_id);
        let is_transform =
            node_config.is_some_and(|n| matches!(n.node_type, crate::config::NodeType::Transform));

        if !is_transform {
            return Err(ControlError::NotSwappable {
                node_id: node_id.to_string(),
            });
        }

        // The trait method hot_swap(node_id) is for manual triggering of a single node.
        // To get the WASM path, we need to reload the config file.
        // This is useful for re-loading a node that was updated in-place (same path).
        let config_path = self.config_path.as_ref().ok_or_else(|| {
            ControlError::ConfigError {
                message: "Cannot hot-swap: no config path stored. Use reload_config() after creating orchestrator with from_config_with_path().".into(),
            }
        })?;

        // Reload config to get current path for this node
        let new_config =
            crate::config::load_dag_config(config_path).map_err(|e| ControlError::ConfigError {
                message: format!("Failed to reload config: {e}"),
            })?;

        // Find the node's WASM path in the new config
        let node_def = new_config
            .nodes
            .iter()
            .find(|n| n.id == node_id)
            .ok_or_else(|| ControlError::NodeNotFound {
                node_id: node_id.to_string(),
            })?;

        // Extract plugin path from config
        let node_cfg: crate::config::NodeConfig =
            node_def
                .config
                .clone()
                .try_into()
                .map_err(|e| ControlError::ConfigError {
                    message: format!("Invalid node config for {node_id}: {e}"),
                })?;

        let wasm_path = node_cfg
            .plugin_path
            .ok_or_else(|| ControlError::ConfigError {
                message: format!("Node {node_id} has no plugin_path configured"),
            })?;

        // Perform the hot-swap using the inherent method
        let metrics = DagOrchestrator::hot_swap(self, node_id, &wasm_path)
            .await
            .map_err(|e| ControlError::Internal {
                message: format!("Hot-swap failed: {e}"),
            })?;

        Ok(HotSwapResult {
            node_id: node_id.to_string(),
            drain_duration: metrics.drain_duration,
            load_duration: metrics.prepare_duration,
            total_duration: metrics.total_duration,
            messages_drained: 0, // TODO: track this in SwapMetrics
        })
    }

    async fn reload_config(&self) -> Result<ReloadResult, ControlError> {
        // Use resync() which handles config diffing and hot-swapping
        let swapped_nodes = self.resync().await.map_err(|e| {
            // Convert WaferError to ControlError
            match e {
                crate::error::WaferError::Runtime(msg) if msg.contains("no config path") => {
                    ControlError::ConfigError { message: msg }
                }
                crate::error::WaferError::Runtime(msg) if msg.contains("require restart") => {
                    ControlError::ConfigError { message: msg }
                }
                crate::error::WaferError::Config(cfg_err) => ControlError::ConfigError {
                    message: cfg_err.to_string(),
                },
                other => ControlError::Internal {
                    message: other.to_string(),
                },
            }
        })?;

        Ok(ReloadResult { swapped_nodes })
    }

    async fn drain(&self) -> Result<(), ControlError> {
        // Signal shutdown which effectively drains
        self.cancel_token.cancel();
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), ControlError> {
        // Signal shutdown
        self.cancel_token.cancel();
        Ok(())
    }

    fn status(&self) -> PipelineStatus {
        // Use control_state for actual metrics
        let state = if self.cancel_token.is_cancelled() {
            PipelineState::Draining
        } else {
            // Note: Can't await in sync fn, so we use try_lock
            self.control_state
                .state
                .try_lock()
                .map(|guard| *guard)
                .unwrap_or(PipelineState::Running)
        };

        PipelineStatus {
            name: self.control_state.name.clone(),
            state,
            uptime_secs: self.control_state.uptime_secs(),
            messages_processed: self
                .control_state
                .messages_processed
                .load(Ordering::Relaxed),
            messages_failed: self.control_state.messages_failed.load(Ordering::Relaxed),
            node_count: self.node_indices.len(),
            swap_in_progress: false,
        }
    }

    fn metrics(&self) -> MetricsSnapshot {
        // Use the Prometheus metrics registry when http-api feature is enabled
        #[cfg(feature = "http-api")]
        {
            self.control_state.metrics_registry.snapshot()
        }
        #[cfg(not(feature = "http-api"))]
        {
            MetricsSnapshot::default()
        }
    }

    fn nodes(&self) -> Vec<NodeInfo> {
        self.config
            .nodes
            .iter()
            .map(|node_def| {
                let node_type = match node_def.node_type {
                    crate::config::NodeType::Source => NodeType::Source,
                    crate::config::NodeType::Transform => NodeType::Transform,
                    crate::config::NodeType::Router => NodeType::Router,
                    crate::config::NodeType::Joiner => NodeType::Joiner,
                    crate::config::NodeType::Sink => NodeType::Sink,
                };

                let swappable = matches!(node_type, NodeType::Transform);

                NodeInfo {
                    id: node_def.id.clone(),
                    node_type,
                    state: NodeState::Running,
                    swappable,
                    messages_processed: 0,
                    messages_failed: 0,
                    avg_process_us: 0,
                    queue_depth: None,
                }
            })
            .collect()
    }

    fn subscribe(&self) -> EventReceiver {
        // Subscribe to the shared event broadcaster
        self.control_state.event_tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NodeType as ConfigNodeType;
    use crate::config::{
        DagConfig, EdgeDefinition, NodeDefinition, OverflowPolicy, PipelineConfig,
    };
    use crate::dag::orchestrator::ControlState;
    use crate::registry::RegistryConfig;
    use petgraph::graph::DiGraph;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    /// Creates a minimal DagOrchestrator for testing without needing actual nodes.
    fn create_test_orchestrator() -> DagOrchestrator {
        let config = DagConfig {
            pipeline: PipelineConfig {
                name: "test-pipeline".to_string(),
                description: Some("Test pipeline".to_string()),
            },
            default_queue_capacity: 1024,
            registry: RegistryConfig::default(),
            dead_letter: None,
            nodes: vec![
                NodeDefinition {
                    id: "source".to_string(),
                    node_type: ConfigNodeType::Source,
                    source_type: Some("stdin".to_string()),
                    sink_type: None,
                    config: toml::Value::Table(toml::map::Map::new()),
                },
                NodeDefinition {
                    id: "transform".to_string(),
                    node_type: ConfigNodeType::Transform,
                    source_type: None,
                    sink_type: None,
                    config: toml::Value::Table({
                        let mut map = toml::map::Map::new();
                        map.insert(
                            "plugin_path".to_string(),
                            toml::Value::String("test.wasm".to_string()),
                        );
                        map
                    }),
                },
                NodeDefinition {
                    id: "sink".to_string(),
                    node_type: ConfigNodeType::Sink,
                    source_type: None,
                    sink_type: Some("stdout".to_string()),
                    config: toml::Value::Table(toml::map::Map::new()),
                },
            ],
            edges: vec![
                EdgeDefinition {
                    from: "source".to_string(),
                    to: "transform".to_string(),
                    from_port: None,
                    to_port: None,
                    queue_capacity: None,
                    overflow: OverflowPolicy::default(),
                },
                EdgeDefinition {
                    from: "transform".to_string(),
                    to: "sink".to_string(),
                    from_port: None,
                    to_port: None,
                    queue_capacity: None,
                    overflow: OverflowPolicy::default(),
                },
            ],
        };

        // Build minimal graph
        let mut graph = DiGraph::new();
        let mut node_indices = HashMap::new();

        for node in &config.nodes {
            let idx = graph.add_node(node.id.clone());
            node_indices.insert(node.id.clone(), idx);
        }

        for edge in &config.edges {
            if let (Some(&from_idx), Some(&to_idx)) =
                (node_indices.get(&edge.from), node_indices.get(&edge.to))
            {
                graph.add_edge(from_idx, to_idx, ());
            }
        }

        let topo_order = vec![
            "source".to_string(),
            "transform".to_string(),
            "sink".to_string(),
        ];
        let control_state = Arc::new(ControlState::new("test-pipeline".to_string()));

        DagOrchestrator {
            graph,
            node_indices,
            config,
            topo_order,
            config_path: None,
            nodes: Mutex::new(HashMap::new()),
            run_state: Mutex::new(None),
            cancel_token: CancellationToken::new(),
            control_state,
            factory_ctx: Mutex::new(None),
            swap_locks: Mutex::new(HashMap::new()),
        }
    }

    #[test]
    fn test_status_returns_pipeline_info() {
        let orchestrator = create_test_orchestrator();
        let status = orchestrator.status();

        assert_eq!(status.name, "test-pipeline");
        assert_eq!(status.node_count, 3);
        assert!(!status.swap_in_progress);
    }

    #[test]
    fn test_status_shows_draining_when_cancelled() {
        let orchestrator = create_test_orchestrator();

        // Initially not draining
        let status = orchestrator.status();
        assert_ne!(status.state, PipelineState::Draining);

        // After cancellation, should show draining
        orchestrator.cancel_token.cancel();
        let status = orchestrator.status();
        assert_eq!(status.state, PipelineState::Draining);
    }

    #[test]
    fn test_nodes_returns_all_nodes() {
        let orchestrator = create_test_orchestrator();
        let nodes = orchestrator.nodes();

        assert_eq!(nodes.len(), 3);

        let source = nodes.iter().find(|n| n.id == "source").unwrap();
        assert_eq!(source.node_type, NodeType::Source);
        assert!(!source.swappable);

        let transform = nodes.iter().find(|n| n.id == "transform").unwrap();
        assert_eq!(transform.node_type, NodeType::Transform);
        assert!(transform.swappable);

        let sink = nodes.iter().find(|n| n.id == "sink").unwrap();
        assert_eq!(sink.node_type, NodeType::Sink);
        assert!(!sink.swappable);
    }

    #[test]
    fn test_metrics_returns_snapshot() {
        let orchestrator = create_test_orchestrator();
        let metrics = orchestrator.metrics();

        // With http-api feature, MetricsRegistry returns populated metrics
        // Without http-api feature, MetricsSnapshot::default() has empty collections
        #[cfg(feature = "http-api")]
        {
            // Should have pipeline uptime metric at minimum
            let prometheus_output = metrics.to_prometheus();
            assert!(prometheus_output.contains("wafer_pipeline_uptime_seconds"));
        }
        #[cfg(not(feature = "http-api"))]
        {
            assert!(metrics.counters.is_empty());
            assert!(metrics.gauges.is_empty());
        }
    }

    #[test]
    fn test_subscribe_returns_receiver() {
        let orchestrator = create_test_orchestrator();
        let _receiver = orchestrator.subscribe();
        // Just verify we can subscribe without panic
    }

    #[tokio::test]
    async fn test_hot_swap_node_not_found() {
        let orchestrator = create_test_orchestrator();
        // Use UFCS to call the trait method, not the inherent method
        let result = PipelineControl::hot_swap(&orchestrator, "nonexistent").await;

        assert!(matches!(
            result,
            Err(ControlError::NodeNotFound { node_id }) if node_id == "nonexistent"
        ));
    }

    #[tokio::test]
    async fn test_hot_swap_not_swappable_for_source() {
        let orchestrator = create_test_orchestrator();
        // Use UFCS to call the trait method, not the inherent method
        let result = PipelineControl::hot_swap(&orchestrator, "source").await;

        assert!(matches!(
            result,
            Err(ControlError::NotSwappable { node_id }) if node_id == "source"
        ));
    }

    #[tokio::test]
    async fn test_hot_swap_not_swappable_for_sink() {
        let orchestrator = create_test_orchestrator();
        // Use UFCS to call the trait method, not the inherent method
        let result = PipelineControl::hot_swap(&orchestrator, "sink").await;

        assert!(matches!(
            result,
            Err(ControlError::NotSwappable { node_id }) if node_id == "sink"
        ));
    }

    #[tokio::test]
    async fn test_hot_swap_requires_config_path() {
        let orchestrator = create_test_orchestrator();
        // Use UFCS to call the trait method, not the inherent method
        let result = PipelineControl::hot_swap(&orchestrator, "transform").await;

        // Transform is swappable but no config_path set, so returns ConfigError
        assert!(matches!(
            result,
            Err(ControlError::ConfigError { message }) if message.contains("no config path")
        ));
    }

    #[tokio::test]
    async fn test_reload_config_requires_config_path() {
        let orchestrator = create_test_orchestrator();
        let result = orchestrator.reload_config().await;

        // No config_path set, so returns ConfigError
        assert!(matches!(
            result,
            Err(ControlError::ConfigError { message }) if message.contains("no config path")
        ));
    }

    #[tokio::test]
    async fn test_drain_cancels_token() {
        let orchestrator = create_test_orchestrator();
        assert!(!orchestrator.cancel_token.is_cancelled());

        let result = orchestrator.drain().await;
        assert!(result.is_ok());
        assert!(orchestrator.cancel_token.is_cancelled());
    }

    #[tokio::test]
    async fn test_shutdown_cancels_token() {
        let orchestrator = create_test_orchestrator();
        assert!(!orchestrator.cancel_token.is_cancelled());

        // Use explicit trait call to call PipelineControl::shutdown, not the inherent method
        let result = PipelineControl::shutdown(&orchestrator).await;
        assert!(result.is_ok());
        assert!(orchestrator.cancel_token.is_cancelled());
    }

    #[tokio::test]
    async fn test_drain_is_idempotent() {
        let orchestrator = create_test_orchestrator();

        // First drain
        let result1 = orchestrator.drain().await;
        assert!(result1.is_ok());

        // Second drain should also succeed
        let result2 = orchestrator.drain().await;
        assert!(result2.is_ok());
    }

    #[test]
    fn test_uptime_increases() {
        let orchestrator = create_test_orchestrator();
        let status1 = orchestrator.status();

        // Sleep briefly to let uptime increase
        std::thread::sleep(std::time::Duration::from_millis(10));

        let status2 = orchestrator.status();
        // uptime_secs is in seconds, so may not increase in 10ms
        // but it should at least not decrease
        assert!(status2.uptime_secs >= status1.uptime_secs);
    }

    #[test]
    fn test_arc_pipeline_control_delegation() {
        // Test that Arc<DagOrchestrator> also implements PipelineControl
        let orchestrator = Arc::new(create_test_orchestrator());

        // These should compile and work via the Arc impl
        let status = orchestrator.status();
        assert_eq!(status.name, "test-pipeline");

        let nodes = orchestrator.nodes();
        assert_eq!(nodes.len(), 3);

        let _receiver = orchestrator.subscribe();
    }
}
