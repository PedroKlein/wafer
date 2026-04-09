//! HTTP client for waferctl.

use anyhow::{Context, Result};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use wafer_types::{
    ErrorResponse, HotSwapResult, MetricsSnapshot, NodeInfo, PipelineStatus, ReloadResult,
};

/// HTTP client for WAFER runtime.
pub struct WaferClient {
    client: Client,
    base_url: String,
}

#[derive(Deserialize, serde::Serialize)]
pub struct HealthResponse {
    pub status: String,
}

impl WaferClient {
    /// Creates a new client.
    pub fn new(base_url: &str) -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self { client, base_url: base_url.trim_end_matches('/').to_string() })
    }

    /// Health check.
    pub async fn health(&self) -> Result<HealthResponse> {
        self.get("/health").await
    }

    /// Get pipeline status.
    pub async fn status(&self) -> Result<PipelineStatus> {
        self.get("/api/v1/pipeline").await
    }

    /// List all nodes.
    pub async fn nodes(&self) -> Result<Vec<NodeInfo>> {
        self.get("/api/v1/nodes").await
    }

    /// Get a specific node.
    pub async fn node(&self, id: &str) -> Result<NodeInfo> {
        self.get(&format!("/api/v1/nodes/{}", id)).await
    }

    /// Trigger hot-swap.
    pub async fn hot_swap(&self, node_id: &str) -> Result<HotSwapResult> {
        self.post(&format!("/api/v1/nodes/{}/hot-swap", node_id)).await
    }

    /// Reload configuration.
    pub async fn reload(&self) -> Result<ReloadResult> {
        self.post("/api/v1/pipeline/reload").await
    }

    /// Drain pipeline.
    pub async fn drain(&self) -> Result<()> {
        self.post_empty("/api/v1/pipeline/drain").await
    }

    /// Shutdown pipeline.
    pub async fn shutdown(&self) -> Result<()> {
        self.post_empty("/api/v1/pipeline/shutdown").await
    }

    /// Get metrics as structured data.
    pub async fn metrics(&self) -> Result<MetricsSnapshot> {
        // Note: This would need a JSON metrics endpoint or parsing
        // For now, return empty snapshot
        Ok(MetricsSnapshot::default())
    }

    /// Get raw Prometheus metrics.
    pub async fn metrics_raw(&self) -> Result<String> {
        let url = format!("{}/metrics", self.base_url);
        let response = self.client.get(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Request failed with status {}: {}", status, body);
        }

        response.text().await.context("Failed to read response")
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let response = self.client.get(&url).send().await?;

        Self::handle_response(response).await
    }

    async fn post<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let response = self.client.post(&url).send().await?;

        Self::handle_response(response).await
    }

    async fn post_empty(&self, path: &str) -> Result<()> {
        let url = format!("{}{}", self.base_url, path);
        let response = self.client.post(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            if let Ok(err) = response.json::<ErrorResponse>().await {
                anyhow::bail!("{}", err.error.message);
            }
            anyhow::bail!("Request failed with status {}", status);
        }

        Ok(())
    }

    async fn handle_response<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
        let status = response.status();

        if !status.is_success() {
            if let Ok(err) = response.json::<ErrorResponse>().await {
                anyhow::bail!("{}", err.error.message);
            }
            anyhow::bail!("Request failed with status {}", status);
        }

        response.json().await.context("Failed to parse response")
    }
}
