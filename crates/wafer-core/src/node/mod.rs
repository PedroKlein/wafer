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
mod state;
mod traits;
mod transform;

pub use joiner::{JoinerInstance, WasmJoiner};
pub use router::{RouterInstance, WasmRouter};
pub use sink::{BatchStats, FileSink, HttpSink, HttpSinkBatchConfig, MqttSink, Sink, StdoutSink};
pub use source::{FileSource, HttpSource, MqttSource, Source, StdinSource};
pub use state::NodeStateTracker;
pub use traits::{
    ConfigParseError, Joiner, Lifecycle, NodeConfig, ProcessError, ProcessResult, RouteResult,
    Router, Transform,
};
pub use transform::WasmTransform;

use crate::error::Result;
use std::fmt;
use std::sync::Arc;
use wafer_types::NodeState;

/// Enum for heterogeneous node storage in DAG orchestration.
///
/// This allows storing different node types in the same collection
/// while still being able to call lifecycle methods uniformly.
///
/// Each variant includes a shared [`NodeStateTracker`] for hot-swap support.
/// The tracker is wrapped in `Arc` so it can be shared with the orchestrator
/// while the node itself is owned by the execution task.
pub enum AnyNode {
    Transform(Box<dyn Transform>, Arc<NodeStateTracker>),
    Source(Box<dyn Source>, Arc<NodeStateTracker>),
    Sink(Box<dyn Sink>, Arc<NodeStateTracker>),
    Router(Box<dyn Router>, Arc<NodeStateTracker>),
    Joiner(Box<dyn Joiner>, Arc<NodeStateTracker>),
}

impl fmt::Debug for AnyNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AnyNode::Transform(t, tracker) => f
                .debug_struct("AnyNode::Transform")
                .field("id", &t.id())
                .field("node_type", &t.node_type())
                .field("state", &tracker.state())
                .finish(),
            AnyNode::Source(s, tracker) => f
                .debug_struct("AnyNode::Source")
                .field("id", &s.id())
                .field("node_type", &s.node_type())
                .field("state", &tracker.state())
                .finish(),
            AnyNode::Sink(s, tracker) => f
                .debug_struct("AnyNode::Sink")
                .field("id", &s.id())
                .field("node_type", &s.node_type())
                .field("state", &tracker.state())
                .finish(),
            AnyNode::Router(r, tracker) => f
                .debug_struct("AnyNode::Router")
                .field("id", &r.id())
                .field("node_type", &r.node_type())
                .field("state", &tracker.state())
                .finish(),
            AnyNode::Joiner(j, tracker) => f
                .debug_struct("AnyNode::Joiner")
                .field("id", &j.id())
                .field("node_type", &j.node_type())
                .field("state", &tracker.state())
                .finish(),
        }
    }
}

impl AnyNode {
    /// Wrap a transform node in the `AnyNode` enum.
    #[must_use]
    pub fn from_transform(t: impl Transform + 'static) -> Self {
        AnyNode::Transform(Box::new(t), Arc::new(NodeStateTracker::new()))
    }

    /// Wrap a transform node with an existing state tracker.
    #[must_use]
    pub fn from_transform_with_tracker(
        t: impl Transform + 'static,
        tracker: Arc<NodeStateTracker>,
    ) -> Self {
        AnyNode::Transform(Box::new(t), tracker)
    }

    /// Wrap a source node in the `AnyNode` enum.
    #[must_use]
    pub fn from_source(s: impl Source + 'static) -> Self {
        AnyNode::Source(Box::new(s), Arc::new(NodeStateTracker::new()))
    }

    /// Wrap a sink node in the `AnyNode` enum.
    #[must_use]
    pub fn from_sink(s: impl Sink + 'static) -> Self {
        AnyNode::Sink(Box::new(s), Arc::new(NodeStateTracker::new()))
    }

    /// Wrap a router node in the `AnyNode` enum.
    #[must_use]
    pub fn from_router(r: impl Router + 'static) -> Self {
        AnyNode::Router(Box::new(r), Arc::new(NodeStateTracker::new()))
    }

    /// Wrap a joiner node in the `AnyNode` enum.
    #[must_use]
    pub fn from_joiner(j: impl Joiner + 'static) -> Self {
        AnyNode::Joiner(Box::new(j), Arc::new(NodeStateTracker::new()))
    }

    /// Get the node's unique identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            AnyNode::Transform(t, _) => t.id(),
            AnyNode::Source(s, _) => s.id(),
            AnyNode::Sink(s, _) => s.id(),
            AnyNode::Router(r, _) => r.id(),
            AnyNode::Joiner(j, _) => j.id(),
        }
    }

    /// Get the current node state.
    #[must_use]
    pub fn state(&self) -> NodeState {
        self.state_tracker().state()
    }

    /// Get a reference to the state tracker.
    #[must_use]
    pub fn state_tracker(&self) -> &Arc<NodeStateTracker> {
        match self {
            AnyNode::Transform(_, tracker)
            | AnyNode::Source(_, tracker)
            | AnyNode::Sink(_, tracker)
            | AnyNode::Router(_, tracker)
            | AnyNode::Joiner(_, tracker) => tracker,
        }
    }

    /// Get a clone of the state tracker Arc for sharing.
    #[must_use]
    pub fn state_tracker_clone(&self) -> Arc<NodeStateTracker> {
        Arc::clone(self.state_tracker())
    }

    /// Check if this node supports hot-swap.
    ///
    /// Only WASM Transform, Router, and Joiner nodes support hot-swap.
    /// Native Source and Sink nodes do not.
    #[must_use]
    pub fn is_swappable(&self) -> bool {
        matches!(self, AnyNode::Transform(_, _) | AnyNode::Router(_, _) | AnyNode::Joiner(_, _))
    }

    /// Validate the node's configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the node's configuration is invalid.
    pub fn validate(&self) -> Result<()> {
        match self {
            AnyNode::Transform(t, _) => t.validate(),
            AnyNode::Source(s, _) => s.validate(),
            AnyNode::Sink(s, _) => s.validate(),
            AnyNode::Router(r, _) => r.validate(),
            AnyNode::Joiner(j, _) => j.validate(),
        }
    }

    /// Initialize the node for execution.
    ///
    /// Transitions state from `Starting` to `Running` on success.
    ///
    /// # Errors
    ///
    /// Returns an error if initialization fails.
    /// State transitions to `Error` on failure.
    pub async fn init(&mut self) -> Result<()> {
        let result = match self {
            AnyNode::Transform(t, _) => t.init().await,
            AnyNode::Source(s, _) => s.init().await,
            AnyNode::Sink(s, _) => s.init().await,
            AnyNode::Router(r, _) => r.init().await,
            AnyNode::Joiner(j, _) => j.init().await,
        };

        match &result {
            Ok(()) => {
                self.state_tracker().transition_to_running();
            }
            Err(_) => {
                self.state_tracker().transition_to_error();
            }
        }

        result
    }

    /// Gracefully close the node and release resources.
    ///
    /// # Errors
    ///
    /// Returns an error if cleanup fails.
    pub async fn close(&mut self) -> Result<()> {
        match self {
            AnyNode::Transform(t, _) => t.close().await,
            AnyNode::Source(s, _) => s.close().await,
            AnyNode::Sink(s, _) => s.close().await,
            AnyNode::Router(r, _) => r.close().await,
            AnyNode::Joiner(j, _) => j.close().await,
        }
    }
}

impl fmt::Display for AnyNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            AnyNode::Source(s, _) => s.node_type(),
            AnyNode::Transform(t, _) => t.node_type(),
            AnyNode::Sink(s, _) => s.node_type(),
            AnyNode::Router(r, _) => r.node_type(),
            AnyNode::Joiner(j, _) => j.node_type(),
        };
        write!(f, "{name}")
    }
}
