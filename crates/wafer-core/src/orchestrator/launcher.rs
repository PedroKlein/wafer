//! Pipeline launcher — single entry point from Config to running orchestrator.
//!
//! Absorbs all startup orchestration: engine creation, plugin resolution,
//! Wasm compilation, source/sink construction, and topology wiring.
//! The result is a fully-running `PipelineOrchestrator` with all tasks spawned.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use wasmtime::Store;

use crate::config::{Config, NodeConfig as PluginNodeConfig, NodeDefinition, NodeType};
use crate::engine::{Capabilities, WaferEngine, WaferState};
use crate::error::{ConfigError, Result, WaferError};
use crate::node::wasm::{WasmFilterNode, WasmRouterNode, WasmTransformNode};
use crate::node::{
    BenchSink, BenchSinkConfig, BenchSource, BenchSourceConfig, FileSink, FileSource, HttpSink,
    HttpSource, MqttSink, MqttSource, Sink, Source, StdinSource,
    StdoutSink,
};
use crate::orchestrator::builder::{build_pipeline_with_io, NodeBundleKind};
use crate::orchestrator::pipeline::PipelineOrchestrator;
use crate::registry::{PluginSource, WaferRegistry};

/// Launch a fully-wired pipeline from configuration.
///
/// Performs the complete startup sequence:
/// 1. Creates `WaferEngine` with OS-thread epoch ticker
/// 2. Resolves and compiles all Wasm plugins via `WaferRegistry`
/// 3. Creates Source/Sink instances by dispatching on `source_type`/`sink_type`
/// 4. Creates NativeTransform/NativeFilter for native nodes
/// 5. Builds pipeline topology via `build_pipeline_with_io()`
/// 6. Injects compiled Wasm node instances into `BuildOutput` bundles
/// 7. Returns a running `PipelineOrchestrator`
///
/// # Errors
///
/// Returns error if engine creation, plugin loading, source/sink creation,
/// or pipeline building fails.
pub async fn launch_pipeline(
    config: Config,
    config_path: Option<&Path>,
) -> Result<PipelineOrchestrator> {
    // 1. Engine
    let engine = WaferEngine::from_engine_config(&config.engine)?;
    engine.ensure_epoch_ticker();
    let engine = Arc::new(engine);

    // 2. Registry for OCI resolution
    let registry = WaferRegistry::new(config.registry.clone()).map_err(WaferError::Registry)?;

    // 3. Create sources and sinks
    let mut sources: HashMap<String, Box<dyn Source + Send>> = HashMap::new();
    let mut sinks: HashMap<String, Box<dyn Sink + Send>> = HashMap::new();

    for node_def in &config.nodes {
        match node_def.node_type {
            NodeType::Source => {
                let source = create_source(node_def)?;
                sources.insert(node_def.id.clone(), source);
            }
            NodeType::Sink => {
                let sink = create_sink(node_def)?;
                sinks.insert(node_def.id.clone(), sink);
            }
            _ => {}
        }
    }

    // 4. Build pipeline topology (queues, control infra, bundles with None wasm slots)
    let mut build_output = build_pipeline_with_io(&config, sources, sinks)?;

    // 5. Load and inject Wasm/Native nodes into bundles
    for bundle in &mut build_output.node_bundles {
        let Some(node_def) = config.nodes.iter().find(|n| n.id.as_str() == &*bundle.node_id)
        else {
            continue;
        };

        match &mut bundle.kind {
            NodeBundleKind::Transform { node, .. } => {
                if let Some(instance) = load_transform_node(
                    node_def, &engine, &registry, config_path,
                ).await? {
                    *node = Some(instance);
                }
            }
            NodeBundleKind::Filter { node, .. } => {
                if let Some(instance) = load_filter_node(
                    node_def, &engine, &registry, config_path,
                ).await? {
                    *node = Some(instance);
                }
            }
            NodeBundleKind::Router { node, .. } => {
                if let Some(instance) = load_router_node(
                    node_def, &engine, &registry, config_path,
                ).await? {
                    *node = Some(instance);
                }
            }
            NodeBundleKind::Source { .. } | NodeBundleKind::Sink { .. } => {}
        }
    }

    // 6. Spawn orchestrator
    let orchestrator = PipelineOrchestrator::from_build_output(build_output, config, engine);
    Ok(orchestrator)
}

// =============================================================================
// Source Factory
// =============================================================================

fn create_source(node_def: &NodeDefinition) -> Result<Box<dyn Source + Send>> {
    let source_type = node_def.source_type.as_deref().unwrap_or("file");

    match source_type {
        "bench-source" | "bench" => {
            let rate = get_f64(&node_def.config, &["rate", "rate_per_sec"]).unwrap_or(1000.0);
            let total_messages =
                get_u64(&node_def.config, "total_messages").unwrap_or(60_000);
            let warmup_messages =
                get_u64(&node_def.config, "warmup_messages").unwrap_or(0);
            let payload_size =
                get_usize(&node_def.config, "payload_size").unwrap_or(128);

            let bench_config = BenchSourceConfig::new(rate, total_messages)
                .with_warmup(warmup_messages)
                .with_payload_size(payload_size);

            Ok(Box::new(BenchSource::new(bench_config).with_id(&node_def.id)))
        }
        "stdin" => Ok(Box::new(StdinSource::new(&node_def.id))),
        "mqtt" => {
            let broker = get_str(&node_def.config, "broker").unwrap_or("localhost");
            let port = get_u16(&node_def.config, "port").unwrap_or(1883);
            let topic = get_str(&node_def.config, "topic").ok_or_else(|| {
                config_err(&node_def.id, "mqtt source requires 'topic' in config")
            })?;
            let qos = get_u8(&node_def.config, "qos").unwrap_or(0);
            let client_id = get_str(&node_def.config, "client_id")
                .map(String::from)
                .unwrap_or_else(|| format!("wafer-{}", node_def.id));

            Ok(Box::new(MqttSource::new(
                &node_def.id, broker, port, topic, qos, client_id,
            )))
        }
        "http" => {
            let bind = get_str(&node_def.config, "bind").unwrap_or("127.0.0.1:8081");
            let path = get_str(&node_def.config, "path").unwrap_or("/ingest");
            let buffer_size = get_usize(&node_def.config, "buffer_size").unwrap_or(1000);

            Ok(Box::new(
                HttpSource::new(&node_def.id, bind, path).with_buffer_size(buffer_size),
            ))
        }
        _ => {
            let path = get_str(&node_def.config, "path").ok_or_else(|| {
                config_err(&node_def.id, "file source requires 'path' in config")
            })?;
            Ok(Box::new(FileSource::new(&node_def.id, path)))
        }
    }
}

// =============================================================================
// Sink Factory
// =============================================================================

fn create_sink(node_def: &NodeDefinition) -> Result<Box<dyn Sink + Send>> {
    use crate::node::HttpSinkBatchConfig;
    use std::time::Duration;

    let sink_type = node_def.sink_type.as_deref().unwrap_or("file");

    match sink_type {
        "bench-sink" | "bench" => {
            let warmup_secs = get_u64(&node_def.config, "warmup_secs").unwrap_or(30);
            let track_sequences =
                get_bool(&node_def.config, "track_sequences").unwrap_or(true);
            let track_hotswap =
                get_bool(&node_def.config, "track_hotswap").unwrap_or(false);
            let output_dir = get_str(&node_def.config, "output_dir").map(PathBuf::from);

            let bench_config = BenchSinkConfig {
                warmup_secs,
                track_sequences,
                track_hotswap,
                output_dir,
            };

            Ok(Box::new(BenchSink::new(bench_config).with_id(&node_def.id)))
        }
        "stdout" => Ok(Box::new(StdoutSink::new(&node_def.id))),
        "mqtt" => {
            let broker = get_str(&node_def.config, "broker").unwrap_or("localhost");
            let port = get_u16(&node_def.config, "port").unwrap_or(1883);
            let topic = get_str(&node_def.config, "topic").ok_or_else(|| {
                config_err(&node_def.id, "mqtt sink requires 'topic' in config")
            })?;
            let qos = get_u8(&node_def.config, "qos").unwrap_or(0);
            let client_id = get_str(&node_def.config, "client_id")
                .map(String::from)
                .unwrap_or_else(|| format!("wafer-{}", node_def.id));

            Ok(Box::new(MqttSink::new(
                &node_def.id, broker, port, topic, qos, client_id,
            )))
        }
        "http" => {
            let url = get_str(&node_def.config, "url").ok_or_else(|| {
                config_err(&node_def.id, "http sink requires 'url' in config")
            })?;
            let batch_size = get_usize(&node_def.config, "batch_size");
            let batch_timeout_ms = get_u64(&node_def.config, "batch_timeout_ms");
            let timeout_secs = get_u64(&node_def.config, "timeout_secs").unwrap_or(30);

            let batch_config = HttpSinkBatchConfig {
                batch_size,
                batch_timeout_ms,
            };

            let sink = HttpSink::with_batching(&node_def.id, url, batch_config)
                .with_timeout(Duration::from_secs(timeout_secs));

            Ok(Box::new(sink))
        }
        _ => {
            let path = get_str(&node_def.config, "path").ok_or_else(|| {
                config_err(&node_def.id, "file sink requires 'path' in config")
            })?;
            Ok(Box::new(FileSink::new(&node_def.id, path)))
        }
    }
}

// =============================================================================
// Wasm Node Loading
// =============================================================================

/// Returns `None` for native nodes (handled by the runner as NativeTransform),
/// `Some(WasmTransformNode)` for Wasm nodes.
async fn load_transform_node(
    node_def: &NodeDefinition,
    engine: &Arc<WaferEngine>,
    registry: &WaferRegistry,
    config_path: Option<&Path>,
) -> Result<Option<WasmTransformNode>> {
    if is_native_node(node_def) {
        // Native nodes don't need Wasm — the runner handles them differently.
        // For now, we return None and the runner awaits cancel (same as test path).
        // TODO: Native transform support in the new runner requires a different bundle variant.
        return Ok(None);
    }

    let component = resolve_and_load_component(node_def, engine, registry, config_path).await?;
    let pre = engine.pre_instantiate_transform(&component)?;
    let pre = Arc::new(pre);

    let state = WaferState::new(&*node_def.id, Capabilities::sandbox());
    let mut store = Store::new(engine.inner(), state);
    store.limiter(|s| s.limits_mut());
    store.epoch_deadline_trap();
    store.set_epoch_deadline(engine.epoch_deadline());

    let bindings = pre.instantiate(&mut store).map_err(|e| WaferError::PluginInit {
        message: format!("transform '{}' instantiation failed: {e}", node_def.id),
    })?;

    Ok(Some(WasmTransformNode::new(store, bindings, pre, engine.fuel_limit())))
}

async fn load_filter_node(
    node_def: &NodeDefinition,
    engine: &Arc<WaferEngine>,
    registry: &WaferRegistry,
    config_path: Option<&Path>,
) -> Result<Option<WasmFilterNode>> {
    if is_native_node(node_def) {
        return Ok(None);
    }

    let component = resolve_and_load_component(node_def, engine, registry, config_path).await?;
    let pre = engine.pre_instantiate_filter(&component)?;
    let pre = Arc::new(pre);

    let state = WaferState::new(&*node_def.id, Capabilities::sandbox());
    let mut store = Store::new(engine.inner(), state);
    store.limiter(|s| s.limits_mut());
    store.epoch_deadline_trap();
    store.set_epoch_deadline(engine.epoch_deadline());

    let bindings = pre.instantiate(&mut store).map_err(|e| WaferError::PluginInit {
        message: format!("filter '{}' instantiation failed: {e}", node_def.id),
    })?;

    Ok(Some(WasmFilterNode::new(store, bindings, pre, engine.fuel_limit())))
}

async fn load_router_node(
    node_def: &NodeDefinition,
    engine: &Arc<WaferEngine>,
    registry: &WaferRegistry,
    config_path: Option<&Path>,
) -> Result<Option<WasmRouterNode>> {
    if is_native_node(node_def) {
        return Ok(None);
    }

    let component = resolve_and_load_component(node_def, engine, registry, config_path).await?;
    let pre = engine.pre_instantiate_router(&component)?;
    let pre = Arc::new(pre);

    let state = WaferState::new(&*node_def.id, Capabilities::sandbox());
    let mut store = Store::new(engine.inner(), state);
    store.limiter(|s| s.limits_mut());
    store.epoch_deadline_trap();
    store.set_epoch_deadline(engine.epoch_deadline());

    let bindings = pre.instantiate(&mut store).map_err(|e| WaferError::PluginInit {
        message: format!("router '{}' instantiation failed: {e}", node_def.id),
    })?;

    Ok(Some(WasmRouterNode::new(store, bindings, pre, engine.fuel_limit())))
}

/// Resolve plugin source from node config and load the Wasm component.
///
/// Handles both local paths (relative to config file dir) and OCI references.
async fn resolve_and_load_component(
    node_def: &NodeDefinition,
    engine: &WaferEngine,
    registry: &WaferRegistry,
    config_path: Option<&Path>,
) -> Result<wasmtime::component::Component> {
    let plugin_config: PluginNodeConfig =
        node_def.config.clone().try_into().map_err(|e: toml::de::Error| {
            WaferError::Config(ConfigError::Message(format!(
                "failed to parse plugin config for '{}': {e}",
                node_def.id
            )))
        })?;

    let mut source = plugin_config.plugin_source().map_err(WaferError::Config)?;

    // Resolve relative local paths against config file directory
    if let PluginSource::Local(ref path) = source {
        if path.is_relative() {
            if let Some(config_dir) = config_path.and_then(Path::parent) {
                source = PluginSource::Local(config_dir.join(path));
            }
        }
    }

    let resolved = registry.resolve(&source).await.map_err(WaferError::Registry)?;

    match &resolved.source {
        PluginSource::Local(path) => {
            tracing::debug!(node = %node_def.id, path = %path.display(), "loading local plugin");
            engine.load_component(path)
        }
        PluginSource::Oci(oci_ref) => {
            tracing::info!(
                node = %node_def.id,
                reference = %oci_ref,
                cache_path = %resolved.wasm_path.display(),
                "loading OCI plugin"
            );
            let bytes = std::fs::read(&resolved.wasm_path).map_err(|e| {
                WaferError::Config(ConfigError::Message(format!(
                    "failed to read cached plugin for '{}': {e}",
                    node_def.id
                )))
            })?;
            engine.load_component_from_bytes(&bytes, oci_ref.as_str())
        }
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Detect native nodes: `native` key present, or `function` key without plugin_path/oci.
fn is_native_node(node_def: &NodeDefinition) -> bool {
    node_def.config.get("native").is_some()
        || (node_def.config.get("function").is_some()
            && node_def.config.get("plugin_path").is_none()
            && node_def.config.get("oci").is_none())
}

fn config_err(node_id: &str, msg: &str) -> WaferError {
    WaferError::Config(ConfigError::Message(format!("node '{node_id}': {msg}")))
}

/// Extract a string value from a TOML table, checking a single key.
fn get_str<'a>(config: &'a toml::Value, key: &str) -> Option<&'a str> {
    config.get(key).and_then(toml::Value::as_str)
}

/// Extract a float from a TOML table, trying multiple keys in order.
/// Accepts both float and integer TOML values.
fn get_f64(config: &toml::Value, keys: &[&str]) -> Option<f64> {
    for key in keys {
        if let Some(val) = config.get(*key) {
            if let Some(f) = val.as_float() {
                return Some(f);
            }
            if let Some(i) = val.as_integer() {
                return Some(i as f64);
            }
        }
    }
    None
}

fn get_u64(config: &toml::Value, key: &str) -> Option<u64> {
    config.get(key).and_then(toml::Value::as_integer).map(|v| v as u64)
}

fn get_u16(config: &toml::Value, key: &str) -> Option<u16> {
    config.get(key).and_then(toml::Value::as_integer).map(|v| v as u16)
}

fn get_u8(config: &toml::Value, key: &str) -> Option<u8> {
    config.get(key).and_then(toml::Value::as_integer).map(|v| v as u8)
}

fn get_usize(config: &toml::Value, key: &str) -> Option<usize> {
    config.get(key).and_then(toml::Value::as_integer).map(|v| v as usize)
}

fn get_bool(config: &toml::Value, key: &str) -> Option<bool> {
    config.get(key).and_then(toml::Value::as_bool)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        ApiServerConfig, Config, EdgeDefinition, MetricsConfig, NodeDefinition, NodeType,
        OverflowPolicy, PipelineConfig,
    };

    fn native_passthrough_config() -> Config {
        Config {
            pipeline: PipelineConfig::default(),
            engine: crate::config::EngineConfig::default(),
            api: ApiServerConfig::default(),
            metrics: MetricsConfig::default(),
            nodes: vec![
                NodeDefinition {
                    id: "src".to_string(),
                    node_type: NodeType::Source,
                    source_type: Some("bench-source".to_string()),
                    sink_type: None,
                    config: toml::toml! {
                        rate = 1000
                        total_messages = 10
                        warmup_messages = 0
                        payload_size = 64
                    }
                    .into(),
                    capabilities: Default::default(),
                },
                NodeDefinition {
                    id: "t1".to_string(),
                    node_type: NodeType::Transform,
                    source_type: None,
                    sink_type: None,
                    config: toml::toml! {
                        function = "passthrough"
                    }
                    .into(),
                    capabilities: Default::default(),
                },
                NodeDefinition {
                    id: "sink".to_string(),
                    node_type: NodeType::Sink,
                    source_type: None,
                    sink_type: Some("bench-sink".to_string()),
                    config: toml::toml! {
                        warmup_secs = 0
                        track_sequences = true
                    }
                    .into(),
                    capabilities: Default::default(),
                },
            ],
            edges: vec![
                EdgeDefinition {
                    from: "src".to_string(),
                    to: "t1".to_string(),
                    from_port: None,
                    to_port: None,
                    queue_capacity: None,
                    overflow: OverflowPolicy::default(),
                },
                EdgeDefinition {
                    from: "t1".to_string(),
                    to: "sink".to_string(),
                    from_port: None,
                    to_port: None,
                    queue_capacity: None,
                    overflow: OverflowPolicy::default(),
                },
            ],
            default_queue_capacity: 1024,
            registry: Default::default(),
            dead_letter: None,
        }
    }

    #[test]
    fn is_native_detects_function_key() {
        let node_def = NodeDefinition {
            id: "t1".to_string(),
            node_type: NodeType::Transform,
            source_type: None,
            sink_type: None,
            config: toml::toml! { function = "passthrough" }.into(),
            capabilities: Default::default(),
        };
        assert!(is_native_node(&node_def));
    }

    #[test]
    fn is_native_detects_native_key() {
        let node_def = NodeDefinition {
            id: "t1".to_string(),
            node_type: NodeType::Transform,
            source_type: None,
            sink_type: None,
            config: toml::toml! { native = "transform" }.into(),
            capabilities: Default::default(),
        };
        assert!(is_native_node(&node_def));
    }

    #[test]
    fn is_native_false_for_wasm_node() {
        let node_def = NodeDefinition {
            id: "t1".to_string(),
            node_type: NodeType::Transform,
            source_type: None,
            sink_type: None,
            config: toml::toml! { plugin_path = "./plugins/pass-through.wasm" }.into(),
            capabilities: Default::default(),
        };
        assert!(!is_native_node(&node_def));
    }

    #[test]
    fn create_source_bench() {
        let node_def = NodeDefinition {
            id: "src".to_string(),
            node_type: NodeType::Source,
            source_type: Some("bench-source".to_string()),
            sink_type: None,
            config: toml::toml! {
                rate = 500
                total_messages = 100
                payload_size = 256
            }
            .into(),
            capabilities: Default::default(),
        };
        let source = create_source(&node_def);
        assert!(source.is_ok());
    }

    #[test]
    fn create_source_stdin() {
        let node_def = NodeDefinition {
            id: "src".to_string(),
            node_type: NodeType::Source,
            source_type: Some("stdin".to_string()),
            sink_type: None,
            config: toml::Value::Table(toml::map::Map::new()),
            capabilities: Default::default(),
        };
        assert!(create_source(&node_def).is_ok());
    }

    #[test]
    fn create_source_file_missing_path_errors() {
        let node_def = NodeDefinition {
            id: "src".to_string(),
            node_type: NodeType::Source,
            source_type: Some("file".to_string()),
            sink_type: None,
            config: toml::Value::Table(toml::map::Map::new()),
            capabilities: Default::default(),
        };
        let result = create_source(&node_def);
        assert!(result.is_err());
    }

    #[test]
    fn create_sink_bench() {
        let node_def = NodeDefinition {
            id: "sink".to_string(),
            node_type: NodeType::Sink,
            source_type: None,
            sink_type: Some("bench-sink".to_string()),
            config: toml::toml! {
                warmup_secs = 10
                track_sequences = true
            }
            .into(),
            capabilities: Default::default(),
        };
        assert!(create_sink(&node_def).is_ok());
    }

    #[test]
    fn create_sink_stdout() {
        let node_def = NodeDefinition {
            id: "sink".to_string(),
            node_type: NodeType::Sink,
            source_type: None,
            sink_type: Some("stdout".to_string()),
            config: toml::Value::Table(toml::map::Map::new()),
            capabilities: Default::default(),
        };
        assert!(create_sink(&node_def).is_ok());
    }

    #[test]
    fn create_sink_file_missing_path_errors() {
        let node_def = NodeDefinition {
            id: "sink".to_string(),
            node_type: NodeType::Sink,
            source_type: None,
            sink_type: Some("file".to_string()),
            config: toml::Value::Table(toml::map::Map::new()),
            capabilities: Default::default(),
        };
        assert!(create_sink(&node_def).is_err());
    }

    #[tokio::test]
    async fn launch_pipeline_native_passthrough() {
        // Native nodes produce None for Wasm slot — tasks await cancel.
        // This verifies the launcher doesn't crash on native configs.
        let config = native_passthrough_config();
        let result = launch_pipeline(config, None).await;
        assert!(result.is_ok(), "launch_pipeline failed: {:?}", result.err());

        let mut orch = result.unwrap();
        assert!(orch.is_running());
        assert_eq!(orch.task_count(), 3);

        orch.shutdown().await.unwrap();
        assert!(!orch.is_running());
    }
}
