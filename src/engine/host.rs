//! Host state and WASI implementation.

use wasmtime::component::ResourceTable;
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

/// Capability configuration for WASM plugins.
///
/// Controls what system resources a plugin can access.
/// By default, plugins get minimal capabilities (sandbox mode).
///
/// # MVP Limitations
///
/// Currently only `inherit_stdio` and `inherit_env` are functional.
/// The `allow_network` and `allow_filesystem` fields are **placeholders**
/// for future capability-based security and have no effect in this MVP.
///
/// Future versions will support:
/// - Fine-grained filesystem access (allowlist of paths)
/// - Network access control (allowlist of hosts/ports)
/// - Resource limits (memory, CPU time)
#[derive(Debug, Clone, Default)]
pub struct Capabilities {
    /// Allow inheriting stdin/stdout/stderr from the host process.
    pub inherit_stdio: bool,
    /// Allow access to environment variables.
    pub inherit_env: bool,
    /// Allow network access.
    ///
    /// **MVP: Placeholder only - has no effect.**
    /// Future: will support specific hosts/ports allowlist.
    pub allow_network: bool,
    /// Allow filesystem access.
    ///
    /// **MVP: Placeholder only - has no effect.**
    /// Future: will support specific paths allowlist.
    pub allow_filesystem: bool,
}

impl Capabilities {
    /// Create default capabilities (minimal sandbox).
    #[must_use]
    pub fn sandbox() -> Self {
        Self::default()
    }

    /// Create capabilities that inherit stdio (for debugging).
    #[must_use]
    pub fn with_stdio() -> Self {
        Self {
            inherit_stdio: true,
            ..Default::default()
        }
    }

    /// Create full capabilities (for trusted plugins).
    #[must_use]
    pub fn full() -> Self {
        Self {
            inherit_stdio: true,
            inherit_env: true,
            allow_network: true,
            allow_filesystem: true,
        }
    }

    /// Enable stdio inheritance (builder pattern).
    #[must_use]
    pub fn stdio(mut self, enabled: bool) -> Self {
        self.inherit_stdio = enabled;
        self
    }

    /// Enable environment variable access (builder pattern).
    #[must_use]
    pub fn env(mut self, enabled: bool) -> Self {
        self.inherit_env = enabled;
        self
    }

    /// Enable network access (builder pattern).
    ///
    /// **MVP: Placeholder only - has no effect.**
    #[must_use]
    pub fn network(mut self, enabled: bool) -> Self {
        self.allow_network = enabled;
        self
    }

    /// Enable filesystem access (builder pattern).
    ///
    /// **MVP: Placeholder only - has no effect.**
    #[must_use]
    pub fn filesystem(mut self, enabled: bool) -> Self {
        self.allow_filesystem = enabled;
        self
    }
}

/// Host state for WASM component execution.
/// Implements WasiView to provide WASI capabilities.
pub struct WaferState {
    ctx: WasiCtx,
    table: ResourceTable,
    /// Retained for future capability inspection/auditing.
    /// Currently unused but will be used for runtime capability queries.
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
