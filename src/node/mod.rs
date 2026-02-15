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

mod sink;
mod source;
mod traits;
mod transform;

pub use sink::{FileSink, Sink};
pub use source::{FileSource, Source};
pub use traits::{ConfigParseError, Lifecycle, NodeConfig, ProcessError, ProcessResult, Transform};
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
        }
    }
}

impl AnyNode {
    pub fn from_transform(t: impl Transform + 'static) -> Self {
        AnyNode::Transform(Box::new(t))
    }

    pub fn from_source(s: impl Source + 'static) -> Self {
        AnyNode::Source(Box::new(s))
    }

    pub fn from_sink(s: impl Sink + 'static) -> Self {
        AnyNode::Sink(Box::new(s))
    }

    pub fn id(&self) -> &str {
        match self {
            AnyNode::Transform(t) => t.id(),
            AnyNode::Source(s) => s.id(),
            AnyNode::Sink(s) => s.id(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        match self {
            AnyNode::Transform(t) => t.validate(),
            AnyNode::Source(s) => s.validate(),
            AnyNode::Sink(s) => s.validate(),
        }
    }

    pub async fn init(&mut self) -> Result<()> {
        match self {
            AnyNode::Transform(t) => t.init().await,
            AnyNode::Source(s) => s.init().await,
            AnyNode::Sink(s) => s.init().await,
        }
    }

    pub async fn close(&mut self) -> Result<()> {
        match self {
            AnyNode::Transform(t) => t.close().await,
            AnyNode::Source(s) => s.close().await,
            AnyNode::Sink(s) => s.close().await,
        }
    }
}
