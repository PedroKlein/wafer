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
pub use source::{BenchBurstSchedule, BenchSource, BenchSourceConfig, FileSource, HttpSource, MqttSource, Source, StdinSource};
pub use native::{NativeFilter, NativeRouter, NativeTransform, ProcessNode};
// Re-export the runner-side TransformNode enum near the top.
// (Full definition below in this module.)
pub use kind::{Node, NodeKind};
pub use metrics::{NodeMetrics, QueueMetrics};
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
            Self::Transform(t, tracker) => f
                .debug_struct("AnyNode::Transform")
                .field("id", &t.id())
                .field("node_type", &t.node_type())
                .field("state", &tracker.state())
                .finish(),
            Self::Source(s, tracker) => f
                .debug_struct("AnyNode::Source")
                .field("id", &s.id())
                .field("node_type", &s.node_type())
                .field("state", &tracker.state())
                .finish(),
            Self::Sink(s, tracker) => f
                .debug_struct("AnyNode::Sink")
                .field("id", &s.id())
                .field("node_type", &s.node_type())
                .field("state", &tracker.state())
                .finish(),
            Self::Router(r, tracker) => f
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
        Self::Transform(Box::new(t), Arc::new(NodeStateTracker::new()))
    }

    #[must_use]
    pub fn from_transform_with_tracker(
        t: impl Transform + 'static,
        tracker: Arc<NodeStateTracker>,
    ) -> Self {
        Self::Transform(Box::new(t), tracker)
    }

    /// Wrap a source node in the `AnyNode` enum.
    #[must_use]
    pub fn from_source(s: impl Source + 'static) -> Self {
        Self::Source(Box::new(s), Arc::new(NodeStateTracker::new()))
    }

    #[must_use]
    pub fn from_sink(s: impl Sink + 'static) -> Self {
        Self::Sink(Box::new(s), Arc::new(NodeStateTracker::new()))
    }

    #[must_use]
    pub fn from_router(r: impl Router + 'static) -> Self {
        Self::Router(Box::new(r), Arc::new(NodeStateTracker::new()))
    }


    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::Transform(t, _) => t.id(),
            Self::Source(s, _) => s.id(),
            Self::Sink(s, _) => s.id(),
            Self::Router(r, _) => r.id(),
        }
    }

    /// Get the current node state.
    #[must_use]
    pub fn state(&self) -> NodeState {
        self.state_tracker().state()
    }

    #[must_use]
    pub const fn state_tracker(&self) -> &Arc<NodeStateTracker> {
        match self {
            Self::Transform(_, tracker)
            | Self::Source(_, tracker)
            | Self::Sink(_, tracker)
            | Self::Router(_, tracker) => tracker,
        }
    }

    /// Get a clone of the state tracker Arc for sharing.
    #[must_use]
    pub fn state_tracker_clone(&self) -> Arc<NodeStateTracker> {
        Arc::clone(self.state_tracker())
    }

    #[must_use]
    pub const fn is_swappable(&self) -> bool {
        matches!(self, Self::Transform(_, _) | Self::Router(_, _))
    }

    /// Validate the node's configuration.
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Transform(t, _) => t.validate(),
            Self::Source(s, _) => s.validate(),
            Self::Sink(s, _) => s.validate(),
            Self::Router(r, _) => r.validate(),
        }
    }

    /// Initialize the node. Transitions state to `Running` on success, `Error` on failure.
    pub async fn init(&mut self) -> Result<()> {
        let result = match self {
            Self::Transform(t, _) => t.init().await,
            Self::Source(s, _) => s.init().await,
            Self::Sink(s, _) => s.init().await,
            Self::Router(r, _) => r.init().await,
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
            Self::Transform(t, _) => t.close().await,
            Self::Source(s, _) => s.close().await,
            Self::Sink(s, _) => s.close().await,
            Self::Router(r, _) => r.close().await,
        }
    }
}

impl fmt::Display for AnyNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Source(s, _) => s.node_type(),
            Self::Transform(t, _) => t.node_type(),
            Self::Sink(s, _) => s.node_type(),
            Self::Router(r, _) => r.node_type(),
        };
        write!(f, "{name}")
    }
}

// =============================================================================
// TransformNode — unified runner-side type covering both Wasm and native.
// =============================================================================

use crate::error::WaferError;
use crate::node::wasm::WasmTransformNode;
use crate::queue::RuntimeEnvelope;
use crate::runner::error_policy::WasmProcessError;

/// Runner-side wrapper distinguishing a Wasm transform (full lifecycle
/// with hot-swap/reconfigure/recover) from a native baseline
/// transform (pure Rust function, no lifecycle beyond process).
///
/// The runner in `crates/wafer-core/src/runner/transform.rs` matches on
/// this enum in the paths that would otherwise depend on Wasm-specific
/// methods. Native transforms report an error for hot-swap operations;
/// recovery is a no-op success (native code is stateless).
pub enum TransformNode {
    /// Full Wasm transform with sandboxing, hot-swap, and recovery.
    Wasm(Box<WasmTransformNode>),
    /// Native Rust baseline (RFC-008 §D5 Layer 1). No sandbox, no
    /// hot-swap, no recovery.
    Native(NativeTransform),
}

impl TransformNode {
    /// Node identifier.
    #[must_use]
    pub fn node_id(&self) -> &str {
        match self {
            Self::Wasm(w) => w.node_id(),
            Self::Native(n) => ProcessNode::node_id(n),
        }
    }

    /// Process one envelope, returning the transformed envelope or a
    /// `WasmProcessError` variant the runner already handles.
    pub fn process(
        &mut self,
        envelope: RuntimeEnvelope,
    ) -> std::result::Result<RuntimeEnvelope, WasmProcessError> {
        match self {
            Self::Wasm(w) => w.process(envelope),
            Self::Native(n) => ProcessNode::process(n, envelope),
        }
    }

    /// Config reload (Wasm only — native transforms reject).
    pub fn try_reconfigure(&mut self, new_config_json: &str) -> Result<()> {
        match self {
            Self::Wasm(w) => w.try_reconfigure(new_config_json),
            Self::Native(_) => Err(WaferError::Runtime(
                "native baseline transforms do not support reconfigure".into(),
            )),
        }
    }

    /// Recover via cached InstancePre (Wasm) / no-op (Native).
    pub fn recover_from_cached_pre(&mut self) -> Result<()> {
        match self {
            Self::Wasm(w) => w.recover_from_cached_pre(),
            Self::Native(_) => Ok(()),
        }
    }

    /// Access the inner Wasm node if this is a Wasm transform.
    /// Used by SwapPayload::try_apply_transform which needs the
    /// concrete type to move new_store/new_bindings into.
    pub fn as_wasm_mut(&mut self) -> Option<&mut WasmTransformNode> {
        match self {
            Self::Wasm(w) => Some(w),
            Self::Native(_) => None,
        }
    }

    /// True when this is a native baseline transform.
    #[must_use]
    pub const fn is_native(&self) -> bool {
        matches!(self, Self::Native(_))
    }
}

impl From<WasmTransformNode> for TransformNode {
    fn from(w: WasmTransformNode) -> Self {
        Self::Wasm(Box::new(w))
    }
}

impl From<NativeTransform> for TransformNode {
    fn from(n: NativeTransform) -> Self {
        Self::Native(n)
    }
}

// =============================================================================
// FilterNode — unified runner-side type covering both Wasm and native.
// =============================================================================

use crate::node::wasm::WasmFilterNode;

/// Runner-side wrapper unifying Wasm and native filters, mirroring
/// [`TransformNode`]. Unblocks the RQ1 apples-to-apples native filter
/// path (`docs/status/implementation-gaps.md` A18).
pub enum FilterNode {
    /// Full Wasm filter with sandboxing, hot-swap, and recovery.
    Wasm(Box<WasmFilterNode>),
    /// Native baseline (RFC-008 §D5). No sandbox, no hot-swap.
    Native(NativeFilter),
}

impl FilterNode {
    #[must_use]
    pub fn node_id(&self) -> &str {
        match self {
            Self::Wasm(w) => w.node_id(),
            Self::Native(n) => n.node_id(),
        }
    }

    /// Native predicates cannot trap, so their `WasmProcessError` slot is
    /// unused; the Wasm path preserves the full trap taxonomy.
    pub fn evaluate(
        &mut self,
        envelope: &RuntimeEnvelope,
    ) -> std::result::Result<FilterOutcome, WasmProcessError> {
        match self {
            Self::Wasm(w) => w.evaluate(envelope),
            Self::Native(n) => n.evaluate_sync(envelope),
        }
    }

    /// Reject reconfigure on native filters — they have no config-driven
    /// guest state to replace.
    pub fn try_reconfigure(&mut self, new_config_json: &str) -> Result<()> {
        match self {
            Self::Wasm(w) => w.try_reconfigure(new_config_json),
            Self::Native(_) => Err(WaferError::Runtime(
                "native baseline filters do not support reconfigure".into(),
            )),
        }
    }

    /// Native filters are stateless — recovery is a no-op success.
    pub fn recover_from_cached_pre(&mut self) -> Result<()> {
        match self {
            Self::Wasm(w) => w.recover_from_cached_pre(),
            Self::Native(_) => Ok(()),
        }
    }

    /// Concrete access for hot-swap payload application; None on native.
    pub fn as_wasm_mut(&mut self) -> Option<&mut WasmFilterNode> {
        match self {
            Self::Wasm(w) => Some(w),
            Self::Native(_) => None,
        }
    }

    #[must_use]
    pub const fn is_native(&self) -> bool {
        matches!(self, Self::Native(_))
    }
}

impl From<WasmFilterNode> for FilterNode {
    fn from(w: WasmFilterNode) -> Self {
        Self::Wasm(Box::new(w))
    }
}

impl From<NativeFilter> for FilterNode {
    fn from(n: NativeFilter) -> Self {
        Self::Native(n)
    }
}
