//! Builder and validation methods for [`PipelineOrchestrator`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::assembler::{create_dlq_sink, create_node, NodeAssembler};
use crate::config::{Config, DagConfig};
use crate::engine::WaferEngine;
use crate::error::{ConfigError, Result, WaferError};
use crate::node::AnyNode;
use crate::queue::{BoundedQueue, RuntimeEnvelope};

use super::pipeline::{ControlState, PipelineOrchestrator, RunState};
use crate::dag::graph::DagGraph;

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

            // Graceful close
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

    #[must_use = "creating an orchestrator without using it is likely a bug"]
    pub fn from_dag_config(config: DagConfig) -> Result<Self> {
        // Convert DagConfig to a full Config with defaults for the extra fields.
        let full_config = Config {
            pipeline: config.pipeline,
            engine: crate::config::EngineConfig::default(),
            api: crate::config::ApiServerConfig::default(),
            metrics: crate::config::MetricsConfig::default(),
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

        // Register node with metrics registry
        #[cfg(feature = "http-api")]
        {
            self.control_state.metrics_registry.register_node(id, node.to_string());
        }

        let mut nodes = self.nodes.lock().await;
        nodes.insert(id.to_string(), Arc::new(Mutex::new(node)));
        Ok(())
    }

    /// Create bounded queues for each edge. Must be called before `run()`.
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

            // Register queue with metrics registry
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EdgeDefinition, NodeDefinition, NodeType, OverflowPolicy, PipelineConfig};
    use crate::node::FileSource;

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
        assert!(run_state
            .queue_senders
            .contains_key(&("source:default".to_string(), "transform:default".to_string())));
        assert!(run_state
            .queue_senders
            .contains_key(&("transform:default".to_string(), "sink:default".to_string())));
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
            assert!(dlq_sender.is_some(), "DLQ sender should be set after initialization");
        }

        // Clean up
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
        orchestrator
            .initialize_dlq(&dlq_config)
            .await
            .expect("Failed to initialize DLQ with stdout sink");

        {
            let dlq_sender = orchestrator.control_state.dlq_sender.lock().await;
            assert!(dlq_sender.is_some(), "DLQ sender should be set after initialization");
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
        assert!(result.is_err(), "Should fail with unknown sink type");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unknown") && err.contains("sink"),
            "Error should mention unknown sink type: {err}"
        );
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
        assert!(result.is_err(), "Should fail when file sink has no path");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("path"), "Error should mention missing path: {err}");
    }
}
