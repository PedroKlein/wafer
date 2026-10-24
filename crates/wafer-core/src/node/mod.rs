//! Node trait architecture for WAFER pipeline.

mod kind;
mod metrics;
pub mod native;
mod sink;
mod source;
mod state;
mod traits;
pub mod wasm;

pub use kind::NodeKind;
pub use metrics::{NodeMetrics, QueueMetrics};
pub use native::{NativeFilter, NativeRouter, NativeTransform, ProcessNode};
pub use sink::{
    BatchStats, BenchSink, BenchSinkConfig, FileSink, HotSwapRecorder, HttpSink,
    HttpSinkBatchConfig, MqttSink, SequenceTracker, Sink, StdoutSink, SwapTransition,
    ThroughputSample,
};
pub use source::{
    BenchBurstSchedule, BenchSource, BenchSourceConfig, FileSource, HttpSource, MqttSource, Source,
    StdinSource,
};
pub use state::NodeStateTracker;
pub use traits::{
    ConfigParseError, Filter, FilterOutcome, Lifecycle, NodeConfig, ProcessError, ProcessResult,
    RouteOutcome, RouteResult, Router, Transform,
};

/// MQTT packet limit for the runtime's clients. rumqttc's 10 KiB default
/// drops the connection on any larger PUBLISH.
pub(crate) const MQTT_MAX_PACKET_BYTES: usize = 1024 * 1024;

use crate::error::Result;

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
    pub async fn process(
        &mut self,
        envelope: RuntimeEnvelope,
    ) -> std::result::Result<RuntimeEnvelope, WasmProcessError> {
        match self {
            Self::Wasm(w) => w.process(envelope).await,
            Self::Native(n) => ProcessNode::process(n, envelope),
        }
    }

    /// Config reload (Wasm only — native transforms reject).
    pub async fn try_reconfigure(&mut self, new_config_json: &str) -> Result<()> {
        match self {
            Self::Wasm(w) => w.try_reconfigure(new_config_json).await,
            Self::Native(_) => Err(WaferError::Runtime(
                "native baseline transforms do not support reconfigure".into(),
            )),
        }
    }

    /// Recover via cached InstancePre (Wasm) / no-op (Native).
    pub async fn recover_from_cached_pre(&mut self) -> Result<()> {
        match self {
            Self::Wasm(w) => w.recover_from_cached_pre().await,
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
/// [`TransformNode`], so the RQ1 native baseline can run a filter that does
/// the same work as the Wasm one.
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
    pub async fn evaluate(
        &mut self,
        envelope: &RuntimeEnvelope,
    ) -> std::result::Result<FilterOutcome, WasmProcessError> {
        match self {
            Self::Wasm(w) => w.evaluate(envelope).await,
            Self::Native(n) => n.evaluate_sync(envelope),
        }
    }

    /// Reject reconfigure on native filters — they have no config-driven
    /// guest state to replace.
    pub async fn try_reconfigure(&mut self, new_config_json: &str) -> Result<()> {
        match self {
            Self::Wasm(w) => w.try_reconfigure(new_config_json).await,
            Self::Native(_) => Err(WaferError::Runtime(
                "native baseline filters do not support reconfigure".into(),
            )),
        }
    }

    /// Native filters are stateless — recovery is a no-op success.
    pub async fn recover_from_cached_pre(&mut self) -> Result<()> {
        match self {
            Self::Wasm(w) => w.recover_from_cached_pre().await,
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
