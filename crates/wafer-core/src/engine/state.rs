//! Host state stored in each Wasmtime `Store<WaferState>`.
//!
//! `WaferState` is the `T` in `Store<T>`. It carries WAFER-specific context:
//! WASI sandbox, resource table for `WaferBuffer` handles, optional wasi-nn,
//! per-call log buffer for the `pipeline:host/logging` import, per-node memory
//! limits, and the node identity for structured logging.
//!
//! See docs/rfcs/RFC-002-host-runtime.md D8
//! and docs/rfcs/RFC-007-performance-optimizations.md (StoreLimits amendment).

use wasmtime::component::ResourceTable;
use wasmtime::{StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use super::Capabilities;

/// Default per-node memory limit (16 MiB).
/// Constrains guest linear memory growth — OOM is contained to a single node (RQ2).
const DEFAULT_MEMORY_LIMIT: usize = 16 * 1024 * 1024;

/// Default per-node table element limit.
const DEFAULT_TABLE_ELEMENTS: usize = 20_000;

/// A single buffered log entry from a guest call to `pipeline:host/logging`.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub level: LogLevel,
    pub message: String,
}

/// Log level matching the WIT `log-level` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// Host state for a single pipeline node's Wasmtime Store.
///
/// One `WaferState` per node. Store is `Send` but NOT `Sync` — lives on one
/// tokio task for the node's lifetime.
pub struct WaferState {
    /// WASI Preview-2 sandbox context.
    ctx: WasiCtx,
    /// Resource table for `WaferBuffer` handles + WASI resources.
    table: ResourceTable,
    /// Per-node memory limits enforced by wasmtime's `ResourceLimiter`.
    limits: StoreLimits,
    /// Buffered log messages from the current Wasm call.
    /// Cleared before each call, flushed to tracing after.
    log_buffer: Vec<LogEntry>,
    /// Node identity for structured logging context.
    node_id: Box<str>,
}

impl WaferState {
    /// Create a new state with the given capabilities and node identity.
    ///
    /// # COLD PATH — called once per node initialization.
    #[must_use]
    pub fn new(node_id: impl Into<Box<str>>, capabilities: Capabilities) -> Self {
        Self::new_with_memory_limit(node_id, capabilities, DEFAULT_MEMORY_LIMIT)
    }

    /// Create a new state with an explicit guest memory limit.
    #[must_use]
    pub fn new_with_memory_limit(
        node_id: impl Into<Box<str>>,
        capabilities: Capabilities,
        memory_limit: usize,
    ) -> Self {
        let mut builder = WasiCtxBuilder::new();

        if capabilities.inherit_stdio {
            builder.inherit_stdio();
        }
        if capabilities.inherit_env {
            builder.inherit_env();
        }

        let ctx = builder.build();

        let limits = StoreLimitsBuilder::new()
            .memory_size(memory_limit)
            .table_elements(DEFAULT_TABLE_ELEMENTS)
            .trap_on_grow_failure(true)
            .build();

        Self {
            ctx,
            table: ResourceTable::new(),
            limits,
            log_buffer: Vec::with_capacity(16),
            node_id: node_id.into(),
        }
    }

    /// Create a sandboxed state (no capabilities) with the given node ID.
    #[must_use]
    pub fn sandboxed(node_id: impl Into<Box<str>>) -> Self {
        Self::new(node_id, Capabilities::sandbox())
    }

    /// Get the node identity.
    #[inline]
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// Access the resource table (for pushing/deleting `WaferBuffer` handles).
    #[inline]
    pub fn table(&self) -> &ResourceTable {
        &self.table
    }

    /// Mutable access to the resource table.
    #[inline]
    pub fn table_mut(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    /// Access the store limits (for `Store::limiter`).
    #[inline]
    pub fn limits(&self) -> &StoreLimits {
        &self.limits
    }

    /// Mutable access to the store limits (for `Store::limiter`).
    #[inline]
    pub fn limits_mut(&mut self) -> &mut StoreLimits {
        &mut self.limits
    }

    /// Clear the log buffer before a new Wasm call.
    #[inline]
    pub fn clear_log_buffer(&mut self) {
        self.log_buffer.clear();
    }

    /// Push a log entry (called by the `pipeline:host/logging` host impl).
    #[inline]
    pub fn push_log(&mut self, level: LogLevel, message: String) {
        self.log_buffer.push(LogEntry { level, message });
    }

    /// Drain and return all buffered log entries (for flushing to tracing).
    #[inline]
    pub fn drain_logs(&mut self) -> std::vec::Drain<'_, LogEntry> {
        self.log_buffer.drain(..)
    }

    /// Check if there are buffered log entries.
    #[inline]
    pub fn has_logs(&self) -> bool {
        !self.log_buffer.is_empty()
    }

    /// Push a `WaferBuffer` into the ResourceTable and return its handle.
    ///
    /// The `borrow<buffer>` WIT semantic leaves ownership with the host;
    /// the caller MUST invoke [`Self::delete_buffer`] once the guest returns
    /// or the table grows one Bytes-clone entry per call.
    ///
    /// # Errors
    ///
    /// Returns error if the ResourceTable is full (2^32 slots — unreachable
    /// in practice).
    pub fn push_buffer(
        &mut self,
        data: bytes::Bytes,
    ) -> wasmtime::Result<wasmtime::component::Resource<super::WaferBuffer>> {
        let buffer = super::WaferBuffer::new(data);
        let resource = self.table.push(buffer)?;
        Ok(resource)
    }

    /// Remove a buffer resource from the ResourceTable.
    ///
    /// Pair of [`Self::push_buffer`] — must be called after every guest
    /// call that used a buffer produced by push_buffer. See push_buffer
    /// for the leak-behaviour rationale.
    ///
    /// # Errors
    ///
    /// Returns error if the handle is invalid (already deleted or from
    /// another table) — always a host-side programmer error.
    pub fn delete_buffer(
        &mut self,
        resource: wasmtime::component::Resource<super::WaferBuffer>,
    ) -> wasmtime::Result<()> {
        self.table.delete(resource).map(|_| ()).map_err(Into::into)
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
    fn new_state_has_empty_log_buffer() {
        let state = WaferState::sandboxed("test-node");
        assert!(!state.has_logs());
        assert_eq!(state.node_id(), "test-node");
    }

    #[test]
    fn push_and_drain_logs() {
        let mut state = WaferState::sandboxed("test-node");

        state.push_log(LogLevel::Info, "hello".to_string());
        state.push_log(LogLevel::Warn, "warning".to_string());
        assert!(state.has_logs());

        let logs: Vec<_> = state.drain_logs().collect();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].level, LogLevel::Info);
        assert_eq!(logs[0].message, "hello");
        assert_eq!(logs[1].level, LogLevel::Warn);
        assert_eq!(logs[1].message, "warning");

        assert!(!state.has_logs());
    }

    #[test]
    fn clear_log_buffer() {
        let mut state = WaferState::sandboxed("test-node");
        state.push_log(LogLevel::Error, "oops".to_string());
        assert!(state.has_logs());

        state.clear_log_buffer();
        assert!(!state.has_logs());
    }

    #[test]
    fn node_id_stored_as_box_str() {
        let state = WaferState::new("my-transform-1".to_string(), Capabilities::sandbox());
        assert_eq!(state.node_id(), "my-transform-1");
    }

    #[test]
    fn capabilities_applied() {
        // Sandboxed: no capabilities
        let _state = WaferState::sandboxed("node");
        // With stdio: shouldn't panic
        let _state = WaferState::new("node", Capabilities::with_stdio());
    }

    #[test]
    fn resource_table_accessible() {
        let mut state = WaferState::sandboxed("node");
        // Table should be empty initially
        let _table = state.table_mut();
    }

    #[test]
    fn push_buffer_creates_resource() {
        let mut state = WaferState::sandboxed("test-node");
        let data = bytes::Bytes::from_static(b"hello world");

        let resource = state.push_buffer(data).expect("push_buffer should succeed");

        // Resource handle should be retrievable from the table
        let buf = state.table().get(&resource).expect("buffer should be in table");
        assert_eq!(buf.size(), 11);
        assert_eq!(buf.read_all(), b"hello world");
    }

    #[test]
    fn push_buffer_then_delete() {
        let mut state = WaferState::sandboxed("test-node");
        let data = bytes::Bytes::from_static(b"payload");

        let resource = state.push_buffer(data).expect("push_buffer should succeed");

        // Delete the resource (simulating post-call cleanup)
        state.table_mut().delete(resource).expect("delete should succeed");
    }
}
