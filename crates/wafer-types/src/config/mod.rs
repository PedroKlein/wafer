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
    HotSwapConfig, MemoryLimits, OverflowPolicy, RetryConfig, SimpleAction,
};
pub use pipeline::{ApiConfig, MetricsConfig, PipelineConfig, RegistryConfig};
pub use source_sink::{
    AuthConfig, BenchSinkConfigToml, BenchSourceConfigToml, FileSinkConfig, FileSourceConfig,
    HttpSinkConfig, HttpSourceConfig, MqttSinkConfig, MqttSourceConfig, SinkDef, SourceDef,
    StdinSourceConfig, StdoutSinkConfig, TlsConfig,
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

/// Plugin binding for a transform/filter/router node.
///
/// Two forms are accepted at the TOML surface:
///
/// * **Bare string shorthand.** `plugin = "path/to/plugin.wasm"` —
///   backward-compatible with every existing config. Parses as
///   [`PluginSpec::Wasm`].
/// * **Structured with `kind`.** `plugin.kind = "wasm"` (with
///   `plugin.path`) or `plugin.kind = "native"` (with
///   `plugin.function`). Selects between the Wasm sandbox and the
///   RFC-008 §D5 native Rust baseline.
///
/// The two forms are distinguished by serde's `untagged` mechanism
/// on the outer enum, and by `serde(tag = "kind")` on the inner
/// structured enum.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum PluginSpec {
    /// `plugin = "path"` shorthand — implies `kind = "wasm"`.
    WasmPath(String),
    /// `plugin = { kind = "…", … }` structured form.
    Structured(PluginSpecStructured),
}

/// Structured variants of [`PluginSpec`]. Serde discriminates on the
/// `kind` field.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PluginSpecStructured {
    /// Explicit Wasm form: `plugin = { kind = "wasm", path = "…" }`.
    /// Equivalent to the bare-string shorthand.
    Wasm {
        /// Filesystem path or OCI reference to the .wasm component.
        path: String,
    },
    /// Native baseline (RFC-008 §D5 Layer 1). No Wasm boundary, same
    /// envelope/channels. `function` selects a built-in native routine
    /// from `wafer_core::node::native::functions`.
    Native {
        /// Name of the native function: `passthrough`, `uppercase`,
        /// `json-parse` (transform); `threshold` (filter, uses
        /// `config.threshold`); `content-router` (router, uses
        /// `config.threshold` and `config.high_port`/`low_port`).
        function: String,
    },
}

impl PluginSpec {
    /// Returns the Wasm path if this spec is a Wasm binding.
    #[must_use]
    pub fn wasm_path(&self) -> Option<&str> {
        match self {
            Self::WasmPath(p) => Some(p.as_str()),
            Self::Structured(PluginSpecStructured::Wasm { path }) => Some(path.as_str()),
            Self::Structured(PluginSpecStructured::Native { .. }) => None,
        }
    }

    /// Returns the native function name if this spec is a native binding.
    #[must_use]
    pub fn native_function(&self) -> Option<&str> {
        match self {
            Self::Structured(PluginSpecStructured::Native { function }) => Some(function.as_str()),
            _ => None,
        }
    }

    /// True when this plugin binding is native (no Wasm boundary).
    #[must_use]
    pub fn is_native(&self) -> bool {
        self.native_function().is_some()
    }
}

impl Default for PluginSpec {
    fn default() -> Self {
        Self::WasmPath(String::new())
    }
}

/// Legacy alias so callers that read a Wasm path directly do not
/// have to pattern-match. Returns `""` for native plugins.
impl From<PluginSpec> for String {
    fn from(spec: PluginSpec) -> Self {
        spec.wasm_path().unwrap_or_default().to_owned()
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct WasmNodeDef {
    pub plugin: PluginSpec,

    /// Per-node fuel override. `None` = use engine default for this node type.
    /// NonZeroU64: `0` would trap on the first fuel check (P0.13 lesson).
    #[serde(default, deserialize_with = "engine::deserialize_metering_limit", serialize_with = "engine::serialize_metering_limit")]
    pub fuel: Option<std::num::NonZeroU64>,

    #[serde(default)]
    pub capabilities: Capabilities,

    #[serde(default)]
    pub config: Option<toml::Value>,

    #[serde(default)]
    pub memory_limit: Option<usize>,

    #[serde(default)]
    pub error_policy: Option<ErrorPolicyConfig>,

    /// Opaque version string for this plugin instance.
    ///
    /// Passed to the guest via `NodeConfig.plugin-version` at init time and
    /// stamped by the host into every outgoing envelope's `plugin.version`
    /// metadata. Enables `BenchSink::HotSwapRecorder` to detect the v1 → v2
    /// transition boundary during hot-swap experiments (E-Swap-1). Defaults
    /// to `""` when unset; the runtime does not enforce semver.
    #[serde(default)]
    pub plugin_version: Option<String>,
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
        assert_eq!(config.engine.epoch_deadline, std::num::NonZeroU64::new(200));
        assert_eq!(config.engine.fuel.transform, std::num::NonZeroU64::new(20_000_000));
        assert_eq!(config.error_policy.bad_input, SimpleAction::Skip);
        assert_eq!(config.error_policy.dependency_failed.retries, 5);
        assert!(config.dead_letter.is_some());
        assert_eq!(config.nodes.len(), 5);
        assert_eq!(config.edges.len(), 4);

        if let NodeDef::Transform(ref wasm) = config.nodes["my-transform"] {
            assert!(wasm.capabilities.inherit_stdio);
            assert!(wasm.capabilities.allow_inference);
            assert!(!wasm.capabilities.inherit_env);
            assert_eq!(wasm.fuel, std::num::NonZeroU64::new(50_000_000));
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
            assert_eq!(wasm.fuel, std::num::NonZeroU64::new(100_000_000));
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
        assert_eq!(engine.epoch_deadline, None);
        assert_eq!(engine.epoch_tick_ms, 10);
        assert_eq!(engine.default_queue_capacity, 1024);
        assert_eq!(engine.fuel.transform, None);
        assert_eq!(engine.fuel.filter, None);
        assert_eq!(engine.fuel.router, None);
        assert_eq!(engine.memory.transform, 64 * 1024 * 1024);
        assert_eq!(engine.memory.filter, 16 * 1024 * 1024);
        assert_eq!(engine.memory.router, 16 * 1024 * 1024);
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

    /// Bench source and sink kinds round-trip through TOML.
    #[test]
    fn bench_source_sink_kinds_roundtrip() {
        let toml_str = r#"
[nodes.src]
type = "source"
kind = "bench-source"
rate = 1000.0
total_messages = 60000
warmup_messages = 30000
payload_size = 256

[nodes.snk]
type = "sink"
kind = "bench-sink"
warmup_secs = 30
track_sequences = true
track_hotswap = false

[[edges]]
from = "src"
to = "snk"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let NodeDef::Source(SourceDef::BenchSource(src_cfg)) = &config.nodes["src"] else {
            panic!("expected BenchSource");
        };
        assert!((src_cfg.rate - 1000.0).abs() < 1e-9);
        assert_eq!(src_cfg.total_messages, 60_000);
        assert_eq!(src_cfg.warmup_messages, 30_000);
        assert_eq!(src_cfg.payload_size, 256);

        let NodeDef::Sink(SinkDef::BenchSink(snk_cfg)) = &config.nodes["snk"] else {
            panic!("expected BenchSink");
        };
        assert_eq!(snk_cfg.warmup_secs, 30);
        assert!(snk_cfg.track_sequences);
        assert!(!snk_cfg.track_hotswap);

        // Re-serialize and re-parse to verify round-trip fidelity.
        let round_trip = toml::to_string(&config).unwrap();
        let reparsed: Config = toml::from_str(&round_trip).unwrap();
        assert_eq!(reparsed.nodes.len(), 2);
        assert_eq!(reparsed.edges.len(), 1);
    }

    /// AC1 guard: `epoch_deadline = 0` is rejected at deserialization time
    /// because NonZeroU64 does not accept zero. Prevents the P0.13 footgun
    /// where a typo silently traps every Wasm call on first epoch check.
    #[test]
    fn config_epoch_zero_rejected() {
        let toml_str = r#"epoch_deadline = 0"#;
        let result: Result<EngineConfig, _> = toml::from_str(toml_str);
        let err = result.expect_err("epoch_deadline = 0 must fail deserialization");
        let msg = err.to_string();
        assert!(
            msg.contains("0") || msg.contains("zero") || msg.contains("trap"),
            "error should mention zero/trap: {msg}"
        );
    }

    /// AC1 guard: `fuel.transform = 0` is rejected at deserialization time.
    #[test]
    fn config_fuel_zero_rejected() {
        let toml_str = r#"
[fuel]
transform = 0
"#;
        let result: Result<EngineConfig, _> = toml::from_str(toml_str);
        assert!(result.is_err(), "fuel = 0 must fail deserialization");
    }

    /// AC1 guard: missing fields deserialize to None (unlimited).
    #[test]
    fn config_missing_means_none() {
        let toml_str = r#"epoch_tick_ms = 5"#;
        let config: EngineConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.epoch_deadline, None);
        assert_eq!(config.fuel.transform, None);
        assert_eq!(config.fuel.filter, None);
        assert_eq!(config.fuel.router, None);
        assert_eq!(config.epoch_tick_ms, 5);
    }
}
