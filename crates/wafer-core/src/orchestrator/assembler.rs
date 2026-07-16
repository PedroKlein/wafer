//! Node factory for creating pipeline nodes from configuration.
// TODO: improve this file
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::Result;
use crate::config::{DeadLetterConfig, NodeConfig as PluginNodeConfig, NodeDefinition, NodeType};
use crate::engine::{TransformInstance, WaferEngine};
use crate::error::{ConfigError, WaferError};
use crate::node::{
    AnyNode, BenchSink, BenchSinkConfig, BenchSource, BenchSourceConfig, FileSink, FileSource,
    HttpSink, HttpSource, MqttSink, MqttSource, NativeTransform, NodeConfig,
    RouterInstance, Sink, StdinSource, StdoutSink, WasmRouter, WasmTransform,
};
use crate::registry::{PluginSource, RegistryConfig, ResolvedPlugin, WaferRegistry};

pub struct NodeAssembler {
    engine: Arc<WaferEngine>,
    registry: WaferRegistry,
    resolved_plugins: HashMap<String, ResolvedPlugin>,
}

impl NodeAssembler {
    pub fn new(engine: Arc<WaferEngine>, registry_config: RegistryConfig) -> Result<Self> {
        let registry = WaferRegistry::new(registry_config).map_err(WaferError::Registry)?;
        Ok(Self { engine, registry, resolved_plugins: HashMap::new() })
    }

    #[must_use]
    pub fn resolved_plugins(&self) -> &HashMap<String, ResolvedPlugin> {
        &self.resolved_plugins
    }

    #[must_use]
    pub fn get_resolved_plugin(&self, node_id: &str) -> Option<&ResolvedPlugin> {
        self.resolved_plugins.get(node_id)
    }

    pub fn engine(&self) -> &Arc<WaferEngine> {
        &self.engine
    }
}

pub async fn create_node(node_def: &NodeDefinition, ctx: &mut NodeAssembler) -> Result<AnyNode> {
    match node_def.node_type {
        NodeType::Source => create_source(node_def),
        NodeType::Transform => create_transform(node_def, ctx).await,
        NodeType::Sink => create_sink(node_def),
        NodeType::Router => create_router(node_def, ctx).await,
        NodeType::Filter => create_transform(node_def, ctx).await,
        NodeType::Joiner => Err(WaferError::Runtime("joiner removed".into())),
    }
}

fn create_source(node_def: &NodeDefinition) -> Result<AnyNode> {
    let source_type = node_def.source_type.as_deref().unwrap_or("file");
    match source_type {
        "bench-source" | "bench" => {
            let rate = node_def
                .config
                .get("rate")
                .or_else(|| node_def.config.get("rate_per_sec"))
                .and_then(toml::Value::as_float)
                .or_else(|| {
                    node_def
                        .config
                        .get("rate")
                        .or_else(|| node_def.config.get("rate_per_sec"))
                        .and_then(toml::Value::as_integer)
                        .map(|v| v as f64)
                })
                .unwrap_or(1000.0);
            let total_messages = node_def
                .config
                .get("total_messages")
                .and_then(toml::Value::as_integer)
                .map_or(60_000, |v| v as u64);
            let warmup_messages = node_def
                .config
                .get("warmup_messages")
                .and_then(toml::Value::as_integer)
                .map_or(0, |v| v as u64);
            let payload_size = node_def
                .config
                .get("payload_size")
                .and_then(toml::Value::as_integer)
                .map_or(128, |v| v as usize);

            let config = BenchSourceConfig::new(rate, total_messages)
                .with_warmup(warmup_messages)
                .with_payload_size(payload_size);

            Ok(AnyNode::from_source(
                BenchSource::new(config).with_id(&node_def.id),
            ))
        }
        "stdin" => Ok(AnyNode::from_source(StdinSource::new(&node_def.id))),
        "mqtt" => {
            let broker =
                node_def.config.get("broker").and_then(toml::Value::as_str).unwrap_or("localhost");
            let port = node_def
                .config
                .get("port")
                .and_then(toml::Value::as_integer)
                .map_or(1883, |v| v as u16);
            let topic =
                node_def.config.get("topic").and_then(toml::Value::as_str).ok_or_else(|| {
                    WaferError::Config(ConfigError::Message(format!(
                        "mqtt source '{}' requires 'topic' in config",
                        node_def.id
                    )))
                })?;
            let qos =
                node_def.config.get("qos").and_then(toml::Value::as_integer).map_or(0, |v| v as u8);
            let client_id = node_def
                .config
                .get("client_id")
                .and_then(toml::Value::as_str)
                .map_or_else(|| format!("wafer-{}", node_def.id), String::from);
            Ok(AnyNode::from_source(MqttSource::new(
                &node_def.id,
                broker,
                port,
                topic,
                qos,
                client_id,
            )))
        }
        "http" => {
            let bind = node_def
                .config
                .get("bind")
                .and_then(toml::Value::as_str)
                .unwrap_or("127.0.0.1:8081");
            let path =
                node_def.config.get("path").and_then(toml::Value::as_str).unwrap_or("/ingest");
            let buffer_size = node_def
                .config
                .get("buffer_size")
                .and_then(toml::Value::as_integer)
                .map_or(1000, |v| v as usize);
            Ok(AnyNode::from_source(
                HttpSource::new(&node_def.id, bind, path).with_buffer_size(buffer_size),
            ))
        }
        _ => {
            let path =
                node_def.config.get("path").and_then(toml::Value::as_str).ok_or_else(|| {
                    WaferError::Config(ConfigError::Message(format!(
                        "source '{}' requires 'path' in config",
                        node_def.id
                    )))
                })?;
            Ok(AnyNode::from_source(FileSource::new(&node_def.id, path)))
        }
    }
}

async fn create_transform(node_def: &NodeDefinition, ctx: &mut NodeAssembler) -> Result<AnyNode> {
    // Check for native node types that don't need Wasm.
    // Native nodes are identified by a "native" key in config (e.g., native = "transform")
    // or by having a "function" key without a "plugin_path" or "oci" key.
    let is_native = node_def.config.get("native").is_some()
        || (node_def.config.get("function").is_some()
            && node_def.config.get("plugin_path").is_none()
            && node_def.config.get("oci").is_none());

    if is_native {
        return match node_def.node_type {
            NodeType::Filter => create_native_filter(node_def),
            _ => create_native_transform(node_def),
        };
    }

    let plugin_config: PluginNodeConfig =
        node_def.config.clone().try_into().map_err(|e: toml::de::Error| {
            WaferError::Config(ConfigError::Message(format!(
                "failed to parse transform '{}' config: {}",
                node_def.id, e
            )))
        })?;

    let engine = Arc::clone(&ctx.engine);
    let component = resolve_and_load_plugin(&node_def.id, &plugin_config, &engine, ctx).await?;

    let instance = TransformInstance::new(&engine, &component).await?;

    let config_str = toml::to_string(&node_def.config)
        .map_err(|e| WaferError::Config(ConfigError::Message(e.to_string())))?;
    let node_config =
        NodeConfig::new(&node_def.id, "transform").with_config_bytes(config_str.into_bytes());

    let transform = WasmTransform::new(engine, instance, node_config);
    Ok(AnyNode::from_transform(transform))
}

fn create_native_transform(node_def: &NodeDefinition) -> Result<AnyNode> {
    let function = node_def
        .config
        .get("function")
        .and_then(toml::Value::as_str)
        .unwrap_or("passthrough");

    let transform = match function {
        "uppercase" => NativeTransform::uppercase(&node_def.id),
        "passthrough" | _ => NativeTransform::passthrough(&node_def.id),
    };

    Ok(AnyNode::from_transform(transform))
}

fn create_native_filter(node_def: &NodeDefinition) -> Result<AnyNode> {
    let threshold = node_def
        .config
        .get("threshold")
        .and_then(toml::Value::as_float)
        .unwrap_or(50.0);

    // Wrap filter logic as a NativeTransform that returns the payload unchanged
    // if it passes the threshold, or an empty payload to signal drop.
    // The filter runner checks ProcessResult::Emit vs ProcessResult::Error.
    // For the native baseline, we use NativeTransform with a filtering closure.
    let transform = NativeTransform::new(&node_def.id, move |payload: &[u8]| {
        let Ok(s) = std::str::from_utf8(payload) else {
            return Ok(Vec::new()); // Drop: empty payload signals filter-out
        };
        // Extract temperature value
        let key = "\"temperature\"";
        let passes = if let Some(idx) = s.find(key) {
            let after_key = &s[idx + key.len()..];
            if let Some(after_colon) = after_key.trim_start().strip_prefix(':') {
                let value_str = after_colon.trim_start();
                let end = value_str
                    .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
                    .unwrap_or(value_str.len());
                value_str[..end].parse::<f64>().is_ok_and(|t| t > threshold)
            } else {
                false
            }
        } else {
            false
        };

        if passes {
            Ok(payload.to_vec())
        } else {
            Ok(Vec::new()) // Drop
        }
    });

    Ok(AnyNode::from_transform(transform))
}

async fn create_router(node_def: &NodeDefinition, ctx: &mut NodeAssembler) -> Result<AnyNode> {
    let plugin_config: PluginNodeConfig =
        node_def.config.clone().try_into().map_err(|e: toml::de::Error| {
            WaferError::Config(ConfigError::Message(format!(
                "failed to parse router '{}' config: {}",
                node_def.id, e
            )))
        })?;

    let engine = Arc::clone(&ctx.engine);
    let _component = resolve_and_load_plugin(&node_def.id, &plugin_config, &engine, ctx).await?;

    let instance = RouterInstance;

    let config_str = toml::to_string(&node_def.config)
        .map_err(|e| WaferError::Config(ConfigError::Message(e.to_string())))?;
    let node_config =
        NodeConfig::new(&node_def.id, "router").with_config_bytes(config_str.into_bytes());

    let router = WasmRouter::new(engine, instance, node_config);
    Ok(AnyNode::from_router(router))
}


async fn resolve_and_load_plugin(
    node_id: &str,
    plugin_config: &PluginNodeConfig,
    engine: &WaferEngine,
    ctx: &mut NodeAssembler,
) -> Result<wasmtime::component::Component> {
    let source = plugin_config.plugin_source().map_err(WaferError::Config)?;
    let resolved = ctx.registry.resolve(&source).await.map_err(WaferError::Registry)?;

    let component = match &resolved.source {
        PluginSource::Local(path) => {
            tracing::debug!("Loading local plugin for '{}' from {:?}", node_id, path);
            engine.load_component(path)?
        }
        PluginSource::Oci(oci_ref) => {
            tracing::info!(
                "Loading OCI plugin for '{}': {} from {:?}",
                node_id,
                oci_ref,
                resolved.wasm_path
            );
            let bytes = std::fs::read(&resolved.wasm_path).map_err(|e| {
                WaferError::Config(ConfigError::Message(format!(
                    "failed to read cached plugin for '{node_id}': {e}"
                )))
            })?;
            engine.load_component_from_bytes(&bytes, oci_ref.as_str())?
        }
    };

    ctx.resolved_plugins.insert(node_id.to_string(), resolved);
    Ok(component)
}

pub fn create_dlq_sink(config: &DeadLetterConfig) -> Result<Box<dyn Sink + Send>> {
    let sink_type = config.sink_type.as_str();
    match sink_type {
        "stdout" => Ok(Box::new(StdoutSink::new("dlq"))),
        "file" => {
            let path = config.config.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                WaferError::Config(ConfigError::Message(
                    "dead_letter file sink requires 'path' in config".to_string(),
                ))
            })?;
            Ok(Box::new(FileSink::new("dlq", path)))
        }
        "mqtt" => {
            let broker =
                config.config.get("broker").and_then(toml::Value::as_str).unwrap_or("localhost");
            let port = config
                .config
                .get("port")
                .and_then(toml::Value::as_integer)
                .map_or(1883, |v| v as u16);
            let topic =
                config.config.get("topic").and_then(toml::Value::as_str).ok_or_else(|| {
                    WaferError::Config(ConfigError::Message(
                        "dead_letter mqtt sink requires 'topic' in config".to_string(),
                    ))
                })?;
            let qos =
                config.config.get("qos").and_then(toml::Value::as_integer).map_or(0, |v| v as u8);
            let client_id = config
                .config
                .get("client_id")
                .and_then(toml::Value::as_str)
                .map_or_else(|| "wafer-dlq".to_string(), String::from);
            Ok(Box::new(MqttSink::new("dlq", broker, port, topic, qos, client_id)))
        }
        "http" => {
            let url = config.config.get("url").and_then(toml::Value::as_str).ok_or_else(|| {
                WaferError::Config(ConfigError::Message(
                    "dead_letter http sink requires 'url' in config".to_string(),
                ))
            })?;
            Ok(Box::new(HttpSink::new("dlq", url)))
        }
        _ => Err(WaferError::Config(ConfigError::Message(format!(
            "unknown dead_letter sink_type '{sink_type}', expected 'file', 'http', 'mqtt', or 'stdout'"
        )))),
    }
}

fn create_sink(node_def: &NodeDefinition) -> Result<AnyNode> {
    use crate::node::HttpSinkBatchConfig;
    use std::time::Duration;

    let sink_type = node_def.sink_type.as_deref().unwrap_or("file");
    match sink_type {
        "bench-sink" | "bench" => {
            let warmup_secs = node_def
                .config
                .get("warmup_secs")
                .and_then(toml::Value::as_integer)
                .map_or(30, |v| v as u64);
            let track_sequences = node_def
                .config
                .get("track_sequences")
                .and_then(toml::Value::as_bool)
                .unwrap_or(true);
            let track_hotswap = node_def
                .config
                .get("track_hotswap")
                .and_then(toml::Value::as_bool)
                .unwrap_or(false);

            let output_dir = node_def
                .config
                .get("output_dir")
                .and_then(toml::Value::as_str)
                .map(PathBuf::from);

            let config = BenchSinkConfig {
                warmup_secs,
                track_sequences,
                track_hotswap,
                output_dir,
            };

            Ok(AnyNode::from_sink(
                BenchSink::new(config).with_id(&node_def.id),
            ))
        }
        "stdout" => Ok(AnyNode::from_sink(StdoutSink::new(&node_def.id))),
        "mqtt" => {
            let broker =
                node_def.config.get("broker").and_then(toml::Value::as_str).unwrap_or("localhost");
            let port = node_def
                .config
                .get("port")
                .and_then(toml::Value::as_integer)
                .map_or(1883, |v| v as u16);
            let topic =
                node_def.config.get("topic").and_then(toml::Value::as_str).ok_or_else(|| {
                    WaferError::Config(ConfigError::Message(format!(
                        "mqtt sink '{}' requires 'topic' in config",
                        node_def.id
                    )))
                })?;
            let qos =
                node_def.config.get("qos").and_then(toml::Value::as_integer).map_or(0, |v| v as u8);
            let client_id = node_def
                .config
                .get("client_id")
                .and_then(toml::Value::as_str)
                .map_or_else(|| format!("wafer-{}", node_def.id), String::from);
            Ok(AnyNode::from_sink(MqttSink::new(&node_def.id, broker, port, topic, qos, client_id)))
        }
        "http" => {
            let url =
                node_def.config.get("url").and_then(toml::Value::as_str).ok_or_else(|| {
                    WaferError::Config(ConfigError::Message(format!(
                        "http sink '{}' requires 'url' in config",
                        node_def.id
                    )))
                })?;
            let batch_size = node_def
                .config
                .get("batch_size")
                .and_then(toml::Value::as_integer)
                .map(|v| v as usize);
            let batch_timeout_ms = node_def
                .config
                .get("batch_timeout_ms")
                .and_then(toml::Value::as_integer)
                .map(|v| v as u64);
            let timeout_secs = node_def
                .config
                .get("timeout_secs")
                .and_then(toml::Value::as_integer)
                .map_or(30, |v| v as u64);

            let batch_config = HttpSinkBatchConfig { batch_size, batch_timeout_ms };

            let sink = HttpSink::with_batching(&node_def.id, url, batch_config)
                .with_timeout(Duration::from_secs(timeout_secs));

            Ok(AnyNode::from_sink(sink))
        }
        _ => {
            let path =
                node_def.config.get("path").and_then(toml::Value::as_str).ok_or_else(|| {
                    WaferError::Config(ConfigError::Message(format!(
                        "sink '{}' requires 'path' in config",
                        node_def.id
                    )))
                })?;
            Ok(AnyNode::from_sink(FileSink::new(&node_def.id, path)))
        }
    }
}
