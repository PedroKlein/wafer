//! Config module - TOML configuration parsing for DAG pipelines.

mod loader;
mod schema;

pub use loader::{load_dag_config, load_dag_config_unchecked};
pub use schema::{
    DagConfig, EdgeDefinition, NodeConfig, NodeDefinition, NodeType, DEFAULT_FUEL_LIMIT,
    DEFAULT_QUEUE_CAPACITY,
};
