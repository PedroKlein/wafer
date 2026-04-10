//! TOML configuration parsing for DAG pipelines.

pub mod diff;
pub mod loader;
mod schema;

pub use diff::{diff_configs, ConfigDiff};
pub use loader::{load_config, load_dag_config, load_dag_config_unchecked};
pub use schema::{
    ApiServerConfig, Config, DagConfig, DeadLetterConfig, EdgeDefinition, MetricsConfig,
    NodeConfig, NodeDefinition, NodeType, OverflowPolicy, PipelineConfig, DEFAULT_API_BIND,
    DEFAULT_DLQ_QUEUE_CAPACITY, DEFAULT_FUEL_LIMIT, DEFAULT_METRICS_BIND, DEFAULT_QUEUE_CAPACITY,
};
