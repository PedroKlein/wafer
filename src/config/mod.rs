//! Config module - TOML configuration parsing.
//!
//! Loads and validates pipeline configuration files.
//!
//! # Example
//!
//! ```ignore
//! use wafer_poc::config::load_config;
//!
//! let config = load_config("pipeline.toml")?;
//! println!("Pipeline: {}", config.name());
//! ```

mod loader;
mod schema;

pub use loader::{load_config, load_config_unchecked};
pub use schema::{PipelineConfig, TransformConfig, DEFAULT_FUEL_LIMIT, DEFAULT_QUEUE_CAPACITY};
