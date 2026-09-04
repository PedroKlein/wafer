use serde::{Deserialize, Serialize};

use super::engine::{default_mqtt_port, default_true};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SourceDef {
    Mqtt(MqttSourceConfig),
    File(FileSourceConfig),
    Stdin(StdinSourceConfig),
    Http(HttpSourceConfig),
    /// Deterministic in-process benchmark source. Emits `total_messages`
    /// envelopes at `rate` msg/s, with the first `warmup_messages` treated
    /// as warm-up by downstream `BenchSink`s. See
    /// `wafer_core::node::source::BenchSource`.
    BenchSource(BenchSourceConfigToml),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SinkDef {
    Mqtt(MqttSinkConfig),
    File(FileSinkConfig),
    Stdout(StdoutSinkConfig),
    Http(HttpSinkConfig),
    /// Evaluation-grade measurement sink with [`HdrHistogram`], sequence
    /// tracking, hot-swap boundary detection, and auto-export on close.
    /// See `wafer_core::node::sink::BenchSink`.
    BenchSink(BenchSinkConfigToml),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MqttSourceConfig {
    pub broker: String,

    #[serde(default = "default_mqtt_port")]
    pub port: u16,

    pub topic: String,

    #[serde(default)]
    pub qos: u8,

    #[serde(default)]
    pub client_id: Option<String>,

    #[serde(default)]
    pub tls: Option<TlsConfig>,

    #[serde(default)]
    pub auth: Option<AuthConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MqttSinkConfig {
    pub broker: String,

    #[serde(default = "default_mqtt_port")]
    pub port: u16,

    pub topic: String,

    #[serde(default)]
    pub qos: u8,

    #[serde(default)]
    pub client_id: Option<String>,

    #[serde(default)]
    pub retain: bool,

    #[serde(default)]
    pub tls: Option<TlsConfig>,

    #[serde(default)]
    pub auth: Option<AuthConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FileSourceConfig {
    pub path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FileSinkConfig {
    pub path: String,

    #[serde(default = "default_true")]
    pub append: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct StdinSourceConfig {}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct StdoutSinkConfig {}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HttpSourceConfig {
    #[serde(default = "default_http_source_bind")]
    pub bind: String,

    #[serde(default = "default_http_source_path")]
    pub path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HttpSinkConfig {
    pub url: String,

    #[serde(default = "default_http_method")]
    pub method: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TlsConfig {
    #[serde(default)]
    pub ca: Option<String>,

    #[serde(default)]
    pub cert: Option<String>,

    #[serde(default)]
    pub key: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AuthConfig {
    pub username: String,
    pub password: String,
}

fn default_http_source_bind() -> String {
    "127.0.0.1:8081".to_owned()
}

fn default_http_source_path() -> String {
    "/webhook".to_owned()
}

fn default_http_method() -> String {
    "POST".to_owned()
}

/// Config-file form of `wafer_core::node::source::BenchSourceConfig`.
///
/// Kept as a distinct type so the TOML schema stays stable when the runtime
/// side adds internal knobs. `From<BenchSourceConfigToml>` in wafer-core
/// bridges the two.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BenchSourceConfigToml {
    /// Target emission rate (msg/s).
    pub rate: f64,

    /// Total number of messages to emit. When reached, the source signals EOF.
    pub total_messages: u64,

    /// Number of leading messages tagged as warm-up (`bench.warmup=true` in
    /// envelope metadata). Defaults to zero — use for [`BenchSink`] warmup
    /// exclusion.
    #[serde(default)]
    pub warmup_messages: u64,

    /// Payload size in bytes. The source emits `payload_size` bytes of a
    /// deterministic filler pattern (see `BenchSource::with_payload_size`).
    #[serde(default = "default_bench_payload_size")]
    pub payload_size: usize,

    /// Optional one-burst schedule relative to the first measured message.
    #[serde(default)]
    pub burst: Option<BenchBurstConfigToml>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BenchBurstConfigToml {
    pub rate: f64,
    pub start_secs: u64,
    pub end_secs: u64,
}

/// Config-file form of `wafer_core::node::sink::BenchSinkConfig`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BenchSinkConfigToml {
    /// Seconds to discard at start (warm-up exclusion). Applied on the
    /// receiving side of the sink.
    #[serde(default = "default_warmup_secs")]
    pub warmup_secs: u64,

    /// Enable sequence gap/duplicate tracking.
    #[serde(default = "default_true")]
    pub track_sequences: bool,

    /// Enable hot-swap version transition recording (`plugin.version`
    /// metadata).
    #[serde(default)]
    pub track_hotswap: bool,

    /// Output directory for `latency.hdr` + `throughput.csv` on `close()`.
    /// Left `None` means no auto-export; the eval scripts provide this.
    #[serde(default)]
    pub output_dir: Option<String>,
}

const fn default_bench_payload_size() -> usize {
    128
}

const fn default_warmup_secs() -> u64 {
    30
}
