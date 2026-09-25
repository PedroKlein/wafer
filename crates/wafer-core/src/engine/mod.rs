//! Wasmtime integration layer: Engine, bindgen, buffer resource, cache, state.

pub mod bindings;
mod buffer;
mod cache;
mod capabilities;
mod loader;
pub(crate) mod state;

pub use bindings::WasmBindings;
pub use buffer::WaferBuffer;
pub use cache::ComponentCache;
pub use capabilities::Capabilities;
pub use loader::WaferEngine;
pub use state::{LogEntry, LogLevel, WaferState};

#[doc(hidden)]
pub type LegacyWaferState = state::WaferState;
