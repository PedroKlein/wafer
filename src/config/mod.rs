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

// Legacy load_config functions removed - use DagConfig with toml::from_str directly
pub use schema::{
    DagConfig, EdgeDefinition, NodeDefinition, NodeType, DEFAULT_FUEL_LIMIT, DEFAULT_QUEUE_CAPACITY,
};
