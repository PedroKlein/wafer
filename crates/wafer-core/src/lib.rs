#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr))]
//! WAFER - WebAssembly Flow Execution Runtime.

#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::return_self_not_must_use)]
#![allow(clippy::struct_excessive_bools)]

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

#[cfg(feature = "http-api")]
pub mod api;

pub use error::{RegistryError, Result, WaferError};
pub use wafer_types::*;

#[cfg(feature = "http-api")]
pub use metrics::{MetricsHandle, MetricsRegistry};
