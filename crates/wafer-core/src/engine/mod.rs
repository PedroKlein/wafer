//! Wasmtime integration layer: Engine, bindgen, buffer resource, cache, state.

mod bindings;
mod buffer;
mod cache;
mod capabilities;
mod host;
mod instance;
mod loader;
mod state;

pub use bindings::WasmBindings;
pub use buffer::WaferBuffer;
pub use cache::ComponentCache;
pub use capabilities::Capabilities;
pub use host::WaferState as LegacyWaferState;
pub use instance::TransformInstance;
pub use loader::WaferEngine;
pub use state::{LogEntry, LogLevel, WaferState};
