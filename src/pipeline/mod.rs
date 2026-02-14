//! Pipeline module - DAG execution engine.
//!
//! Orchestrates message flow through transform nodes.

mod builder;
mod executor;

pub use builder::PipelineBuilder;
pub use executor::PipelineExecutor;
