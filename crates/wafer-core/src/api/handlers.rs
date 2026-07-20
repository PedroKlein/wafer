//! HTTP request handlers for the new pipeline orchestrator.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};

use crate::orchestrator::PipelineHandle;

/// Shared state type for axum handlers.
pub type AppState = Arc<PipelineHandle>;

/// Health check response.
#[derive(Serialize)]
pub struct HealthResponse {
    status: &'static str,
}

/// Readiness check response.
#[derive(Serialize)]
struct ReadyResponse {
    ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

/// Node info response.
#[derive(Serialize)]
pub struct NodeInfoResponse {
    pub id: String,
    pub state: String,
    pub processed: u64,
    pub failed: u64,
    pub swappable: bool,
}

/// Hot-swap request body.
#[derive(Deserialize)]
pub struct HotSwapRequest {
    pub wasm_path: String,
}

/// GET /health — always OK (liveness probe)
pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

/// GET /ready — is the pipeline running?
pub async fn ready(State(orch): State<AppState>) -> impl IntoResponse {
    if orch.is_running() {
        (StatusCode::OK, Json(ReadyResponse { ready: true, reason: None }))
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ReadyResponse { ready: false, reason: Some("not running".to_string()) }),
        )
    }
}

/// GET /api/v1/nodes — list all nodes with state and metrics
pub async fn list_nodes(State(orch): State<AppState>) -> Json<Vec<NodeInfoResponse>> {
    let swappable = orch.swappable_nodes();
    let nodes: Vec<NodeInfoResponse> = orch
        .config()
        .nodes
        .keys()
        .map(|id| {
            let state = orch
                .node_state(id)
                .map(|s| format!("{s:?}"))
                .unwrap_or_else(|| "Unknown".to_string());
            let (processed, failed) = orch
                .node_metrics(id)
                .map(|m| (m.processed(), m.failed()))
                .unwrap_or((0, 0));
            NodeInfoResponse {
                id: id.clone(),
                state,
                processed,
                failed,
                swappable: swappable.contains(&id.as_str()),
            }
        })
        .collect();
    Json(nodes)
}

/// GET /api/v1/nodes/:id — single node info
pub async fn get_node(
    State(orch): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<NodeInfoResponse>, StatusCode> {
    let state = orch.node_state(&id).ok_or(StatusCode::NOT_FOUND)?;
    let (processed, failed) = orch
        .node_metrics(&id)
        .map(|m| (m.processed(), m.failed()))
        .unwrap_or((0, 0));
    let swappable = orch.swappable_nodes().contains(&id.as_str());

    Ok(Json(NodeInfoResponse {
        id,
        state: format!("{state:?}"),
        processed,
        failed,
        swappable,
    }))
}

/// POST /api/v1/nodes/:id/hot-swap — trigger hot-swap with new Wasm binary
pub async fn hot_swap(
    State(orch): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<HotSwapRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    use crate::config::NodeDef;
    use crate::orchestrator::hotswap::prepare_transform_swap_timed;
    use crate::orchestrator::launcher::capabilities_from_config;

    let engine = orch.engine();
    let capabilities = match orch.config().nodes.get(&id) {
        Some(NodeDef::Transform(wasm) | NodeDef::Filter(wasm) | NodeDef::Router(wasm)) => {
            capabilities_from_config(&wasm.capabilities)
        }
        Some(NodeDef::Source(_) | NodeDef::Sink(_)) => {
            return Err((StatusCode::NOT_FOUND, format!("node '{id}' does not support hot-swap")));
        }
        None => return Err((StatusCode::NOT_FOUND, format!("node '{id}' not found"))),
    };

    let wasm_bytes = tokio::fs::read(&body.wasm_path).await.map_err(|e| {
        (StatusCode::BAD_REQUEST, format!("failed to read wasm file: {e}"))
    })?;

    let mut timed_result = prepare_transform_swap_timed(
        engine,
        &wasm_bytes,
        &id,
        capabilities,
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("swap preparation failed: {e}")))?;

    timed_result.timeline.mark_signal_sent();
    orch.send_swap(&id, timed_result.payload)
        .map_err(|e| (StatusCode::NOT_FOUND, format!("{e}")))?;
    timed_result.timeline.mark_swap_acked();
    timed_result.timeline.mark_first_v2_output();

    Ok(Json(serde_json::json!({
        "node_id": id,
        "status": "swap_sent",
        "timeline": {
            "compile_ns": timed_result.timeline.compile_duration_ns(),
            "instantiate_ns": timed_result.timeline.instantiate_duration_ns(),
            "signal_ns": timed_result.timeline.signal_duration_ns(),
            "ack_ns": timed_result.timeline.ack_duration_ns(),
            "convergence_ns": timed_result.timeline.convergence_duration_ns(),
        }
    })))
}

/// POST /api/v1/pipeline/shutdown — trigger graceful shutdown
pub async fn shutdown(State(orch): State<AppState>) -> StatusCode {
    orch.cancel();
    StatusCode::OK
}

/// GET /metrics — Prometheus-style text metrics
pub async fn metrics(State(orch): State<AppState>) -> impl IntoResponse {
    let mut output = String::new();

    for node_id in orch.config().nodes.keys() {
        if let Some(m) = orch.node_metrics(node_id) {
            output.push_str(&format!(
                "wafer_node_processed_total{{node=\"{}\"}} {}\n",
                node_id,
                m.processed()
            ));
            output.push_str(&format!(
                "wafer_node_failed_total{{node=\"{}\"}} {}\n",
                node_id,
                m.failed()
            ));
        }
    }

    ([(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4")], output)
}
