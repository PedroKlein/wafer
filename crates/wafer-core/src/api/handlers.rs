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

/// Reconfigure request body.
#[derive(Deserialize)]
pub struct ReconfigureRequest {
    /// New node configuration as an arbitrary JSON object.
    pub config: serde_json::Value,
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
    use crate::orchestrator::hotswap::{
        prepare_filter_swap_timed, prepare_router_swap_timed, prepare_transform_swap_timed,
    };
    use crate::orchestrator::launcher::capabilities_from_config;
    use crate::runner::HotSwapProgress;

    #[derive(Clone, Copy)]
    enum SwapKind {
        Transform,
        Filter,
        Router,
    }

    let engine = orch.engine();
    let (kind, capabilities) = match orch.config().nodes.get(&id) {
        Some(NodeDef::Transform(wasm)) => {
            (SwapKind::Transform, capabilities_from_config(&wasm.capabilities))
        }
        Some(NodeDef::Filter(wasm)) => {
            (SwapKind::Filter, capabilities_from_config(&wasm.capabilities))
        }
        Some(NodeDef::Router(wasm)) => {
            (SwapKind::Router, capabilities_from_config(&wasm.capabilities))
        }
        Some(NodeDef::Source(_) | NodeDef::Sink(_)) => {
            return Err((StatusCode::NOT_FOUND, format!("node '{id}' does not support hot-swap")));
        }
        None => return Err((StatusCode::NOT_FOUND, format!("node '{id}' not found"))),
    };

    let wasm_bytes = tokio::fs::read(&body.wasm_path).await.map_err(|e| {
        (StatusCode::BAD_REQUEST, format!("failed to read wasm file: {e}"))
    })?;

    let (progress, completion_rx) = HotSwapProgress::channel();
    let timed_result = match kind {
        SwapKind::Transform => {
            prepare_transform_swap_timed(engine, &wasm_bytes, &id, capabilities, progress).await
        }
        SwapKind::Filter => {
            prepare_filter_swap_timed(engine, &wasm_bytes, &id, capabilities, progress).await
        }
        SwapKind::Router => {
            prepare_router_swap_timed(engine, &wasm_bytes, &id, capabilities, progress).await
        }
    };
    let mut timed_result = timed_result.map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("swap preparation failed: {e}"))
    })?;

    let signal_at = std::time::Instant::now();
    timed_result.timeline.mark_signal_sent();
    orch.send_swap(&id, timed_result.payload)
        .map_err(|e| (StatusCode::NOT_FOUND, format!("{e}")))?;

    let completion = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        completion_rx,
    )
    .await;
    let report = match completion {
        Ok(Ok(Ok(report))) => report,
        Ok(Ok(Err(err))) => {
            return Err((StatusCode::CONFLICT, err.to_string()));
        }
        Ok(Err(_)) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "hot-swap runner exited before acknowledgement".to_string(),
            ));
        }
        Err(_) => {
            return Err((
                StatusCode::GATEWAY_TIMEOUT,
                "hot-swap did not converge within 5s (no post-swap message?)".to_string(),
            ));
        }
    };

    let ack_ns = report.ack_at.duration_since(signal_at).as_nanos() as u64;
    let convergence_ns = report
        .first_v2_at
        .duration_since(report.ack_at)
        .as_nanos() as u64;

    Ok(Json(serde_json::json!({
        "node_id": id,
        "status": "swap_converged",
        "timeline": {
            "compile_ns": timed_result.timeline.compile_duration_ns(),
            "instantiate_ns": timed_result.timeline.instantiate_duration_ns(),
            "signal_ns": timed_result.timeline.signal_duration_ns(),
            "ack_ns": ack_ns,
            "convergence_ns": convergence_ns,
        }
    })))
}

/// POST /api/v1/nodes/:id/reconfigure — warm reconfigure via cached InstancePre
pub async fn reconfigure(
    State(orch): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ReconfigureRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    use crate::config::NodeDef;
    use crate::runner::{HotSwapProgress, SwapPayload};

    // Confirm the node exists and is a Wasm node.
    match orch.config().nodes.get(&id) {
        Some(NodeDef::Transform(_) | NodeDef::Filter(_) | NodeDef::Router(_)) => {}
        Some(NodeDef::Source(_) | NodeDef::Sink(_)) => {
            return Err((
                StatusCode::NOT_FOUND,
                format!("node '{id}' does not support reconfigure"),
            ));
        }
        None => return Err((StatusCode::NOT_FOUND, format!("node '{id}' not found"))),
    }

    let new_config_json = serde_json::to_string(&body.config)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid config json: {e}")))?;

    let (progress, completion_rx) = HotSwapProgress::channel();
    let payload = SwapPayload::Reconfigure { new_config_json, progress };

    let signal_at = std::time::Instant::now();
    orch.send_swap(&id, payload)
        .map_err(|e| (StatusCode::NOT_FOUND, format!("{e}")))?;

    let completion = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        completion_rx,
    )
    .await;
    let report = match completion {
        Ok(Ok(Ok(report))) => report,
        Ok(Ok(Err(err))) => {
            return Err((StatusCode::CONFLICT, err.to_string()));
        }
        Ok(Err(_)) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "reconfigure runner exited before acknowledgement".to_string(),
            ));
        }
        Err(_) => {
            return Err((
                StatusCode::GATEWAY_TIMEOUT,
                "reconfigure did not converge within 5s (no post-swap message?)".to_string(),
            ));
        }
    };

    let ack_ns = report.ack_at.duration_since(signal_at).as_nanos() as u64;
    let convergence_ns = report
        .first_v2_at
        .duration_since(report.ack_at)
        .as_nanos() as u64;

    // Reconfigure reuses the cached InstancePre; compile is unused and
    // instantiation is the tiny cached-pre `.instantiate()` inside try_reconfigure,
    // which happens between signal and ack. Report compile_ns=0 and
    // instantiate_ns=0 so evaluation code can distinguish reconfigure from
    // full hot-swap.
    Ok(Json(serde_json::json!({
        "node_id": id,
        "status": "reconfigured",
        "timeline": {
            "compile_ns": 0u64,
            "instantiate_ns": 0u64,
            "signal_ns": 0u64,
            "ack_ns": ack_ns,
            "convergence_ns": convergence_ns,
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
