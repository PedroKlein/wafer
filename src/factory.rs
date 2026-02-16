//! Node factory for creating pipeline nodes from configuration.
//!
//! This module handles the instantiation of source, transform, and sink nodes
//! from their TOML configuration definitions.
//!
//! # Usage
//!
//! ```no_run
//! use wafer_poc::config::NodeDefinition;
//! use wafer_poc::factory::{create_node, FactoryContext};
//!
//! async fn example(node_def: &NodeDefinition) {
//!     let mut ctx = FactoryContext::new();
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
//!
//! ## Transforms
//! - WASM components loaded from `plugin_path` in config
//!
//! ## Sinks
//! - `stdout` - Writes to standard output
//! - `file` (default) - Writes to a file (requires `path` in config)

use tokio::task::JoinHandle;

use crate::config::{NodeDefinition, NodeType};
use crate::engine::{Capabilities, TransformInstance, WaferEngine};
use crate::error::{ConfigError, WaferError};
use crate::node::{AnyNode, FileSink, FileSource, NodeConfig, StdinSource, StdoutSink, WasmTransform};
use crate::Result;

/// Context for node creation that tracks resources needing cleanup.
///
/// This struct collects epoch ticker handles during node creation so they
/// can be properly cleaned up when the pipeline stops. Each WASM transform
/// node requires an epoch ticker for cooperative interruption.
///
/// # Lifecycle
///
/// 1. Create a new context before creating nodes
/// 2. Pass to `create_node` for each node
/// 3. Call `abort_tickers` when the pipeline completes
///
/// # Example
///
/// ```no_run
/// use wafer_poc::factory::FactoryContext;
///
/// let mut ctx = FactoryContext::new();
/// // ... create nodes ...
/// ctx.abort_tickers(); // Cleanup when done
/// ```
#[derive(Default)]
pub struct FactoryContext {
    /// Epoch ticker handles for WASM engines (need abort on shutdown).
    pub epoch_tickers: Vec<JoinHandle<()>>,
}

impl FactoryContext {
    /// Create a new empty factory context.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Abort all epoch tickers.
    ///
    /// Call this when the pipeline stops to clean up background tasks.
    pub fn abort_tickers(&self) {
        for ticker in &self.epoch_tickers {
            ticker.abort();
        }
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
    }
}

/// Create a source node from configuration.
fn create_source(node_def: &NodeDefinition) -> Result<AnyNode> {
    let source_type = node_def.source_type.as_deref().unwrap_or("file");
    if source_type == "stdin" {
        Ok(AnyNode::from_source(StdinSource::new(&node_def.id)))
    } else {
        // Default to file source
        let path = node_def
            .config
            .get("path")
            .and_then(|v| v.as_str())
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
async fn create_transform(
    node_def: &NodeDefinition,
    ctx: &mut FactoryContext,
) -> Result<AnyNode> {
    let plugin_path = node_def
        .config
        .get("plugin_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            WaferError::Config(ConfigError::Message(format!(
                "transform '{}' requires 'plugin_path' in config",
                node_def.id
            )))
        })?;

    let engine = WaferEngine::new()?;
    // Start epoch ticker for cooperative interruption
    ctx.epoch_tickers.push(engine.start_epoch_ticker());

    let component = engine.load_component(plugin_path)?;

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

/// Create a sink node from configuration.
fn create_sink(node_def: &NodeDefinition) -> Result<AnyNode> {
    let sink_type = node_def.sink_type.as_deref().unwrap_or("file");
    if sink_type == "stdout" {
        Ok(AnyNode::from_sink(StdoutSink::new(&node_def.id)))
    } else {
        // Default to file sink
        let path = node_def
            .config
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                WaferError::Config(ConfigError::Message(format!(
                    "sink '{}' requires 'path' in config",
                    node_def.id
                )))
            })?;
        Ok(AnyNode::from_sink(FileSink::new(&node_def.id, path)))
    }
}
