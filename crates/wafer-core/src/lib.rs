//! WAFER - WebAssembly Flow Execution Runtime.

#![expect(
    clippy::module_name_repetitions,
    reason = "crate-internal modules share the crate name prefix for disambiguation"
)]
#![expect(
    clippy::doc_markdown,
    reason = "technical terms like HdrHistogram, WebAssembly are not code identifiers"
)]
#![expect(
    clippy::missing_errors_doc,
    reason = "error documentation is added incrementally; bulk requirement deferred"
)]
#![expect(
    clippy::must_use_candidate,
    reason = "most functions have clear semantics from signature; adding #[must_use] everywhere adds noise"
)]
#![expect(
    clippy::return_self_not_must_use,
    reason = "builder pattern methods return self by convention"
)]

pub mod error;

pub mod bench;
pub mod config;
pub mod dag;
pub mod dlq;
pub mod engine;
pub mod metrics;
pub mod node;
pub mod orchestrator;
pub mod queue;
pub mod registry;
pub mod runner;
pub mod testing;
pub mod util;

#[cfg(feature = "http-api")]
pub mod api;

pub use error::{RegistryError, Result, WaferError};
pub use wafer_types::*;

#[cfg(feature = "http-api")]
pub use metrics::{MetricsHandle, MetricsRegistry};
