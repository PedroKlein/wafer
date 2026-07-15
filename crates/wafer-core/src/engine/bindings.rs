//! Wasmtime [`bindgen!`] outputs for the three WAFER pipeline worlds.
//!
//! Each world (transform-node, filter-node, router-node) gets its own submodule.
//! The first invocation (`transform_node`) generates the canonical
//! `pipeline:types/types` interface bindings; subsequent invocations redirect
//! to that module via `with:` so `WaferBuffer` impls are deduplicated and
//! `WaferState` only needs one `HostBuffer` implementation.
//!
//! Directory layout:
//!   - `wit/node/` — main package `pipeline:node` (transform-node + filter-node worlds)
//!   - `wit/router/` — main package `pipeline:routing` (router-node world)
//!   - Each has `deps/` subdirectories for cross-package references.
//!
//! [`bindgen!`]: wasmtime::component::bindgen

/// Bindings for the `transform-node` world (package `pipeline:node`).
///
/// This is the "canonical" invocation that generates the `pipeline:types/types`
/// host trait definitions. Other worlds redirect to these types via `with:`.
pub(crate) mod transform_node {
    wasmtime::component::bindgen!({
        path: "wit/node",
        world: "transform-node",
        with: {
            "pipeline:types/types.buffer": crate::engine::WaferBuffer,
        },
    });
}

/// Bindings for the `filter-node` world (package `pipeline:node`).
///
/// Reuses `pipeline:types/types` from the transform-node bindings via `with:`.
pub(crate) mod filter_node {
    wasmtime::component::bindgen!({
        path: "wit/node",
        world: "filter-node",
        with: {
            "pipeline:types/types": super::transform_node::pipeline::types::types,
            "pipeline:host/logging": super::transform_node::pipeline::host::logging,
        },
    });
}

/// Bindings for the `router-node` world (package `pipeline:routing`).
///
/// Reuses `pipeline:types/types` and `pipeline:node/lifecycle` from previous bindings.
pub(crate) mod router_node {
    wasmtime::component::bindgen!({
        path: "wit/router",
        world: "router-node",
        with: {
            "pipeline:types/types": super::transform_node::pipeline::types::types,
            "pipeline:host/logging": super::transform_node::pipeline::host::logging,
            "pipeline:node/lifecycle": super::transform_node::exports::pipeline::node::lifecycle,
        },
    });
}

/// Discriminator for which world a compiled component targets.
///
/// Used by `WaferEngine` to create the correct `InstancePre` variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WasmBindings {
    Transform,
    Filter,
    Router,
}

impl std::fmt::Display for WasmBindings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transform => write!(f, "transform-node"),
            Self::Filter => write!(f, "filter-node"),
            Self::Router => write!(f, "router-node"),
        }
    }
}

// =============================================================================
// Host trait implementations for WaferState
//
// bindgen! generates:
//   - `pipeline::types::types::HostBuffer` — resource methods (size, read, read_all)
//   - `pipeline::types::types::Host` — marker trait for the types interface
//   - `pipeline::host::logging::Host` — log(level, message) function
//
// These are implemented on WaferState since Store<WaferState> is the store type.
// All three worlds share the same canonical traits via `with:` directives above.
// =============================================================================

use wasmtime::component::Resource;

use super::WaferBuffer;
use super::state::{LogLevel, WaferState};

/// Re-export the generated host trait paths for use in linker setup.
pub use transform_node::pipeline::host::logging;
pub use transform_node::pipeline::types::types as host_types;

/// Marker trait — the `pipeline:types/types` interface has no free functions,
/// only the `buffer` resource. bindgen generates an empty `Host` trait.
impl host_types::Host for WaferState {}

/// Host implementation for the `resource buffer` methods.
///
/// Each method receives a `Resource<WaferBuffer>` handle that the guest obtained
/// from the `borrow<buffer>` in the `message` record. The ResourceTable maps
/// the handle to the underlying `WaferBuffer`.
///
/// The Component Model guarantees borrow handles are valid for the duration of
/// the call — `table.get()` can only fail if there's a host-side bug.
#[expect(clippy::expect_used, reason = "CM guarantees borrow handles are valid during call; failure = host bug")]
impl host_types::HostBuffer for WaferState {
    /// Returns the total byte length of the payload.
    fn size(&mut self, resource: Resource<WaferBuffer>) -> u64 {
        let buf = self.table().get(&resource).expect("CM invariant: borrow handle valid during call");
        buf.size()
    }

    /// Reads a slice of the payload. Returns fewer bytes if offset+len exceeds size.
    fn read(
        &mut self,
        resource: Resource<WaferBuffer>,
        offset: u64,
        len: u64,
    ) -> Vec<u8> {
        let buf = self.table().get(&resource).expect("CM invariant: borrow handle valid during call");
        buf.read(offset, len)
    }

    /// Reads the entire payload in one call.
    fn read_all(&mut self, resource: Resource<WaferBuffer>) -> Vec<u8> {
        let buf = self.table().get(&resource).expect("CM invariant: borrow handle valid during call");
        buf.read_all()
    }

    /// Called by the Component Model when a `borrow<buffer>` goes out of scope.
    /// Removes the resource from the table, freeing the underlying Bytes handle.
    fn drop(&mut self, resource: Resource<WaferBuffer>) -> wasmtime::Result<()> {
        self.table_mut().delete(resource)?;
        Ok(())
    }
}

/// Host implementation for `pipeline:host/logging` — guest log emission.
///
/// Pushes log entries to WaferState's per-call buffer. The runner loop
/// drains this buffer after each Wasm call and emits via the tracing crate.
impl logging::Host for WaferState {
    fn log(
        &mut self,
        level: host_types::LogLevel,
        message: String,
    ) {
        // Map WIT log-level enum to our internal LogLevel
        let internal_level = match level {
            host_types::LogLevel::Trace => LogLevel::Trace,
            host_types::LogLevel::Debug => LogLevel::Debug,
            host_types::LogLevel::Info => LogLevel::Info,
            host_types::LogLevel::Warn => LogLevel::Warn,
            host_types::LogLevel::Error => LogLevel::Error,
        };
        self.push_log(internal_level, message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    use super::host_types::HostBuffer;
    use super::logging::Host as LoggingHost;
    use crate::engine::state::LogLevel as InternalLogLevel;

    /// Helper to create a WaferState with a buffer pushed into the ResourceTable.
    fn state_with_buffer(data: &[u8]) -> (WaferState, Resource<WaferBuffer>) {
        let mut state = WaferState::sandboxed("test-node");
        let resource = state.push_buffer(Bytes::from(data.to_vec())).unwrap();
        (state, resource)
    }

    #[test]
    fn host_buffer_size_returns_payload_length() {
        let (mut state, resource) = state_with_buffer(b"hello world");
        assert_eq!(HostBuffer::size(&mut state, resource), 11);
    }

    #[test]
    fn host_buffer_read_returns_slice() {
        let (mut state, resource) = state_with_buffer(b"hello world");
        let slice = HostBuffer::read(&mut state, resource, 0, 5);
        assert_eq!(slice, b"hello");
    }

    #[test]
    fn host_buffer_read_with_offset() {
        let (mut state, resource) = state_with_buffer(b"hello world");
        let slice = HostBuffer::read(&mut state, resource, 6, 5);
        assert_eq!(slice, b"world");
    }

    #[test]
    fn host_buffer_read_clamped() {
        let (mut state, resource) = state_with_buffer(b"hi");
        // Request more than available
        let slice = HostBuffer::read(&mut state, resource, 0, 100);
        assert_eq!(slice, b"hi");
    }

    #[test]
    fn host_buffer_read_all_returns_full_payload() {
        let (mut state, resource) = state_with_buffer(b"full payload");
        assert_eq!(HostBuffer::read_all(&mut state, resource), b"full payload");
    }

    #[test]
    fn host_buffer_drop_removes_from_table() {
        let (mut state, resource) = state_with_buffer(b"data");
        // Drop should succeed
        HostBuffer::drop(&mut state, resource).unwrap();
    }

    #[test]
    fn logging_host_pushes_to_buffer() {
        let mut state = WaferState::sandboxed("test-node");

        LoggingHost::log(&mut state, host_types::LogLevel::Info, "hello from guest".to_string());
        LoggingHost::log(&mut state, host_types::LogLevel::Warn, "something odd".to_string());

        assert!(state.has_logs());
        let logs: Vec<_> = state.drain_logs().collect();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].level, InternalLogLevel::Info);
        assert_eq!(logs[0].message, "hello from guest");
        assert_eq!(logs[1].level, InternalLogLevel::Warn);
        assert_eq!(logs[1].message, "something odd");
    }

    #[test]
    fn logging_all_levels_mapped_correctly() {
        let mut state = WaferState::sandboxed("test-node");

        let levels = [
            (host_types::LogLevel::Trace, InternalLogLevel::Trace),
            (host_types::LogLevel::Debug, InternalLogLevel::Debug),
            (host_types::LogLevel::Info, InternalLogLevel::Info),
            (host_types::LogLevel::Warn, InternalLogLevel::Warn),
            (host_types::LogLevel::Error, InternalLogLevel::Error),
        ];

        for (wit_level, expected_level) in levels {
            state.clear_log_buffer();
            LoggingHost::log(&mut state, wit_level, "test".to_string());
            let logs: Vec<_> = state.drain_logs().collect();
            assert_eq!(logs[0].level, expected_level);
        }
    }
}
