//! HTTP request handlers.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use wafer_types::{ControlError, ErrorResponse, NodeInfo, PipelineState, PipelineStatus};

use crate::control::PipelineControl;

/// Health check response.
#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

/// Readiness check response.
#[derive(Serialize)]
struct ReadyResponse {
    ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

/// GET /health - Liveness check
pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

/// GET /ready - Readiness check
pub async fn ready<C: PipelineControl>(
    State(controller): State<Arc<C>>,
) -> impl IntoResponse {
    let status = controller.status();
    
    match status.state {
        PipelineState::Running => {
            (StatusCode::OK, Json(ReadyResponse { ready: true, reason: None }))
        }
        PipelineState::Starting => {
            (StatusCode::SERVICE_UNAVAILABLE, Json(ReadyResponse { 
                ready: false, 
                reason: Some("starting".to_string()) 
            }))
        }
        PipelineState::Draining => {
            (StatusCode::SERVICE_UNAVAILABLE, Json(ReadyResponse { 
                ready: false, 
                reason: Some("draining".to_string()) 
            }))
        }
        PipelineState::Stopped => {
            (StatusCode::SERVICE_UNAVAILABLE, Json(ReadyResponse { 
                ready: false, 
                reason: Some("stopped".to_string()) 
            }))
        }
        PipelineState::Error => {
            (StatusCode::SERVICE_UNAVAILABLE, Json(ReadyResponse { 
                ready: false, 
                reason: Some("error".to_string()) 
            }))
        }
    }
}

/// GET /api/v1/pipeline - Get pipeline status
pub async fn get_pipeline<C: PipelineControl>(
    State(controller): State<Arc<C>>,
) -> Json<PipelineStatus> {
    Json(controller.status())
}

/// GET /api/v1/nodes - List all nodes
pub async fn list_nodes<C: PipelineControl>(
    State(controller): State<Arc<C>>,
) -> Json<Vec<NodeInfo>> {
    Json(controller.nodes())
}

/// GET /api/v1/nodes/:id - Get specific node
pub async fn get_node<C: PipelineControl>(
    State(controller): State<Arc<C>>,
    Path(id): Path<String>,
) -> Result<Json<NodeInfo>, (StatusCode, Json<ErrorResponse>)> {
    let nodes = controller.nodes();
    
    nodes
        .into_iter()
        .find(|n| n.id == id)
        .map(Json)
        .ok_or_else(|| {
            let err = ControlError::NodeNotFound { node_id: id };
            (StatusCode::NOT_FOUND, Json(err.into()))
        })
}

/// POST /api/v1/nodes/:id/hot-swap - Trigger hot-swap
pub async fn hot_swap<C: PipelineControl>(
    State(controller): State<Arc<C>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    controller
        .hot_swap(&id)
        .await
        .map(|result| Json(result))
        .map_err(|err| {
            let status = match &err {
                ControlError::NodeNotFound { .. } => StatusCode::NOT_FOUND,
                ControlError::SwapInProgress => StatusCode::CONFLICT,
                ControlError::NotSwappable { .. } => StatusCode::BAD_REQUEST,
                ControlError::NotImplemented { .. } => StatusCode::NOT_IMPLEMENTED,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            (status, Json(err.into()))
        })
}

/// POST /api/v1/pipeline/reload - Reload configuration
pub async fn reload_config<C: PipelineControl>(
    State(controller): State<Arc<C>>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    controller
        .reload_config()
        .await
        .map(|result| Json(result))
        .map_err(|err| {
            let status = match &err {
                ControlError::ConfigError { .. } => StatusCode::BAD_REQUEST,
                ControlError::SwapInProgress => StatusCode::CONFLICT,
                ControlError::NotImplemented { .. } => StatusCode::NOT_IMPLEMENTED,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            (status, Json(err.into()))
        })
}

/// POST /api/v1/pipeline/drain - Drain pipeline
pub async fn drain<C: PipelineControl>(
    State(controller): State<Arc<C>>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    controller
        .drain()
        .await
        .map(|()| StatusCode::OK)
        .map_err(|err| {
            let status = match &err {
                ControlError::InvalidState { .. } => StatusCode::CONFLICT,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            (status, Json(err.into()))
        })
}

/// POST /api/v1/pipeline/shutdown - Shutdown pipeline
pub async fn shutdown<C: PipelineControl>(
    State(controller): State<Arc<C>>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    controller
        .shutdown()
        .await
        .map(|()| StatusCode::OK)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, Json(err.into())))
}

/// GET /metrics - Prometheus metrics
pub async fn metrics<C: PipelineControl>(
    State(controller): State<Arc<C>>,
) -> impl IntoResponse {
    let snapshot = controller.metrics();
    let prometheus_text = snapshot.to_prometheus();
    
    (
        [(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        prometheus_text,
    )
}
