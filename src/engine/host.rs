//! Host state and WASI implementation.

use wasmtime::component::ResourceTable;
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

/// Capability configuration for WASM plugins.
///
/// Controls what system resources a plugin can access.
/// By default, plugins get minimal capabilities (sandbox mode).
#[derive(Debug, Clone, Default)]
pub struct Capabilities {
    /// Allow inheriting stdin/stdout/stderr from the host process.
    pub inherit_stdio: bool,
    /// Allow access to environment variables.
    pub inherit_env: bool,
    /// Allow network access (future: specific hosts/ports).
    pub allow_network: bool,
    /// Allow filesystem access (future: specific paths).
    pub allow_filesystem: bool,
}

impl Capabilities {
    /// Create default capabilities (minimal sandbox).
    pub fn sandbox() -> Self {
        Self::default()
    }

    /// Create capabilities that inherit stdio (for debugging).
    pub fn with_stdio() -> Self {
        Self {
            inherit_stdio: true,
            ..Default::default()
        }
    }

    /// Create full capabilities (for trusted plugins).
    pub fn full() -> Self {
        Self {
            inherit_stdio: true,
            inherit_env: true,
            allow_network: true,
            allow_filesystem: true,
        }
    }
}

/// Host state for WASM component execution.
/// Implements WasiView to provide WASI capabilities.
pub struct WaferState {
    ctx: WasiCtx,
    table: ResourceTable,
    #[allow(dead_code)]
    capabilities: Capabilities,
}

impl WaferState {
    /// Create a new host state with default capabilities (inherit stdio).
    pub fn new() -> Self {
        Self::with_capabilities(Capabilities::with_stdio())
    }

    /// Create a new host state with specific capabilities.
    pub fn with_capabilities(capabilities: Capabilities) -> Self {
        let mut builder = WasiCtxBuilder::new();

        if capabilities.inherit_stdio {
            builder.inherit_stdio();
        }

        if capabilities.inherit_env {
            builder.inherit_env();
        }

        // Note: Network and filesystem capabilities require additional
        // configuration with specific paths/hosts. For now, these flags
        // are placeholders for future capability-based security.

        let ctx = builder.build();

        Self {
            ctx,
            table: ResourceTable::new(),
            capabilities,
        }
    }

    /// Create a sandboxed state with no host access.
    pub fn sandboxed() -> Self {
        Self::with_capabilities(Capabilities::sandbox())
    }
}

impl Default for WaferState {
    fn default() -> Self {
        Self::new()
    }
}

impl WasiView for WaferState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.ctx,
            table: &mut self.table,
        }
    }
}
