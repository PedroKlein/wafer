//! TOML configuration parsing for DAG pipelines.

pub mod diff;
pub mod loader;
mod schema;

pub use diff::{ConfigDiff, diff_configs};
pub use loader::load_config;
pub use schema::{
    ApiServerConfig, Config, DEFAULT_API_BIND, DEFAULT_DLQ_QUEUE_CAPACITY, DEFAULT_EPOCH_DEADLINE,
    DEFAULT_EPOCH_TICK_MS, DEFAULT_FUEL_LIMIT, DEFAULT_METRICS_BIND, DEFAULT_QUEUE_CAPACITY,
    DagConfig, DeadLetterConfig, EdgeDefinition, EngineConfig, MetricsConfig, NodeConfig,
    NodeDefinition, NodeType, OverflowPolicy, PipelineConfig,
};
