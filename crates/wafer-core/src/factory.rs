// Allow casts in config parsing - TOML integers are validated at config level
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]

//! Node factory for creating pipeline nodes from configuration.
//!
//! This module handles the instantiation of source, transform, and sink nodes
//! from their TOML configuration definitions.
//!
//! # Usage
//!
//! ```no_run
//! use wafer_core::config::NodeDefinition;
//! use wafer_core::factory::{create_node, FactoryContext};
//! use wafer_core::registry::RegistryConfig;
//!
//! async fn example(node_def: &NodeDefinition) {
//!     let registry_config = RegistryConfig::default();
//!     let mut ctx = FactoryContext::new(registry_config).expect("Failed to create context");
//!     let node = create_node(node_def, &mut ctx).await.expect("Failed to create node");
//!     
//!     // When pipeline completes, clean up epoch tickers
//!     ctx.abort_tickers();
//! }
//! ```
//!
//! # Supported Node Types
//!
//! ## Sources
//! - `stdin` - Reads lines from standard input
//! - `file` (default) - Reads lines from a file (requires `path` in config)
//! - `mqtt` - Subscribes to an MQTT topic (requires `topic` in config)
//! - `http` - Receives POST requests on a configurable endpoint (optional `bind`, `path` in config)
//!
//! ## Transforms
//! - WASM components loaded from `plugin_path` in config (local path)
//! - WASM components loaded from `package` + `version` in config (remote registry)
//!
//! ## Sinks
//! - `stdout` - Writes to standard output
//! - `file` (default) - Writes to a file (requires `path` in config)
//! - `mqtt` - Publishes to an MQTT topic (requires `topic` in config)
//! - `http` - POSTs messages to an HTTP endpoint (requires `url` in config)

use std::collections::HashMap;
use tokio::task::JoinHandle;

use crate::config::{DeadLetterConfig, NodeConfig as PluginNodeConfig, NodeDefinition, NodeType};
use crate::engine::{Capabilities, TransformInstance, WaferEngine};
use crate::error::{ConfigError, WaferError};
use crate::node::{
    AnyNode, FileSink, FileSource, HttpSink, HttpSource, JoinerInstance, MqttSink, MqttSource,
    NodeConfig, RouterInstance, Sink, StdinSource, StdoutSink, WasmJoiner, WasmRouter,
    WasmTransform,
};
use crate::registry::{PluginSource, RegistryConfig, ResolvedPlugin, WaferRegistry};
use crate::Result;

/// Context for node creation that tracks resources needing cleanup.
///
/// This struct collects epoch ticker handles during node creation so they
/// can be properly cleaned up when the pipeline stops. Each WASM transform
/// node requires an epoch ticker for cooperative interruption.
///
/// It also manages the registry client for loading plugins from OCI registries
/// and tracks resolved plugins for future hot-swap support.
///
/// # Lifecycle
///
/// 1. Create a new context with registry configuration before creating nodes
/// 2. Pass to `create_node` for each node
/// 3. Call `abort_tickers` when the pipeline completes
///
/// # Example
///
/// ```no_run
/// use wafer_core::factory::FactoryContext;
/// use wafer_core::registry::RegistryConfig;
///
/// let config = RegistryConfig::default();
/// let mut ctx = FactoryContext::new(config).expect("Failed to create context");
/// // ... create nodes ...
/// ctx.abort_tickers(); // Cleanup when done
/// ```
pub struct FactoryContext {
    /// Epoch ticker handles for WASM engines (need abort on shutdown).
    pub epoch_tickers: Vec<JoinHandle<()>>,
    /// Registry client for fetching remote plugins.
    registry: WaferRegistry,
    /// Resolved plugins indexed by node ID (for future hot-swap support).
    resolved_plugins: HashMap<String, ResolvedPlugin>,
}

impl FactoryContext {
    /// Create a new factory context with registry configuration.
    ///
    /// # Arguments
    ///
    /// * `registry_config` - Configuration for the registry client (cache settings, default registry, etc.)
    ///
    /// # Errors
    ///
    /// Returns `WaferError::Registry` if the registry client fails to initialize.
    pub fn new(registry_config: RegistryConfig) -> Result<Self> {
        let registry =
            WaferRegistry::new(registry_config).map_err(WaferError::Registry)?;

        Ok(Self {
            epoch_tickers: Vec::new(),
            registry,
            resolved_plugins: HashMap::new(),
        })
    }

    /// Abort all epoch tickers.
    ///
    /// Call this when the pipeline stops to clean up background tasks.
    pub fn abort_tickers(&self) {
        for ticker in &self.epoch_tickers {
            ticker.abort();
        }
    }

    /// Get a reference to the resolved plugins map.
    ///
    /// This is useful for hot-swap operations where you need to know
    /// the current state of loaded plugins.
    #[must_use]
    pub fn resolved_plugins(&self) -> &HashMap<String, ResolvedPlugin> {
        &self.resolved_plugins
    }

    /// Get the resolved plugin for a specific node.
    #[must_use]
    pub fn get_resolved_plugin(&self, node_id: &str) -> Option<&ResolvedPlugin> {
        self.resolved_plugins.get(node_id)
    }
}

/// Create a node from its configuration definition.
///
/// # Arguments
///
/// * `node_def` - The node definition from TOML config
/// * `ctx` - Factory context for tracking cleanup resources
///
/// # Errors
///
/// Returns `WaferError::Config` if:
/// - Required configuration fields are missing
/// - Plugin path doesn't exist or can't be loaded
/// - WASM component instantiation fails
pub async fn create_node(node_def: &NodeDefinition, ctx: &mut FactoryContext) -> Result<AnyNode> {
    match node_def.node_type {
        NodeType::Source => create_source(node_def),
        NodeType::Transform => create_transform(node_def, ctx).await,
        NodeType::Sink => create_sink(node_def),
        NodeType::Router => create_router(node_def, ctx).await,
        NodeType::Joiner => create_joiner(node_def, ctx).await,
    }
}

/// Create a source node from configuration.
fn create_source(node_def: &NodeDefinition) -> Result<AnyNode> {
    let source_type = node_def.source_type.as_deref().unwrap_or("file");
    if source_type == "stdin" {
        Ok(AnyNode::from_source(StdinSource::new(&node_def.id)))
    } else if source_type == "mqtt" {
        let broker = node_def
            .config
            .get("broker")
            .and_then(toml::Value::as_str)
            .unwrap_or("localhost");
        let port = node_def
            .config
            .get("port")
            .and_then(toml::Value::as_integer)
            .map_or(1883, |v| v as u16);
        let topic = node_def
            .config
            .get("topic")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| {
                WaferError::Config(ConfigError::Message(format!(
                    "mqtt source '{}' requires 'topic' in config",
                    node_def.id
                )))
            })?;
        let qos = node_def
            .config
            .get("qos")
            .and_then(toml::Value::as_integer)
            .map_or(0, |v| v as u8);
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
    } else if source_type == "http" {
        let bind = node_def
            .config
            .get("bind")
            .and_then(toml::Value::as_str)
            .unwrap_or("127.0.0.1:8081");
        let path = node_def
            .config
            .get("path")
            .and_then(toml::Value::as_str)
            .unwrap_or("/ingest");
        let buffer_size = node_def
            .config
            .get("buffer_size")
            .and_then(toml::Value::as_integer)
            .map_or(1000, |v| v as usize);
        Ok(AnyNode::from_source(
            HttpSource::new(&node_def.id, bind, path).with_buffer_size(buffer_size),
        ))
    } else {
        // Default to file source
        let path = node_def
            .config
            .get("path")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| {
                WaferError::Config(ConfigError::Message(format!(
                    "source '{}' requires 'path' in config",
                    node_def.id
                )))
            })?;
        Ok(AnyNode::from_source(FileSource::new(&node_def.id, path)))
    }
}

/// Create a transform node from configuration.
///
/// This function supports both local plugin paths and remote registry packages:
/// - Local: Specify `plugin_path` in config
/// - Remote: Specify `package` and `version` in config
///
/// For backward compatibility, `plugin_path` takes precedence if both are specified.
async fn create_transform(node_def: &NodeDefinition, ctx: &mut FactoryContext) -> Result<AnyNode> {
    // Parse the node config to get plugin source specification
    let plugin_config: PluginNodeConfig =
        node_def.config.clone().try_into().map_err(|e: toml::de::Error| {
            WaferError::Config(ConfigError::Message(format!(
                "failed to parse transform '{}' config: {}",
                node_def.id, e
            )))
        })?;

    // Create the WASM engine
    let engine = WaferEngine::new()?;
    // Start epoch ticker for cooperative interruption
    ctx.epoch_tickers.push(engine.start_epoch_ticker());

    // Resolve and load the plugin component
    let component = resolve_and_load_plugin(&node_def.id, &plugin_config, &engine, ctx).await?;

    // Use stdio + inference capabilities for MVP
    // TODO: Add per-node capability configuration in TOML schema
    let capabilities = Capabilities::with_stdio().inference(true);
    let instance = TransformInstance::new(&engine, &component, capabilities).await?;

    let config_str = toml::to_string(&node_def.config)
        .map_err(|e| WaferError::Config(ConfigError::Message(e.to_string())))?;
    let node_config =
        NodeConfig::new(&node_def.id, "transform").with_config_bytes(config_str.into_bytes());

    let transform = WasmTransform::new(engine, instance, node_config);
    Ok(AnyNode::from_transform(transform))
}

/// Create a router node from configuration.
///
/// This function supports both local plugin paths and remote registry packages.
/// Follows the same pattern as `create_transform`.
async fn create_router(node_def: &NodeDefinition, ctx: &mut FactoryContext) -> Result<AnyNode> {
    // Parse the node config to get plugin source specification
    let plugin_config: PluginNodeConfig =
        node_def.config.clone().try_into().map_err(|e: toml::de::Error| {
            WaferError::Config(ConfigError::Message(format!(
                "failed to parse router '{}' config: {}",
                node_def.id, e
            )))
        })?;

    // Create the WASM engine
    let engine = WaferEngine::new()?;
    // Start epoch ticker for cooperative interruption
    ctx.epoch_tickers.push(engine.start_epoch_ticker());

    // Resolve and load the plugin component
    let component = resolve_and_load_plugin(&node_def.id, &plugin_config, &engine, ctx).await?;

    // Use stdio + inference capabilities for MVP
    let capabilities = Capabilities::with_stdio().inference(true);
    let instance = RouterInstance::new(&engine, &component, capabilities).await?;

    let config_str = toml::to_string(&node_def.config)
        .map_err(|e| WaferError::Config(ConfigError::Message(e.to_string())))?;
    let node_config =
        NodeConfig::new(&node_def.id, "router").with_config_bytes(config_str.into_bytes());

    let router = WasmRouter::new(engine, instance, node_config);
    Ok(AnyNode::from_router(router))
}

/// Create a joiner node from configuration.
///
/// This function supports both local plugin paths and remote registry packages.
/// Follows the same pattern as `create_transform`.
async fn create_joiner(node_def: &NodeDefinition, ctx: &mut FactoryContext) -> Result<AnyNode> {
    // Parse the node config to get plugin source specification
    let plugin_config: PluginNodeConfig =
        node_def.config.clone().try_into().map_err(|e: toml::de::Error| {
            WaferError::Config(ConfigError::Message(format!(
                "failed to parse joiner '{}' config: {}",
                node_def.id, e
            )))
        })?;

    // Create the WASM engine
    let engine = WaferEngine::new()?;
    // Start epoch ticker for cooperative interruption
    ctx.epoch_tickers.push(engine.start_epoch_ticker());

    // Resolve and load the plugin component
    let component = resolve_and_load_plugin(&node_def.id, &plugin_config, &engine, ctx).await?;

    // Use stdio + inference capabilities for MVP
    let capabilities = Capabilities::with_stdio().inference(true);
    let instance = JoinerInstance::new(&engine, &component, capabilities).await?;

    let config_str = toml::to_string(&node_def.config)
        .map_err(|e| WaferError::Config(ConfigError::Message(e.to_string())))?;
    let node_config =
        NodeConfig::new(&node_def.id, "joiner").with_config_bytes(config_str.into_bytes());

    let joiner = WasmJoiner::new(engine, instance, node_config);
    Ok(AnyNode::from_joiner(joiner))
}

/// Resolve and load a plugin component from either local path or remote registry.
///
/// This function:
/// 1. Gets the plugin source from the node configuration
/// 2. Resolves it using the registry client (validates local paths, fetches remote packages)
/// 3. Loads the WASM component using the appropriate method
/// 4. Tracks the resolved plugin for future hot-swap support
///
/// # Arguments
///
/// * `node_id` - The node identifier (used for error messages and tracking)
/// * `plugin_config` - The parsed plugin configuration with source specification
/// * `engine` - The WASM engine to load the component into
/// * `ctx` - The factory context containing the registry client
///
/// # Errors
///
/// Returns an error if:
/// - Plugin source configuration is invalid
/// - Plugin resolution fails (file not found, fetch failed, etc.)
/// - WASM component loading fails
async fn resolve_and_load_plugin(
    node_id: &str,
    plugin_config: &PluginNodeConfig,
    engine: &WaferEngine,
    ctx: &mut FactoryContext,
) -> Result<wasmtime::component::Component> {
    // Get the plugin source from config
    let source = plugin_config
        .plugin_source()
        .map_err(WaferError::Config)?;

    // Resolve the plugin (validates local paths, fetches remote packages)
    let resolved = ctx
        .registry
        .resolve(&source)
        .await
        .map_err(WaferError::Registry)?;

    // Load the component based on source type
    let component = match &resolved.source {
        PluginSource::Local(path) => {
            // For local files, load directly from path
            tracing::debug!("Loading local plugin for '{}' from {:?}", node_id, path);
            engine.load_component(path)?
        }
        PluginSource::Oci(oci_ref) => {
            // For OCI images, load from cached/fetched bytes
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

    // Track the resolved plugin for future hot-swap support
    ctx.resolved_plugins.insert(node_id.to_string(), resolved);

    Ok(component)
}

/// Create a Dead Letter Queue sink from configuration.
///
/// This creates a sink instance that will receive failed/dropped messages.
/// The sink type and configuration are taken from the `[dead_letter]` config section.
///
/// # Arguments
///
/// * `config` - The dead letter configuration
///
/// # Returns
///
/// A boxed sink trait object ready to receive DLQ messages.
///
/// # Errors
///
/// Returns an error if:
/// - The sink type is not recognized
/// - Required sink configuration is missing
pub fn create_dlq_sink(config: &DeadLetterConfig) -> Result<Box<dyn Sink + Send>> {
    let sink_type = config.sink_type.as_str();
    match sink_type {
        "stdout" => Ok(Box::new(StdoutSink::new("dlq"))),
        "file" => {
            let path = config
                .config
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    WaferError::Config(ConfigError::Message(
                        "dead_letter file sink requires 'path' in config".to_string(),
                    ))
                })?;
            Ok(Box::new(FileSink::new("dlq", path)))
        }
        "mqtt" => {
            let broker = config
                .config
                .get("broker")
                .and_then(toml::Value::as_str)
                .unwrap_or("localhost");
            let port = config
                .config
                .get("port")
                .and_then(toml::Value::as_integer)
                .map_or(1883, |v| v as u16);
            let topic = config
                .config
                .get("topic")
                .and_then(toml::Value::as_str)
                .ok_or_else(|| {
                    WaferError::Config(ConfigError::Message(
                        "dead_letter mqtt sink requires 'topic' in config".to_string(),
                    ))
                })?;
            let qos = config
                .config
                .get("qos")
                .and_then(toml::Value::as_integer)
                .map_or(0, |v| v as u8);
            let client_id = config
                .config
                .get("client_id")
                .and_then(toml::Value::as_str)
                .map_or_else(|| "wafer-dlq".to_string(), String::from);
            Ok(Box::new(MqttSink::new(
                "dlq", broker, port, topic, qos, client_id,
            )))
        }
        "http" => {
            let url = config
                .config
                .get("url")
                .and_then(toml::Value::as_str)
                .ok_or_else(|| {
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

/// Create a sink node from configuration.
fn create_sink(node_def: &NodeDefinition) -> Result<AnyNode> {
    use crate::node::HttpSinkBatchConfig;
    use std::time::Duration;

    let sink_type = node_def.sink_type.as_deref().unwrap_or("file");
    if sink_type == "stdout" {
        Ok(AnyNode::from_sink(StdoutSink::new(&node_def.id)))
    } else if sink_type == "mqtt" {
        let broker = node_def
            .config
            .get("broker")
            .and_then(toml::Value::as_str)
            .unwrap_or("localhost");
        let port = node_def
            .config
            .get("port")
            .and_then(toml::Value::as_integer)
            .map_or(1883, |v| v as u16);
        let topic = node_def
            .config
            .get("topic")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| {
                WaferError::Config(ConfigError::Message(format!(
                    "mqtt sink '{}' requires 'topic' in config",
                    node_def.id
                )))
            })?;
        let qos = node_def
            .config
            .get("qos")
            .and_then(toml::Value::as_integer)
            .map_or(0, |v| v as u8);
        let client_id = node_def
            .config
            .get("client_id")
            .and_then(toml::Value::as_str)
            .map_or_else(|| format!("wafer-{}", node_def.id), String::from);
        Ok(AnyNode::from_sink(MqttSink::new(
            &node_def.id,
            broker,
            port,
            topic,
            qos,
            client_id,
        )))
    } else if sink_type == "http" {
        let url = node_def
            .config
            .get("url")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| {
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

        let batch_config = HttpSinkBatchConfig {
            batch_size,
            batch_timeout_ms,
        };

        let sink = HttpSink::with_batching(&node_def.id, url, batch_config)
            .with_timeout(Duration::from_secs(timeout_secs));

        Ok(AnyNode::from_sink(sink))
    } else {
        // Default to file sink
        let path = node_def
            .config
            .get("path")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| {
                WaferError::Config(ConfigError::Message(format!(
                    "sink '{}' requires 'path' in config",
                    node_def.id
                )))
            })?;
        Ok(AnyNode::from_sink(FileSink::new(&node_def.id, path)))
    }
}
