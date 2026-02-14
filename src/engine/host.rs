//! Host state and WASI implementation.

use wasmtime::component::ResourceTable;
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiView};

/// Host state for WASM component execution.
/// Implements WasiView to provide WASI capabilities.
pub struct WaferState {
    ctx: WasiCtx,
    table: ResourceTable,
}

impl WaferState {
    pub fn new() -> Self {
        let ctx = WasiCtxBuilder::new().inherit_stdio().build();

        Self {
            ctx,
            table: ResourceTable::new(),
        }
    }
}

impl Default for WaferState {
    fn default() -> Self {
        Self::new()
    }
}

impl WasiView for WaferState {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.ctx
    }
}
