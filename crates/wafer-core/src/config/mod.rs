//! Configuration domain types shared by the runtime.
//!
//! Parsing and validation live in `wafer-config`; `wafer-core` consumes the
//! validated `wafer-types` schema directly.

pub use wafer_types::config::*;

pub const DEFAULT_QUEUE_CAPACITY: usize = 1024;
pub const DEFAULT_EPOCH_TICK_MS: u64 = 10;
