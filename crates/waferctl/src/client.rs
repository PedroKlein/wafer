//! HTTP client for waferctl.

use anyhow::{Context, Result};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use wafer_types::ErrorResponse;

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
    pub replacement_eligible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[expect(clippy::struct_field_names, reason = "field names match the runtime's JSON keys")]
pub struct HotSwapTimeline {
    pub compile_ns: Option<u64>,
    pub instantiate_ns: Option<u64>,
    pub signal_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement_adopted_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_post_replacement_local_outcome_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_ns: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalOutcome {
    pub disposition: String,
    pub after_adoption_ns: u64,
}

/// Body of a 200 or 202 hot-swap response, or of a 200 `rolled_back` one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotSwapResult {
    pub node_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub replacement_adopted: bool,
    #[serde(default)]
    pub first_post_replacement_local_outcome: Option<LocalOutcome>,
    pub compile_cache: Option<String>,
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
        self.post_json(&format!("/api/v1/nodes/{node_id}/hot-swap"), &HotSwapRequest { wasm_path })
            .await
    }

    /// Shutdown pipeline.
    pub async fn shutdown(&self) -> Result<()> {
        self.post_empty("/api/v1/pipeline/shutdown").await
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

    async fn post_json<T: DeserializeOwned, B: Serialize + Sync>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let response = self.client.post(&url).json(body).send().await?;

        Self::handle_response(response).await
    }

    async fn post_empty(&self, path: &str) -> Result<()> {
        let url = format!("{}{}", self.base_url, path);
        let response = self.client.post(&url).send().await?;

        if !response.status().is_success() {
            return Err(Self::error_from(response).await);
        }

        Ok(())
    }

    async fn error_from(response: reqwest::Response) -> anyhow::Error {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::anyhow!("{}", error_message(status, &body))
    }

    async fn handle_response<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
        let status = response.status();

        if !status.is_success() {
            return Err(Self::error_from(response).await);
        }

        response.json().await.context("Failed to parse response")
    }
}

/// The runtime answers some errors with a JSON body and others, such as
/// axum's request rejections, with plain text.
fn error_message(status: reqwest::StatusCode, body: &str) -> String {
    if let Ok(err) = serde_json::from_str::<ErrorResponse>(body) {
        return err.error.message;
    }
    let body = body.trim();
    if body.is_empty() {
        format!("Request failed with status {status}")
    } else {
        format!("Request failed with status {status}: {body}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node_json(id: &str, replacement_eligible: bool) -> serde_json::Value {
        json!({
            "id": id,
            "state": "running",
            "processed": 42,
            "failed": 1,
            "replacement_eligible": replacement_eligible,
        })
    }

    #[test]
    fn parses_node_list() {
        let body = json!([node_json("source", false), node_json("transform", true)]);
        let nodes: Vec<NodeInfo> = serde_json::from_value(body).unwrap();
        assert_eq!(nodes.len(), 2);
        assert!(!nodes[0].replacement_eligible);
        assert!(nodes[1].replacement_eligible);
        assert_eq!(nodes[1].state, "running");
    }

    #[test]
    fn parses_single_node() {
        let node: NodeInfo = serde_json::from_value(node_json("transform", true)).unwrap();
        assert_eq!(node.id, "transform");
        assert_eq!((node.processed, node.failed), (42, 1));
        assert!(node.replacement_eligible);
    }

    #[test]
    fn parses_adopted_hot_swap() {
        let body = json!({
            "node_id": "transform",
            "replacement_adopted": true,
            "first_post_replacement_local_outcome": {
                "disposition": "forwarded/enqueued",
                "after_adoption_ns": 900,
            },
            "compile_cache": "compiled",
            "timeline": {
                "compile_ns": 1000,
                "instantiate_ns": 200,
                "signal_ns": 30,
                "replacement_adopted_ns": 400,
                "first_post_replacement_local_outcome_ns": 900,
            }
        });
        let result: HotSwapResult = serde_json::from_value(body).unwrap();
        assert_eq!(result.status, None);
        assert!(result.replacement_adopted);
        assert_eq!(
            result.first_post_replacement_local_outcome.unwrap().disposition,
            "forwarded/enqueued"
        );
        assert_eq!(result.compile_cache.as_deref(), Some("compiled"));
        assert_eq!(result.timeline.replacement_adopted_ns, Some(400));
    }

    #[test]
    fn parses_accepted_hot_swap() {
        let body = json!({
            "node_id": "transform",
            "replacement_adopted": false,
            "first_post_replacement_local_outcome": null,
            "compile_cache": "memory_hit",
            "timeline": {
                "compile_ns": 1000,
                "instantiate_ns": 200,
                "signal_ns": 30,
                "replacement_adopted_ns": null,
                "first_post_replacement_local_outcome_ns": null,
            }
        });
        let result: HotSwapResult = serde_json::from_value(body).unwrap();
        assert!(!result.replacement_adopted);
        assert!(result.first_post_replacement_local_outcome.is_none());
        assert_eq!(result.timeline.replacement_adopted_ns, None);
    }

    #[test]
    fn parses_rolled_back_hot_swap() {
        let body = json!({
            "node_id": "transform",
            "status": "rolled_back",
            "reason": "init failed",
            "compile_cache": null,
            "timeline": {
                "compile_ns": null,
                "instantiate_ns": 200,
                "signal_ns": 30,
                "rollback_ns": 50,
            }
        });
        let result: HotSwapResult = serde_json::from_value(body).unwrap();
        assert_eq!(result.status.as_deref(), Some("rolled_back"));
        assert_eq!(result.reason.as_deref(), Some("init failed"));
        assert!(!result.replacement_adopted);
        assert_eq!(result.timeline.rollback_ns, Some(50));
    }

    #[test]
    fn error_message_uses_the_json_error_message() {
        let body = json!({"error": {"code": "node_not_found", "message": "node 'x' not found"}})
            .to_string();
        let message = error_message(reqwest::StatusCode::NOT_FOUND, &body);
        assert_eq!(message, "node 'x' not found");
    }

    #[test]
    fn error_message_includes_a_plain_text_body() {
        let message = error_message(
            reqwest::StatusCode::UNPROCESSABLE_ENTITY,
            "Failed to deserialize the JSON body into the target type\n",
        );
        assert_eq!(
            message,
            "Request failed with status 422 Unprocessable Entity: \
             Failed to deserialize the JSON body into the target type"
        );
    }

    #[test]
    fn error_message_falls_back_to_the_status_for_an_empty_body() {
        let message = error_message(reqwest::StatusCode::BAD_GATEWAY, "");
        assert_eq!(message, "Request failed with status 502 Bad Gateway");
    }
}
