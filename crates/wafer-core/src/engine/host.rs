//! Host state and WASI implementation for WASM component execution.

use wasmtime::component::ResourceTable;
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_nn::InMemoryRegistry;
use wasmtime_wasi_nn::backend::onnx::OnnxBackend;
use wasmtime_wasi_nn::wit::WasiNnCtx;

use super::Capabilities;

pub struct WaferState {
    ctx: WasiCtx,
    table: ResourceTable,
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

        let ctx = builder.build();

        let nn_ctx = if capabilities.allow_inference {
            Some(WasiNnCtx::new([OnnxBackend::default().into()], InMemoryRegistry::new().into()))
        } else {
            None
        };

        Self { ctx, table: ResourceTable::new(), nn_ctx }
    }

    #[must_use]
    pub fn sandboxed() -> Self {
        Self::with_capabilities(Capabilities::sandbox())
    }

    /// Returns a [`WasiNnView`] for wasi-nn host function calls.
    ///
    /// # Panics
    ///
    /// Panics if `allow_inference` was not enabled for this node's
    /// [`Capabilities`]. Wasmtime catches host-function panics via
    /// `catch_unwind` and converts them into WASM traps, so the host
    /// process is not affected. This is the idiomatic pattern used by
    /// the wasmtime CLI itself (see `serve.rs`).
    ///
    /// The `add_nn_to_linker` closure signature requires `WasiNnView`
    /// (not `Result<WasiNnView>`), so returning an error is not an option.
    #[expect(clippy::expect_used, reason = "linker closure requires WasiNnView (not Result); only called when wasi-nn capability is configured")]
    pub fn nn_view(&mut self) -> wasmtime_wasi_nn::wit::WasiNnView<'_> {
        let nn_ctx = self
            .nn_ctx
            .as_mut()
            .expect("wasi-nn called but inference capability is not enabled for this node");
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
    fn default_state_has_no_inference() {
        let state = WaferState::new();
        assert!(state.nn_ctx.is_none());
    }

    #[test]
    fn sandboxed_state_has_no_inference() {
        let state = WaferState::sandboxed();
        assert!(state.nn_ctx.is_none());
    }

    #[test]
    fn with_inference_has_nn_ctx() {
        let caps = Capabilities::sandbox().inference(true);
        let state = WaferState::with_capabilities(caps);
        assert!(state.nn_ctx.is_some());
    }
}
