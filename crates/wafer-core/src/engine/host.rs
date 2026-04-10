//! Host state and WASI implementation for WASM component execution.

use wasmtime::component::ResourceTable;
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_nn::backend::onnx::OnnxBackend;
use wasmtime_wasi_nn::wit::WasiNnCtx;
use wasmtime_wasi_nn::InMemoryRegistry;

use super::Capabilities;

/// Host state for WASM component execution.
///
/// Implements `WasiView` to provide WASI capabilities to guest components.
pub struct WaferState {
    ctx: WasiCtx,
    table: ResourceTable,
    /// Retained for future capability inspection/auditing.
    #[expect(dead_code, reason = "retained for future capability inspection")]
    capabilities: Capabilities,
    nn_ctx: Option<WasiNnCtx>,
}

impl WaferState {
    #[must_use]
    pub fn new() -> Self {
        Self::with_capabilities(Capabilities::with_stdio())
    }

    #[must_use]
    pub fn with_capabilities(capabilities: Capabilities) -> Self {
        let mut builder = WasiCtxBuilder::new();

        if capabilities.inherit_stdio {
            builder.inherit_stdio();
        }

        if capabilities.inherit_env {
            builder.inherit_env();
        }

        // Network and filesystem capabilities are placeholders for future use

        let ctx = builder.build();

        let nn_ctx = if capabilities.allow_inference {
            Some(WasiNnCtx::new([OnnxBackend::default().into()], InMemoryRegistry::new().into()))
        } else {
            None
        };

        Self { ctx, table: ResourceTable::new(), capabilities, nn_ctx }
    }

    #[must_use]
    pub fn sandboxed() -> Self {
        Self::with_capabilities(Capabilities::sandbox())
    }

    /// # Panics
    ///
    /// Panics if inference capability was not enabled.
    pub fn nn_view(&mut self) -> wasmtime_wasi_nn::wit::WasiNnView<'_> {
        let nn_ctx = self.nn_ctx.as_mut().expect("inference not enabled");
        wasmtime_wasi_nn::wit::WasiNnView::new(&mut self.table, nn_ctx)
    }
}

impl Default for WaferState {
    fn default() -> Self {
        Self::new()
    }
}

impl WasiView for WaferState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.ctx, table: &mut self.table }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_state_has_stdio() {
        let state = WaferState::new();
        // Can't easily inspect WasiCtx, but we can verify it doesn't panic
        assert!(state.nn_ctx.is_none());
    }

    #[test]
    fn test_sandboxed_state() {
        let state = WaferState::sandboxed();
        assert!(state.nn_ctx.is_none());
    }

    #[test]
    fn test_with_inference() {
        let caps = Capabilities::sandbox().inference(true);
        let state = WaferState::with_capabilities(caps);
        assert!(state.nn_ctx.is_some());
    }
}
