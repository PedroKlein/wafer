//! Config module - TOML configuration parsing for DAG pipelines.

pub mod loader;
mod schema;

pub use loader::{load_config, load_dag_config, load_dag_config_unchecked};
pub use schema::{
    ApiServerConfig, Config, DagConfig, EdgeDefinition, MetricsConfig, NodeConfig, NodeDefinition,
    NodeType, PipelineConfig, DEFAULT_API_BIND, DEFAULT_FUEL_LIMIT, DEFAULT_METRICS_BIND,
    DEFAULT_QUEUE_CAPACITY,
};
