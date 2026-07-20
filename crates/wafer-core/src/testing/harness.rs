//! Plugin test harness — load .wasm components and call them directly.
//!
//! Provides `PluginTestHarness` for integration tests that validate Wasm plugin
//! behavior without the full pipeline machinery (no channels, no orchestrator).
//! Feature-gated behind `integration-tests` because it requires pre-built .wasm
//! artifacts on disk.
//!
//! # Usage
//!
//! ```ignore
//! let harness = PluginTestHarness::new().unwrap();
//! let mut transform = harness.load_transform("path/to/plugin.wasm").unwrap();
//! let output = transform.process(input_envelope).unwrap();
//! ```

use std::path::Path;
use std::sync::Arc;

use wasmtime::Store;

use crate::engine::state::WaferState;
use crate::engine::{Capabilities, WaferEngine};
use crate::error::Result;
use crate::node::wasm::WasmTransformNode;
use crate::queue::RuntimeEnvelope;
use crate::runner::error_policy::WasmProcessError;

/// Test harness for directly calling Wasm plugin functions.
///
/// Loads .wasm components, pre-instantiates, and provides typed call wrappers.
/// No pipeline machinery — useful for plugin validation and benchmarks.
pub struct PluginTestHarness {
    engine: WaferEngine,
}

impl PluginTestHarness {
    /// Create a new harness with a default engine.
    ///
    /// Starts the epoch ticker so fuel/epoch limits work in tests.
    ///
    /// # Errors
    ///
    /// Returns error if wasmtime engine creation fails.
    pub fn new() -> Result<Self> {
        let engine = WaferEngine::new()?;
        engine.ensure_epoch_ticker();
        Ok(Self { engine })
    }

    /// Load and instantiate a transform-node component from a file path.
    ///
    /// Performs: load → pre-instantiate → create Store → instantiate → wrap.
    ///
    /// # Errors
    ///
    /// Returns error if the file cannot be read, doesn't compile, or doesn't
    /// implement the transform-node world.
    pub fn load_transform(&self, wasm_path: impl AsRef<Path>) -> Result<TransformHarness> {
        self.load_transform_with_memory_limit(wasm_path, 64 * 1024 * 1024)
    }

    /// Same as `load_transform` but with a caller-chosen store memory limit.
    /// Useful for benchmarks that push a large number of messages through a
    /// long-lived Store.
    pub fn load_transform_with_memory_limit(
        &self,
        wasm_path: impl AsRef<Path>,
        memory_limit: usize,
    ) -> Result<TransformHarness> {
        let component = self.engine.load_component(wasm_path)?;
        let pre = self.engine.pre_instantiate_transform(&component)?;
        let pre = Arc::new(pre);

        let state = WaferState::new_with_memory_limit(
            "harness-transform",
            Capabilities::sandbox(),
            memory_limit,
        );
        let mut store = Store::new(self.engine.inner(), state);
        store.limiter(|s| s.limits_mut());
        store.epoch_deadline_trap();
        store.set_epoch_deadline(self.engine.epoch_deadline());

        let bindings = pre
            .instantiate(&mut store)
            .map_err(|e| crate::error::WaferError::PluginInit { message: e.to_string() })?;

        let node = WasmTransformNode::new(store, bindings, pre, self.engine.fuel_limit());

        Ok(TransformHarness { node })
    }

    /// Access the underlying engine (for advanced use cases).
    pub fn engine(&self) -> &WaferEngine {
        &self.engine
    }
}

/// Direct transform caller — bypasses pipeline, calls process() directly.
///
/// Thin wrapper around `WasmTransformNode` that handles the boilerplate
/// tests would otherwise repeat.
pub struct TransformHarness {
    node: WasmTransformNode,
}

impl TransformHarness {
    /// Call transform::process() with a test envelope.
    ///
    /// Returns the transformed envelope or the Wasm process error.
    pub fn process(
        &mut self,
        input: RuntimeEnvelope,
    ) -> std::result::Result<RuntimeEnvelope, WasmProcessError> {
        self.node.process(input)
    }

    /// Access the underlying node (for lifecycle calls or inspection).
    pub fn node(&self) -> &WasmTransformNode {
        &self.node
    }

    /// Mutable access to the underlying node.
    pub fn node_mut(&mut self) -> &mut WasmTransformNode {
        &mut self.node
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Path to the pre-built pass-through plugin.
    const PASS_THROUGH_WASM: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/pass-through/target/wasm32-wasip2/release/pass_through_transform.wasm"
    );

    #[test]
    fn harness_creation_succeeds() {
        let harness = PluginTestHarness::new();
        assert!(harness.is_ok(), "Harness creation failed: {:?}", harness.err());
    }

    #[test]
    fn load_transform_nonexistent_path_fails() {
        let harness = PluginTestHarness::new().unwrap();
        let result = harness.load_transform("/nonexistent/path.wasm");
        assert!(result.is_err());
    }

    #[test]
    fn pass_through_returns_same_payload() {
        if !Path::new(PASS_THROUGH_WASM).exists() {
            eprintln!("SKIP: pass-through.wasm not built (run `just build-plugin pass-through`)");
            return;
        }

        let harness = PluginTestHarness::new().unwrap();
        let mut transform = harness.load_transform(PASS_THROUGH_WASM).unwrap();

        let input = RuntimeEnvelope::from_string("test-source", "hello world");
        let output = transform.process(input).unwrap();

        assert_eq!(
            std::str::from_utf8(&output.payload).unwrap(),
            "hello world",
            "Pass-through should return payload unchanged"
        );
    }

    #[test]
    fn pass_through_preserves_source() {
        if !Path::new(PASS_THROUGH_WASM).exists() {
            eprintln!("SKIP: pass-through.wasm not built");
            return;
        }

        let harness = PluginTestHarness::new().unwrap();
        let mut transform = harness.load_transform(PASS_THROUGH_WASM).unwrap();

        let input = RuntimeEnvelope::from_string("my-source", "data");
        let output = transform.process(input).unwrap();

        assert_eq!(
            &*output.header.source, "my-source",
            "Pass-through should preserve the source field"
        );
    }

    #[test]
    fn pass_through_multiple_messages() {
        if !Path::new(PASS_THROUGH_WASM).exists() {
            eprintln!("SKIP: pass-through.wasm not built");
            return;
        }

        let harness = PluginTestHarness::new().unwrap();
        let mut transform = harness.load_transform(PASS_THROUGH_WASM).unwrap();

        for i in 0..10 {
            let msg = format!("message-{i}");
            let input = RuntimeEnvelope::from_string("src", &msg);
            let output = transform.process(input).unwrap();
            assert_eq!(std::str::from_utf8(&output.payload).unwrap(), msg);
        }
    }
}
