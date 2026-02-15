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
//! # Example
//!
//! ```ignore
//! let config = DagConfig { nodes: vec![...], edges: vec![...], ... };
//! let orchestrator = DagOrchestrator::from_config(config)?;
//! let order = orchestrator.topo_order(); // ["source", "transform", "sink"]
//! ```

mod orchestrator;

pub use orchestrator::DagOrchestrator;
