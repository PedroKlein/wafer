//! Node trait architecture for WAFER pipeline.

mod kind;
mod metrics;
pub mod native;
mod router;
mod sink;
mod source;
mod state;
mod traits;
pub mod wasm;

pub use router::{RouterInstance, WasmRouter};
pub use sink::{BatchStats, BenchSink, BenchSinkConfig, FileSink, HotSwapRecorder, HttpSink, HttpSinkBatchConfig, MqttSink, SequenceTracker, Sink, StdoutSink, SwapTransition, ThroughputSample};
pub use source::{BenchSource, BenchSourceConfig, FileSource, HttpSource, MqttSource, Source, StdinSource};
pub use native::{NativeFilter, NativeTransform};
pub use kind::{Node, NodeKind};
pub use metrics::NodeMetrics;
pub use state::{NodeStateTracker, ProcessingGuard};
pub use traits::{
    ConfigParseError, Filter, FilterOutcome, Lifecycle, NodeConfig, ProcessError,
    ProcessResult, RouteOutcome, RouteResult, Router, Transform,
};

use crate::error::Result;
use std::fmt;
use std::sync::Arc;
use wafer_types::NodeState;

/// Enum for heterogeneous node storage in DAG orchestration.
///
/// Each variant includes a shared [`NodeStateTracker`] for hot-swap support.
pub enum AnyNode {
    Transform(Box<dyn Transform>, Arc<NodeStateTracker>),
    Source(Box<dyn Source>, Arc<NodeStateTracker>),
    Sink(Box<dyn Sink>, Arc<NodeStateTracker>),
    Router(Box<dyn Router>, Arc<NodeStateTracker>),
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
        }
    }
}

impl AnyNode {
    #[must_use]
    pub fn from_transform(t: impl Transform + 'static) -> Self {
        AnyNode::Transform(Box::new(t), Arc::new(NodeStateTracker::new()))
    }

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

    #[must_use]
    pub fn from_sink(s: impl Sink + 'static) -> Self {
        AnyNode::Sink(Box::new(s), Arc::new(NodeStateTracker::new()))
    }

    #[must_use]
    pub fn from_router(r: impl Router + 'static) -> Self {
        AnyNode::Router(Box::new(r), Arc::new(NodeStateTracker::new()))
    }


    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            AnyNode::Transform(t, _) => t.id(),
            AnyNode::Source(s, _) => s.id(),
            AnyNode::Sink(s, _) => s.id(),
            AnyNode::Router(r, _) => r.id(),
        }
    }

    /// Get the current node state.
    #[must_use]
    pub fn state(&self) -> NodeState {
        self.state_tracker().state()
    }

    #[must_use]
    pub fn state_tracker(&self) -> &Arc<NodeStateTracker> {
        match self {
            AnyNode::Transform(_, tracker)
            | AnyNode::Source(_, tracker)
            | AnyNode::Sink(_, tracker)
            | AnyNode::Router(_, tracker) => tracker,
        }
    }

    /// Get a clone of the state tracker Arc for sharing.
    #[must_use]
    pub fn state_tracker_clone(&self) -> Arc<NodeStateTracker> {
        Arc::clone(self.state_tracker())
    }

    #[must_use]
    pub fn is_swappable(&self) -> bool {
        matches!(self, AnyNode::Transform(_, _) | AnyNode::Router(_, _))
    }

    /// Validate the node's configuration.
    pub fn validate(&self) -> Result<()> {
        match self {
            AnyNode::Transform(t, _) => t.validate(),
            AnyNode::Source(s, _) => s.validate(),
            AnyNode::Sink(s, _) => s.validate(),
            AnyNode::Router(r, _) => r.validate(),
        }
    }

    /// Initialize the node. Transitions state to `Running` on success, `Error` on failure.
    pub async fn init(&mut self) -> Result<()> {
        let result = match self {
            AnyNode::Transform(t, _) => t.init().await,
            AnyNode::Source(s, _) => s.init().await,
            AnyNode::Sink(s, _) => s.init().await,
            AnyNode::Router(r, _) => r.init().await,
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
    pub async fn close(&mut self) -> Result<()> {
        match self {
            AnyNode::Transform(t, _) => t.close().await,
            AnyNode::Source(s, _) => s.close().await,
            AnyNode::Sink(s, _) => s.close().await,
            AnyNode::Router(r, _) => r.close().await,
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
        };
        write!(f, "{name}")
    }
}
