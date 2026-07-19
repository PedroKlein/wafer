//! Hot-swap support — watch-channel based swap between messages.
//!
//! Swap happens atomically between messages (no explicit drain phase needed).
//! See docs/rfcs/RFC-005-orchestrator.md D6.

use std::sync::Arc;

use wasmtime::Store;

use crate::engine::WaferEngine;
use crate::engine::state::WaferState;
use crate::engine::Capabilities;
use crate::error::{Result, WaferError};
use crate::runner::SwapPayload;

/// Prepare a transform swap payload from a compiled component.
///
/// Compiles, pre-instantiates, instantiates, and packages into a `SwapPayload`
/// ready to send via watch channel.
///
/// # Errors
///
/// Returns error if compilation or instantiation fails.
pub async fn prepare_transform_swap(
    engine: &WaferEngine,
    wasm_bytes: &[u8],
    node_id: &str,
    capabilities: Capabilities,
) -> Result<SwapPayload> {
    let component = engine.compile_cached(wasm_bytes)?;
    let pre = engine.pre_instantiate_transform(&component)?;
    let pre = Arc::new(pre);

    // Instantiate a fresh Store + bindings from the pre
    let mut store = Store::new(
        engine.inner(),
        WaferState::new(node_id, capabilities),
    );
    store.set_fuel(engine.fuel_limit()).map_err(|e| {
        WaferError::PluginInit { message: format!("failed to set fuel: {e}") }
    })?;
    store.epoch_deadline_trap();
    store.set_epoch_deadline(engine.epoch_deadline());

    let instance = pre.instantiate_async(&mut store).await.map_err(|e| {
        WaferError::PluginInit { message: format!("instantiation failed: {e}") }
    })?;

    Ok(SwapPayload::Transform {
        new_store: Arc::new(std::sync::Mutex::new(Some(store))),
        new_bindings: Arc::new(std::sync::Mutex::new(Some(instance))),
        new_pre: pre,
    })
}

/// Prepare a filter swap payload from a compiled component.
pub async fn prepare_filter_swap(
    engine: &WaferEngine,
    wasm_bytes: &[u8],
    node_id: &str,
    capabilities: Capabilities,
) -> Result<SwapPayload> {
    let component = engine.compile_cached(wasm_bytes)?;
    let pre = engine.pre_instantiate_filter(&component)?;
    let pre = Arc::new(pre);

    let mut store = Store::new(
        engine.inner(),
        WaferState::new(node_id, capabilities),
    );
    store.set_fuel(engine.fuel_limit()).map_err(|e| {
        WaferError::PluginInit { message: format!("failed to set fuel: {e}") }
    })?;
    store.epoch_deadline_trap();
    store.set_epoch_deadline(engine.epoch_deadline());

    let instance = pre.instantiate_async(&mut store).await.map_err(|e| {
        WaferError::PluginInit { message: format!("instantiation failed: {e}") }
    })?;

    Ok(SwapPayload::Filter {
        new_store: Arc::new(std::sync::Mutex::new(Some(store))),
        new_bindings: Arc::new(std::sync::Mutex::new(Some(instance))),
        new_pre: pre,
    })
}

/// Prepare a router swap payload from a compiled component.
pub async fn prepare_router_swap(
    engine: &WaferEngine,
    wasm_bytes: &[u8],
    node_id: &str,
    capabilities: Capabilities,
) -> Result<SwapPayload> {
    let component = engine.compile_cached(wasm_bytes)?;
    let pre = engine.pre_instantiate_router(&component)?;
    let pre = Arc::new(pre);

    let mut store = Store::new(
        engine.inner(),
        WaferState::new(node_id, capabilities),
    );
    store.set_fuel(engine.fuel_limit()).map_err(|e| {
        WaferError::PluginInit { message: format!("failed to set fuel: {e}") }
    })?;
    store.epoch_deadline_trap();
    store.set_epoch_deadline(engine.epoch_deadline());

    let instance = pre.instantiate_async(&mut store).await.map_err(|e| {
        WaferError::PluginInit { message: format!("instantiation failed: {e}") }
    })?;

    Ok(SwapPayload::Router {
        new_store: Arc::new(std::sync::Mutex::new(Some(store))),
        new_bindings: Arc::new(std::sync::Mutex::new(Some(instance))),
        new_pre: pre,
    })
}

// =============================================================================
// Legacy hot-swap coordinator — feature-gated for old tests
// =============================================================================

/// Error types for hot-swap operations.
#[derive(Debug)]
pub enum SwapError {
    NodeNotFound(String),
    NotSwappable(String),
    WatchSendFailed(String),
}

impl std::fmt::Display for SwapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SwapError::NodeNotFound(id) => write!(f, "node '{id}' not found"),
            SwapError::NotSwappable(id) => write!(f, "node '{id}' does not support hot-swap"),
            SwapError::WatchSendFailed(id) => {
                write!(f, "watch channel send failed for node '{id}' (task dead?)")
            }
        }
    }
}

impl std::error::Error for SwapError {}

impl From<SwapError> for WaferError {
    fn from(e: SwapError) -> Self {
        WaferError::Runtime(e.to_string())
    }
}

// =============================================================================
// SwapTimeline — phase decomposition for E-Swap-6
// =============================================================================

/// Records timestamps of each hot-swap phase for latency decomposition.
///
/// Used by E-Swap-6 to identify which phase dominates swap cost:
/// compilation, instantiation, signal propagation, or pipeline convergence.
///
/// See docs/rfcs/RFC-008-evaluation-harness.md — D7A.
#[derive(Debug, Clone)]
pub struct SwapTimeline {
    /// When the swap request was received (API handler or file-watch trigger).
    pub request_time: std::time::Instant,
    /// When Wasm compilation completed (from `engine.compile_cached()`).
    pub compile_done: Option<std::time::Instant>,
    /// When pre-instantiation + instantiation completed.
    pub instantiate_done: Option<std::time::Instant>,
    /// When the swap payload was sent via watch channel.
    pub signal_sent: Option<std::time::Instant>,
    /// When the node loop acknowledged the swap (picked up watch value).
    pub swap_acked: Option<std::time::Instant>,
    /// When the first v2 output was observed at the sink.
    pub first_v2_output: Option<std::time::Instant>,
}

impl SwapTimeline {
    /// Start a new timeline at the given request time.
    #[must_use]
    pub fn start() -> Self {
        Self {
            request_time: std::time::Instant::now(),
            compile_done: None,
            instantiate_done: None,
            signal_sent: None,
            swap_acked: None,
            first_v2_output: None,
        }
    }

    /// Mark compilation as complete.
    pub fn mark_compile_done(&mut self) {
        self.compile_done = Some(std::time::Instant::now());
    }

    /// Mark instantiation as complete.
    pub fn mark_instantiate_done(&mut self) {
        self.instantiate_done = Some(std::time::Instant::now());
    }

    /// Mark signal sent via watch channel.
    pub fn mark_signal_sent(&mut self) {
        self.signal_sent = Some(std::time::Instant::now());
    }

    /// Mark swap acknowledged by node loop.
    pub fn mark_swap_acked(&mut self) {
        self.swap_acked = Some(std::time::Instant::now());
    }

    /// Mark first v2 output observed.
    pub fn mark_first_v2_output(&mut self) {
        self.first_v2_output = Some(std::time::Instant::now());
    }

    // --- Phase duration accessors ---

    /// Compilation duration (request → compile_done).
    #[must_use]
    pub fn compile_duration_ns(&self) -> Option<u64> {
        self.compile_done
            .map(|done| done.duration_since(self.request_time).as_nanos() as u64)
    }

    /// Instantiation duration (compile_done → instantiate_done).
    #[must_use]
    pub fn instantiate_duration_ns(&self) -> Option<u64> {
        match (self.compile_done, self.instantiate_done) {
            (Some(start), Some(end)) => Some(end.duration_since(start).as_nanos() as u64),
            _ => None,
        }
    }

    /// Signal propagation (instantiate_done → signal_sent).
    #[must_use]
    pub fn signal_duration_ns(&self) -> Option<u64> {
        match (self.instantiate_done, self.signal_sent) {
            (Some(start), Some(end)) => Some(end.duration_since(start).as_nanos() as u64),
            _ => None,
        }
    }

    /// Acknowledgment latency (signal_sent → swap_acked).
    #[must_use]
    pub fn ack_duration_ns(&self) -> Option<u64> {
        match (self.signal_sent, self.swap_acked) {
            (Some(start), Some(end)) => Some(end.duration_since(start).as_nanos() as u64),
            _ => None,
        }
    }

    /// Pipeline convergence (swap_acked → first_v2_output).
    #[must_use]
    pub fn convergence_duration_ns(&self) -> Option<u64> {
        match (self.swap_acked, self.first_v2_output) {
            (Some(start), Some(end)) => Some(end.duration_since(start).as_nanos() as u64),
            _ => None,
        }
    }

    /// Total end-to-end swap time (request → first_v2_output).
    #[must_use]
    pub fn total_duration_ns(&self) -> Option<u64> {
        self.first_v2_output
            .map(|end| end.duration_since(self.request_time).as_nanos() as u64)
    }

    /// Serialize to JSON for swap_timeline.json output.
    #[must_use]
    pub fn to_json(&self) -> String {
        let fmt_opt = |opt: Option<u64>| match opt {
            Some(v) => v.to_string(),
            None => "null".to_string(),
        };

        format!(
            r#"{{
  "compile_ns": {},
  "instantiate_ns": {},
  "signal_ns": {},
  "ack_ns": {},
  "convergence_ns": {},
  "total_ns": {}
}}"#,
            fmt_opt(self.compile_duration_ns()),
            fmt_opt(self.instantiate_duration_ns()),
            fmt_opt(self.signal_duration_ns()),
            fmt_opt(self.ack_duration_ns()),
            fmt_opt(self.convergence_duration_ns()),
            fmt_opt(self.total_duration_ns()),
        )
    }
}

/// Result of a timed swap preparation (compile + instantiate).
pub struct TimedSwapResult {
    pub payload: SwapPayload,
    pub timeline: SwapTimeline,
}

/// Prepare a transform swap with timeline instrumentation.
///
/// Same as `prepare_transform_swap` but records compile and instantiate timestamps.
pub async fn prepare_transform_swap_timed(
    engine: &WaferEngine,
    wasm_bytes: &[u8],
    node_id: &str,
    capabilities: Capabilities,
) -> Result<TimedSwapResult> {
    let mut timeline = SwapTimeline::start();

    let component = engine.compile_cached(wasm_bytes)?;
    timeline.mark_compile_done();

    let pre = engine.pre_instantiate_transform(&component)?;
    let pre = Arc::new(pre);

    let mut store = Store::new(
        engine.inner(),
        WaferState::new(node_id, capabilities),
    );
    store.set_fuel(engine.fuel_limit()).map_err(|e| {
        WaferError::PluginInit { message: format!("failed to set fuel: {e}") }
    })?;
    store.epoch_deadline_trap();
    store.set_epoch_deadline(engine.epoch_deadline());

    let instance = pre.instantiate_async(&mut store).await.map_err(|e| {
        WaferError::PluginInit { message: format!("instantiation failed: {e}") }
    })?;
    timeline.mark_instantiate_done();

    let payload = SwapPayload::Transform {
        new_store: Arc::new(std::sync::Mutex::new(Some(store))),
        new_bindings: Arc::new(std::sync::Mutex::new(Some(instance))),
        new_pre: pre,
    };

    Ok(TimedSwapResult { payload, timeline })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_swap_error_display() {
        let err = SwapError::NodeNotFound("foo".to_string());
        assert_eq!(err.to_string(), "node 'foo' not found");

        let err = SwapError::NotSwappable("bar".to_string());
        assert_eq!(err.to_string(), "node 'bar' does not support hot-swap");

        let err = SwapError::WatchSendFailed("baz".to_string());
        assert_eq!(
            err.to_string(),
            "watch channel send failed for node 'baz' (task dead?)"
        );
    }

    #[test]
    fn test_swap_error_into_wafer_error() {
        let err: WaferError = SwapError::NodeNotFound("test".to_string()).into();
        match err {
            WaferError::Runtime(msg) => assert!(msg.contains("not found")),
            _ => panic!("expected Runtime error"),
        }
    }

    #[test]
    fn swap_timeline_phase_durations() {
        let mut tl = SwapTimeline::start();

        // Simulate phases with small sleeps
        std::thread::sleep(std::time::Duration::from_micros(100));
        tl.mark_compile_done();

        std::thread::sleep(std::time::Duration::from_micros(100));
        tl.mark_instantiate_done();

        std::thread::sleep(std::time::Duration::from_micros(50));
        tl.mark_signal_sent();

        std::thread::sleep(std::time::Duration::from_micros(50));
        tl.mark_swap_acked();

        std::thread::sleep(std::time::Duration::from_micros(50));
        tl.mark_first_v2_output();

        // All durations should be non-zero
        assert!(tl.compile_duration_ns().unwrap() > 0);
        assert!(tl.instantiate_duration_ns().unwrap() > 0);
        assert!(tl.signal_duration_ns().unwrap() > 0);
        assert!(tl.ack_duration_ns().unwrap() > 0);
        assert!(tl.convergence_duration_ns().unwrap() > 0);
        assert!(tl.total_duration_ns().unwrap() > 0);

        // Total should be >= sum of compile + instantiate
        let total = tl.total_duration_ns().unwrap();
        let compile = tl.compile_duration_ns().unwrap();
        let instantiate = tl.instantiate_duration_ns().unwrap();
        assert!(total >= compile + instantiate);
    }

    #[test]
    fn swap_timeline_partial_phases() {
        let mut tl = SwapTimeline::start();
        tl.mark_compile_done();

        // instantiate_done not set—should return None for later phases
        assert!(tl.compile_duration_ns().is_some());
        assert!(tl.instantiate_duration_ns().is_none());
        assert!(tl.signal_duration_ns().is_none());
        assert!(tl.total_duration_ns().is_none());
    }

    #[test]
    fn swap_timeline_to_json() {
        let mut tl = SwapTimeline::start();

        std::thread::sleep(std::time::Duration::from_micros(100));
        tl.mark_compile_done();
        std::thread::sleep(std::time::Duration::from_micros(100));
        tl.mark_instantiate_done();
        tl.mark_signal_sent();
        tl.mark_swap_acked();
        tl.mark_first_v2_output();

        let json = tl.to_json();

        // Should be valid-looking JSON with numeric values
        assert!(json.contains("\"compile_ns\""));
        assert!(json.contains("\"instantiate_ns\""));
        assert!(json.contains("\"total_ns\""));
        // No nulls since all phases are set
        assert!(!json.contains("null"));
    }

    #[test]
    fn swap_timeline_to_json_partial() {
        let tl = SwapTimeline::start();
        let json = tl.to_json();

        // Only request_time set — all values should be null
        assert!(json.contains("\"compile_ns\": null"));
        assert!(json.contains("\"total_ns\": null"));
    }
}
