//! HTTP client for waferctl.

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;
use wafer_types::{ErrorResponse, MetricsSnapshot};

/// HTTP client for WAFER runtime.
pub struct WaferClient {
    client: Client,
    base_url: String,
}

#[derive(Deserialize, Serialize)]
pub struct HealthResponse {
    pub status: String,
}

#[derive(Deserialize)]
struct ReadyResponse {
    ready: bool,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineStatus {
    pub name: String,
    pub state: String,
    pub node_count: usize,
    pub messages_processed: u64,
    pub messages_failed: u64,
    pub ready_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub id: String,
    pub state: String,
    pub processed: u64,
    pub failed: u64,
    pub swappable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotSwapTimeline {
    pub compile_ns: Option<u64>,
    pub instantiate_ns: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotSwapResult {
    pub node_id: String,
    pub status: String,
    pub timeline: HotSwapTimeline,
}

#[derive(Serialize)]
struct HotSwapRequest<'a> {
    wasm_path: &'a str,
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

    /// Get pipeline status inferred from the documented readiness and nodes endpoints.
    pub async fn status(&self) -> Result<PipelineStatus> {
        let ready: ReadyResponse = self.get("/ready").await?;
        let nodes = self.nodes().await?;
        Ok(PipelineStatus {
            name: "wafer-pipeline".to_string(),
            state: if ready.ready { "running" } else { "not-ready" }.to_string(),
            node_count: nodes.len(),
            messages_processed: nodes.iter().map(|node| node.processed).sum(),
            messages_failed: nodes.iter().map(|node| node.failed).sum(),
            ready_reason: ready.reason,
        })
    }

    /// List all nodes.
    pub async fn nodes(&self) -> Result<Vec<NodeInfo>> {
        self.get("/api/v1/nodes").await
    }

    /// Get a specific node.
    pub async fn node(&self, id: &str) -> Result<NodeInfo> {
        self.get(&format!("/api/v1/nodes/{id}")).await
    }

    /// Trigger hot-swap.
    pub async fn hot_swap(&self, node_id: &str, wasm_path: &str) -> Result<HotSwapResult> {
        self.post_json(
            &format!("/api/v1/nodes/{node_id}/hot-swap"),
            &HotSwapRequest { wasm_path },
        )
        .await
    }

    /// Shutdown pipeline.
    pub async fn shutdown(&self) -> Result<()> {
        self.post_empty("/api/v1/pipeline/shutdown").await
    }

    /// Get metrics as structured data.
    ///
    /// Currently returns a default snapshot; will call the runtime API when
    /// the structured metrics endpoint is implemented.
    #[expect(clippy::unused_self, reason = "Will use self.get() once runtime metrics endpoint exists")]
    #[expect(clippy::unnecessary_wraps, reason = "Maintains consistent Result<T> API with other client methods")]
    pub fn metrics(&self) -> Result<MetricsSnapshot> {
        Ok(MetricsSnapshot::default())
    }

    /// Get raw Prometheus metrics.
    pub async fn metrics_raw(&self) -> Result<String> {
        let url = format!("{}/metrics", self.base_url);
        let response = self.client.get(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Request failed with status {status}: {body}");
        }

        response.text().await.context("Failed to read response")
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let response = self.client.get(&url).send().await?;

        Self::handle_response(response).await
    }

    async fn post_json<T: DeserializeOwned, B: Serialize + Sync>(&self, path: &str, body: &B) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let response = self.client.post(&url).json(body).send().await?;

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
            anyhow::bail!("Request failed with status {status}");
        }

        Ok(())
    }

    async fn handle_response<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
        let status = response.status();

        if !status.is_success() {
            if let Ok(err) = response.json::<ErrorResponse>().await {
                anyhow::bail!("{}", err.error.message);
            }
            anyhow::bail!("Request failed with status {status}");
        }

        response.json().await.context("Failed to parse response")
    }
}
