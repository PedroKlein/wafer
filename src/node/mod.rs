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

mod traits;
mod transform;

pub use traits::{Lifecycle, NodeConfig, ProcessError, ProcessResult, Transform};
pub use transform::WasmTransform;

use crate::error::Result;

/// Enum for heterogeneous node storage in DAG orchestration.
///
/// This allows storing different node types in the same collection
/// while still being able to call lifecycle methods uniformly.
pub enum AnyNode {
    /// A transform node (1:1 message processing)
    Transform(Box<dyn Transform>),
    // Future variants:
    // Source(Box<dyn Source>),
    // Sink(Box<dyn Sink>),
    // Router(Box<dyn Router>),
    // Joiner(Box<dyn Joiner>),
}

impl AnyNode {
    /// Create an AnyNode from a Transform implementation.
    pub fn from_transform(t: impl Transform + 'static) -> Self {
        AnyNode::Transform(Box::new(t))
    }

    /// Get the node's ID.
    pub fn id(&self) -> &str {
        match self {
            AnyNode::Transform(t) => t.id(),
        }
    }

    /// Validate the node's configuration.
    pub fn validate(&self) -> Result<()> {
        match self {
            AnyNode::Transform(t) => t.validate(),
        }
    }

    /// Initialize the node.
    pub async fn init(&mut self) -> Result<()> {
        match self {
            AnyNode::Transform(t) => t.init().await,
        }
    }

    /// Close the node gracefully.
    pub async fn close(&mut self) -> Result<()> {
        match self {
            AnyNode::Transform(t) => t.close().await,
        }
    }
}
