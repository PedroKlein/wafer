//! DAG orchestration for WAFER pipeline.
//!
//! This module provides the core DAG (Directed Acyclic Graph) orchestration
//! for managing multi-node pipelines. The [`DagOrchestrator`] builds a graph
//! from configuration and validates the topology.
//!
//! # Design
//!
//! - Uses petgraph for graph representation (future-proofing for fan-out/fan-in)
//! - Validates: exactly one source, one sink, no cycles, no orphans
//! - Stores topological order for execution scheduling
//!
//! # Module Organization
//!
//! - [`orchestrator`]: Core struct, `run()`, and public API
//! - [`builder`]: Construction from config (`from_config`) and validation
//! - [`runner`]: Node execution loops (source, transform, sink)
//!
//! # Example
//!
//! ```ignore
//! use std::sync::Arc;
//!
//! let config = DagConfig { nodes: vec![...], edges: vec![...], ... };
//! let orchestrator = Arc::new(DagOrchestrator::from_config(config).await?);
//!
//! // Register nodes and wire queues (async with interior mutability)
//! orchestrator.register_node("source", source).await?;
//! orchestrator.wire_queues().await?;
//!
//! // Run the pipeline (takes &self, can be shared via Arc)
//! orchestrator.run().await?;
//! ```

mod builder;
mod control;
mod orchestrator;
mod runner;

pub use orchestrator::DagOrchestrator;

/// Type alias for `DagOrchestrator` - the core pipeline controller.
pub type PipelineOrchestrator = DagOrchestrator;
