//! Hot-swap support for the new orchestrator (watch-channel based).
//!
//! The old `HotSwapCoordinator` with drain-and-flip is feature-gated under
//! `phase2-tests` — it's replaced by the simpler watch-channel model where
//! swap happens atomically between messages (no explicit drain phase needed).
//!
//! See docs/decisions/2025-07-12-orchestrator-runtime-simplification.md D6.

use std::sync::Arc;

use wasmtime::Store;

use crate::engine::WaferEngine;
use crate::engine::bindings::transform_node::{TransformNode, TransformNodePre};
use crate::engine::bindings::filter_node::{FilterNode, FilterNodePre};
use crate::engine::bindings::router_node::{RouterNode, RouterNodePre};
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

#[cfg(feature = "phase2-tests")]
pub use legacy::*;

#[cfg(feature = "phase2-tests")]
mod legacy {
    pub use super::super::hotswap_legacy::*;
}

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
}
