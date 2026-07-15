//! Pipeline configuration domain types.
//!
//! Validation logic lives in the `wafer-config` crate.

mod engine;
mod pipeline;
mod source_sink;

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

pub use engine::{
    Capabilities, DeadLetterConfig, EngineConfig, ErrorCategory, ErrorPolicyConfig, FuelBudgets,
    OverflowPolicy, RetryConfig, SimpleAction,
};
pub use pipeline::{ApiConfig, MetricsConfig, PipelineConfig, RegistryConfig};
pub use source_sink::{
    AuthConfig, FileSinkConfig, FileSourceConfig, HttpSinkConfig, HttpSourceConfig,
    MqttSinkConfig, MqttSourceConfig, SinkDef, SourceDef, StdinSourceConfig, StdoutSinkConfig,
    TlsConfig,
};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub pipeline: Option<PipelineConfig>,

    #[serde(default)]
    pub engine: EngineConfig,

    #[serde(default)]
    pub error_policy: ErrorPolicyConfig,

    #[serde(default)]
    pub dead_letter: Option<DeadLetterConfig>,

    #[serde(default)]
    pub registry: Option<RegistryConfig>,

    #[serde(default)]
    pub api: Option<ApiConfig>,

    #[serde(default)]
    pub metrics: Option<MetricsConfig>,

    #[serde(default)]
    pub nodes: HashMap<String, NodeDef>,

    #[serde(default)]
    pub edges: Vec<EdgeDef>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum NodeDef {
    Source(SourceDef),
    Sink(SinkDef),
    Transform(WasmNodeDef),
    Filter(WasmNodeDef),
    Router(WasmNodeDef),
}

impl NodeDef {
    #[must_use]
    pub const fn category(&self) -> NodeCategory {
        match self {
            Self::Source(_) => NodeCategory::Source,
            Self::Sink(_) => NodeCategory::Sink,
            Self::Transform(_) => NodeCategory::Transform,
            Self::Filter(_) => NodeCategory::Filter,
            Self::Router(_) => NodeCategory::Router,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeCategory {
    Source,
    Sink,
    Transform,
    Filter,
    Router,
}

impl fmt::Display for NodeCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source => write!(f, "source"),
            Self::Sink => write!(f, "sink"),
            Self::Transform => write!(f, "transform"),
            Self::Filter => write!(f, "filter"),
            Self::Router => write!(f, "router"),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct WasmNodeDef {
    pub plugin: String,

    #[serde(default)]
    pub fuel: Option<u64>,

    #[serde(default)]
    pub capabilities: Capabilities,

    #[serde(default)]
    pub config: Option<toml::Value>,

    #[serde(default)]
    pub error_policy: Option<ErrorPolicyConfig>,
}



#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EdgeDef {
    pub from: String,
    pub to: String,

    /// Router output port name (only required for edges from a router node).
    #[serde(default)]
    pub port: Option<String>,

    #[serde(default)]
    pub capacity: Option<usize>,

    #[serde(default)]
    pub overflow: Option<OverflowPolicy>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_minimal_config_parses() {
        let toml_str = r#"
[nodes.in]
type = "source"
kind = "stdin"

[nodes.out]
type = "sink"
kind = "stdout"

[[edges]]
from = "in"
to = "out"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.nodes.len(), 2);
        assert_eq!(config.edges.len(), 1);
        assert!(matches!(config.nodes["in"], NodeDef::Source(SourceDef::Stdin(_))));
        assert!(matches!(config.nodes["out"], NodeDef::Sink(SinkDef::Stdout(_))));
    }

    #[test]
    fn test_full_pipeline_config_parses() {
        let toml_str = r#"
[pipeline]
name = "test-pipeline"
description = "A full test"

[engine]
epoch_deadline = 200
epoch_tick_ms = 20
default_queue_capacity = 2048

[engine.fuel]
transform = 20_000_000
filter = 1_000_000
router = 1_000_000

[error_policy]
bad_input = "skip"
timed_out = "dlq"
retry_buffer_capacity = 2000

[error_policy.dependency_failed]
retries = 5
backoff_ms = 500
exhausted = "teardown"

[error_policy.processing_failed]
retries = 1
backoff_ms = 50
exhausted = "dlq"

[dead_letter]
kind = "file"
path = "/tmp/dlq.jsonl"

[api]
enabled = true
bind = "0.0.0.0:8080"

[metrics]
enabled = false
path = "/prom"

[registry]
cache_dir = "/tmp/wafer-cache"

[nodes.mqtt-in]
type = "source"
kind = "mqtt"
broker = "localhost"
port = 1883
topic = "test/topic"

[nodes.my-filter]
type = "filter"
plugin = "plugins/filter.wasm"

[nodes.my-transform]
type = "transform"
plugin = "ghcr.io/org/transform:1.0"
fuel = 50_000_000

[nodes.my-transform.capabilities]
inherit_stdio = true
allow_inference = true

[nodes.my-router]
type = "router"
plugin = "plugins/router.wasm"

[nodes.out]
type = "sink"
kind = "stdout"

[[edges]]
from = "mqtt-in"
to = "my-filter"

[[edges]]
from = "my-filter"
to = "my-transform"
capacity = 4096
overflow = "drop"

[[edges]]
from = "my-transform"
to = "my-router"

[[edges]]
from = "my-router"
to = "out"
port = "default"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.pipeline.as_ref().unwrap().name.as_deref(), Some("test-pipeline"));
        assert_eq!(config.engine.epoch_deadline, 200);
        assert_eq!(config.engine.fuel.transform, 20_000_000);
        assert_eq!(config.error_policy.bad_input, SimpleAction::Skip);
        assert_eq!(config.error_policy.dependency_failed.retries, 5);
        assert!(config.dead_letter.is_some());
        assert_eq!(config.nodes.len(), 5);
        assert_eq!(config.edges.len(), 4);

        if let NodeDef::Transform(ref wasm) = config.nodes["my-transform"] {
            assert!(wasm.capabilities.inherit_stdio);
            assert!(wasm.capabilities.allow_inference);
            assert!(!wasm.capabilities.inherit_env);
            assert_eq!(wasm.fuel, Some(50_000_000));
        } else {
            panic!("Expected Transform node");
        }
    }

    #[test]
    fn test_error_policy_defaults() {
        let policy = ErrorPolicyConfig::default();
        assert_eq!(policy.bad_input, SimpleAction::Dlq);
        assert_eq!(policy.timed_out, SimpleAction::Skip);
        assert_eq!(policy.dependency_failed.retries, 3);
        assert_eq!(policy.dependency_failed.backoff_ms, 100);
        assert_eq!(policy.dependency_failed.exhausted, SimpleAction::Dlq);
        assert_eq!(policy.processing_failed.retries, 2);
        assert_eq!(policy.processing_failed.backoff_ms, 100);
        assert_eq!(policy.processing_failed.exhausted, SimpleAction::Dlq);
        assert_eq!(policy.retry_buffer_capacity, 1000);
    }

    #[test]
    fn test_simple_action_roundtrip() {
        for action in [SimpleAction::Skip, SimpleAction::Dlq, SimpleAction::Teardown] {
            let json = serde_json::to_string(&action).unwrap();
            let parsed: SimpleAction = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, action);
        }
    }

    #[test]
    fn test_overflow_policy_roundtrip() {
        for policy in [OverflowPolicy::Slow, OverflowPolicy::Drop, OverflowPolicy::DeadLetter] {
            let json = serde_json::to_string(&policy).unwrap();
            let parsed: OverflowPolicy = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, policy);
        }
    }

    #[test]
    fn test_error_category_display() {
        assert_eq!(ErrorCategory::BadInput.to_string(), "bad-input");
        assert_eq!(ErrorCategory::DependencyFailed.to_string(), "dependency-failed");
        assert_eq!(ErrorCategory::ProcessingFailed.to_string(), "processing-failed");
        assert_eq!(ErrorCategory::TimedOut.to_string(), "timed-out");
        assert_eq!(ErrorCategory::Unrecoverable.to_string(), "unrecoverable");
    }

    #[test]
    fn test_edge_with_port_parses() {
        let toml_str = r#"
from = "router"
to = "transform-alerts"
port = "alert"
capacity = 2048
overflow = "drop"
"#;
        let edge: EdgeDef = toml::from_str(toml_str).unwrap();
        assert_eq!(edge.from, "router");
        assert_eq!(edge.to, "transform-alerts");
        assert_eq!(edge.port.as_deref(), Some("alert"));
        assert_eq!(edge.capacity, Some(2048));
        assert_eq!(edge.overflow, Some(OverflowPolicy::Drop));
    }

    #[test]
    fn test_wasm_node_with_capabilities() {
        let toml_str = r#"
[nodes.inference]
type = "transform"
plugin = "plugins/inference.wasm"
fuel = 100_000_000

[nodes.inference.capabilities]
inherit_stdio = true
inherit_env = false
allow_inference = true

[nodes.inference.config]
model_path = "/data/model.onnx"
batch_size = 32
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        if let NodeDef::Transform(ref wasm) = config.nodes["inference"] {
            assert!(wasm.capabilities.inherit_stdio);
            assert!(!wasm.capabilities.inherit_env);
            assert!(wasm.capabilities.allow_inference);
            assert_eq!(wasm.fuel, Some(100_000_000));
            assert!(wasm.config.is_some());
        } else {
            panic!("Expected Transform node");
        }
    }

    #[test]
    fn test_dead_letter_file_variant() {
        let toml_str = r#"
kind = "file"
path = "/var/log/dlq.jsonl"
queue_capacity = 8000
"#;
        let dlq: DeadLetterConfig = toml::from_str(toml_str).unwrap();
        if let DeadLetterConfig::File { path, queue_capacity } = &dlq {
            assert_eq!(path, "/var/log/dlq.jsonl");
            assert_eq!(*queue_capacity, 8000);
        } else {
            panic!("Expected File variant");
        }
    }

    #[test]
    fn test_dead_letter_mqtt_variant() {
        let toml_str = r#"
kind = "mqtt"
broker = "broker.local"
port = 8883
topic = "dlq/messages"
queue_capacity = 20000
"#;
        let dlq: DeadLetterConfig = toml::from_str(toml_str).unwrap();
        if let DeadLetterConfig::Mqtt {
            broker,
            port,
            topic,
            queue_capacity,
            ..
        } = &dlq
        {
            assert_eq!(broker, "broker.local");
            assert_eq!(*port, 8883);
            assert_eq!(topic, "dlq/messages");
            assert_eq!(*queue_capacity, 20000);
        } else {
            panic!("Expected Mqtt variant");
        }
    }

    #[test]
    fn test_engine_config_defaults() {
        let engine = EngineConfig::default();
        assert_eq!(engine.epoch_deadline, 100);
        assert_eq!(engine.epoch_tick_ms, 10);
        assert_eq!(engine.default_queue_capacity, 1024);
        assert_eq!(engine.fuel.transform, 10_000_000);
        assert_eq!(engine.fuel.filter, 500_000);
        assert_eq!(engine.fuel.router, 500_000);
    }

    #[test]
    fn test_node_category_display() {
        assert_eq!(NodeCategory::Source.to_string(), "source");
        assert_eq!(NodeCategory::Sink.to_string(), "sink");
        assert_eq!(NodeCategory::Transform.to_string(), "transform");
        assert_eq!(NodeCategory::Filter.to_string(), "filter");
        assert_eq!(NodeCategory::Router.to_string(), "router");
    }

    #[test]
    fn test_overflow_policy_default_is_slow() {
        assert_eq!(OverflowPolicy::default(), OverflowPolicy::Slow);
    }

    #[test]
    fn test_node_def_category() {
        let toml_str = r#"
[nodes.src]
type = "source"
kind = "stdin"

[nodes.flt]
type = "filter"
plugin = "f.wasm"

[nodes.xfm]
type = "transform"
plugin = "t.wasm"

[nodes.rtr]
type = "router"
plugin = "r.wasm"

[nodes.snk]
type = "sink"
kind = "stdout"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.nodes["src"].category(), NodeCategory::Source);
        assert_eq!(config.nodes["flt"].category(), NodeCategory::Filter);
        assert_eq!(config.nodes["xfm"].category(), NodeCategory::Transform);
        assert_eq!(config.nodes["rtr"].category(), NodeCategory::Router);
        assert_eq!(config.nodes["snk"].category(), NodeCategory::Sink);
    }
}
