use serde::{Deserialize, Serialize};

use super::engine::{default_mqtt_port, default_true};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SourceDef {
    Mqtt(MqttSourceConfig),
    File(FileSourceConfig),
    Stdin(StdinSourceConfig),
    Http(HttpSourceConfig),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SinkDef {
    Mqtt(MqttSinkConfig),
    File(FileSinkConfig),
    Stdout(StdoutSinkConfig),
    Http(HttpSinkConfig),
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
