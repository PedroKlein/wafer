//! Pipeline configuration loading, validation, and DAG construction.
//!
//! This crate owns parsing logic and semantic validation. The strongly-typed
//! config structs live in `wafer-types`; this crate turns TOML bytes into those
//! structs and checks that the topology makes sense before the pipeline starts.

mod dag;
mod error;
mod loader;
mod validation;

pub use dag::DagGraph;
pub use error::{ConfigError, ValidationError};
pub use loader::load_config;
pub use validation::validate;
