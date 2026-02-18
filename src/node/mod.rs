//! Node trait architecture for WAFER pipeline.
//!
//! This module defines the core traits that all pipeline nodes implement:
//! - [`Lifecycle`] - Node lifecycle management (validate, init, close)
//! - [`Transform`] - Transform node processing
//!
//! Future node types (Source, Sink, Router, Joiner) will have their own traits
//! that compose with Lifecycle.
//!
//! # Design
//!
//! Traits use composition rather than inheritance. All node types implement
//! Lifecycle, then additionally implement their role-specific trait (Transform,
//! Source, etc.). This enables flexible DAG orchestration where nodes can be
//! treated uniformly for lifecycle but specifically for processing.

mod joiner;
mod router;
mod sink;
mod source;
mod traits;
mod transform;

pub use sink::{FileSink, MqttSink, Sink, StdoutSink};
pub use source::{FileSource, MqttSource, Source, StdinSource};
pub use traits::{
    ConfigParseError, Joiner, Lifecycle, NodeConfig, ProcessError, ProcessResult, RouteResult,
    Router, Transform,
};
pub use joiner::WasmJoiner;
pub use router::WasmRouter;
pub use transform::WasmTransform;

use crate::error::Result;
use std::fmt;

/// Enum for heterogeneous node storage in DAG orchestration.
///
/// This allows storing different node types in the same collection
/// while still being able to call lifecycle methods uniformly.
pub enum AnyNode {
    Transform(Box<dyn Transform>),
    Source(Box<dyn Source>),
    Sink(Box<dyn Sink>),
    Router(Box<dyn Router>),
    Joiner(Box<dyn Joiner>),
}

impl fmt::Debug for AnyNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AnyNode::Transform(t) => f
                .debug_struct("AnyNode::Transform")
                .field("id", &t.id())
                .field("node_type", &t.node_type())
                .finish(),
            AnyNode::Source(s) => f
                .debug_struct("AnyNode::Source")
                .field("id", &s.id())
                .field("node_type", &s.node_type())
                .finish(),
            AnyNode::Sink(s) => f
                .debug_struct("AnyNode::Sink")
                .field("id", &s.id())
                .field("node_type", &s.node_type())
                .finish(),
            AnyNode::Router(r) => f
                .debug_struct("AnyNode::Router")
                .field("id", &r.id())
                .field("node_type", &r.node_type())
                .finish(),
            AnyNode::Joiner(j) => f
                .debug_struct("AnyNode::Joiner")
                .field("id", &j.id())
                .field("node_type", &j.node_type())
                .finish(),
        }
    }
}

impl AnyNode {
    /// Wrap a transform node in the `AnyNode` enum.
    #[must_use]
    pub fn from_transform(t: impl Transform + 'static) -> Self {
        AnyNode::Transform(Box::new(t))
    }

    /// Wrap a source node in the `AnyNode` enum.
    #[must_use]
    pub fn from_source(s: impl Source + 'static) -> Self {
        AnyNode::Source(Box::new(s))
    }

    /// Wrap a sink node in the `AnyNode` enum.
    #[must_use]
    pub fn from_sink(s: impl Sink + 'static) -> Self {
        AnyNode::Sink(Box::new(s))
    }

    /// Wrap a router node in the `AnyNode` enum.
    #[must_use]
    pub fn from_router(r: impl Router + 'static) -> Self {
        AnyNode::Router(Box::new(r))
    }

    /// Wrap a joiner node in the `AnyNode` enum.
    #[must_use]
    pub fn from_joiner(j: impl Joiner + 'static) -> Self {
        AnyNode::Joiner(Box::new(j))
    }

    /// Get the node's unique identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            AnyNode::Transform(t) => t.id(),
            AnyNode::Source(s) => s.id(),
            AnyNode::Sink(s) => s.id(),
            AnyNode::Router(r) => r.id(),
            AnyNode::Joiner(j) => j.id(),
        }
    }

    /// Validate the node's configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the node's configuration is invalid.
    pub fn validate(&self) -> Result<()> {
        match self {
            AnyNode::Transform(t) => t.validate(),
            AnyNode::Source(s) => s.validate(),
            AnyNode::Sink(s) => s.validate(),
            AnyNode::Router(r) => r.validate(),
            AnyNode::Joiner(j) => j.validate(),
        }
    }

    /// Initialize the node for execution.
    ///
    /// # Errors
    ///
    /// Returns an error if initialization fails.
    pub async fn init(&mut self) -> Result<()> {
        match self {
            AnyNode::Transform(t) => t.init().await,
            AnyNode::Source(s) => s.init().await,
            AnyNode::Sink(s) => s.init().await,
            AnyNode::Router(r) => r.init().await,
            AnyNode::Joiner(j) => j.init().await,
        }
    }

    /// Gracefully close the node and release resources.
    ///
    /// # Errors
    ///
    /// Returns an error if cleanup fails.
    pub async fn close(&mut self) -> Result<()> {
        match self {
            AnyNode::Transform(t) => t.close().await,
            AnyNode::Source(s) => s.close().await,
            AnyNode::Sink(s) => s.close().await,
            AnyNode::Router(r) => r.close().await,
            AnyNode::Joiner(j) => j.close().await,
        }
    }
}
