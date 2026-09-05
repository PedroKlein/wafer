use serde::{Deserialize, Serialize};

use super::engine::default_true;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PipelineConfig {
    #[serde(default)]
    pub name: Option<String>,

    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default = "default_api_bind")]
    pub bind: String,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self { enabled: true, bind: default_api_bind() }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MetricsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default = "default_metrics_path")]
    pub path: String,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self { enabled: true, path: default_metrics_path() }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RegistryConfig {
    #[serde(default)]
    pub cache_dir: Option<String>,
}

fn default_api_bind() -> String {
    "127.0.0.1:9090".to_owned()
}

fn default_metrics_path() -> String {
    "/metrics".to_owned()
}
