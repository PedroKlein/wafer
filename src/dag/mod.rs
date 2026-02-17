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
//! let config = DagConfig { nodes: vec![...], edges: vec![...], ... };
//! let mut orchestrator = DagOrchestrator::from_config(config)?;
//!
//! // Register nodes and wire queues
//! orchestrator.register_node("source", source)?;
//! orchestrator.wire_queues()?;
//!
//! // Run the pipeline
//! orchestrator.run().await?;
//! ```

mod builder;
mod orchestrator;
mod runner;

pub use orchestrator::DagOrchestrator;
