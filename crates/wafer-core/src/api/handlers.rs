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

/// GET /health - Liveness check
pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

/// GET /ready - Readiness check
pub async fn ready<C: PipelineControl>(State(controller): State<Arc<C>>) -> impl IntoResponse {
    let status = controller.status();

    match status.state {
        PipelineState::Running => (
            StatusCode::OK,
            Json(ReadyResponse {
                ready: true,
                reason: None,
            }),
        ),
        PipelineState::Starting => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ReadyResponse {
                ready: false,
                reason: Some("starting".to_string()),
            }),
        ),
        PipelineState::Draining => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ReadyResponse {
                ready: false,
                reason: Some("draining".to_string()),
            }),
        ),
        PipelineState::Stopped => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ReadyResponse {
                ready: false,
                reason: Some("stopped".to_string()),
            }),
        ),
        PipelineState::Error => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ReadyResponse {
                ready: false,
                reason: Some("error".to_string()),
            }),
        ),
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
pub async fn metrics<C: PipelineControl>(State(controller): State<Arc<C>>) -> impl IntoResponse {
    let snapshot = controller.metrics();
    let prometheus_text = snapshot.to_prometheus();

    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        prometheus_text,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::{get, post},
        Router,
    };
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::broadcast;
    use tower::ServiceExt;
    use wafer_types::{
        ControlError, HotSwapResult, MetricsSnapshot, NodeInfo, NodeState, NodeType, PipelineEvent,
        PipelineState, PipelineStatus, ReloadResult,
    };

    /// Mock controller for testing HTTP handlers.
    struct MockController {
        name: String,
        state: PipelineState,
        nodes: Vec<NodeInfo>,
        event_tx: broadcast::Sender<PipelineEvent>,
        drain_called: AtomicBool,
        shutdown_called: AtomicBool,
        hot_swap_result: Option<Result<HotSwapResult, ControlError>>,
        reload_result: Option<Result<ReloadResult, ControlError>>,
    }

    impl MockController {
        fn new() -> Self {
            let (event_tx, _) = broadcast::channel(16);
            Self {
                name: "test-pipeline".to_string(),
                state: PipelineState::Running,
                nodes: vec![
                    NodeInfo {
                        id: "source".to_string(),
                        node_type: NodeType::Source,
                        state: NodeState::Running,
                        swappable: false,
                        messages_processed: 100,
                        messages_failed: 0,
                        avg_process_us: 50,
                        queue_depth: None,
                    },
                    NodeInfo {
                        id: "transform".to_string(),
                        node_type: NodeType::Transform,
                        state: NodeState::Running,
                        swappable: true,
                        messages_processed: 100,
                        messages_failed: 2,
                        avg_process_us: 150,
                        queue_depth: Some(10),
                    },
                    NodeInfo {
                        id: "sink".to_string(),
                        node_type: NodeType::Sink,
                        state: NodeState::Running,
                        swappable: false,
                        messages_processed: 98,
                        messages_failed: 0,
                        avg_process_us: 30,
                        queue_depth: None,
                    },
                ],
                event_tx,
                drain_called: AtomicBool::new(false),
                shutdown_called: AtomicBool::new(false),
                hot_swap_result: None,
                reload_result: None,
            }
        }

        fn with_state(mut self, state: PipelineState) -> Self {
            self.state = state;
            self
        }

        fn with_hot_swap_result(mut self, result: Result<HotSwapResult, ControlError>) -> Self {
            self.hot_swap_result = Some(result);
            self
        }

        fn with_reload_result(mut self, result: Result<ReloadResult, ControlError>) -> Self {
            self.reload_result = Some(result);
            self
        }
    }

    impl PipelineControl for MockController {
        async fn hot_swap(&self, node_id: &str) -> Result<HotSwapResult, ControlError> {
            if let Some(ref result) = self.hot_swap_result {
                return result.clone();
            }

            // Default behavior: check if node exists and is swappable
            let node = self.nodes.iter().find(|n| n.id == node_id);
            match node {
                None => Err(ControlError::NodeNotFound {
                    node_id: node_id.to_string(),
                }),
                Some(n) if !n.swappable => Err(ControlError::NotSwappable {
                    node_id: node_id.to_string(),
                }),
                Some(_) => Err(ControlError::NotImplemented {
                    operation: "hot_swap".to_string(),
                }),
            }
        }

        async fn reload_config(&self) -> Result<ReloadResult, ControlError> {
            if let Some(ref result) = self.reload_result {
                return result.clone();
            }
            Err(ControlError::NotImplemented {
                operation: "reload_config".to_string(),
            })
        }

        async fn drain(&self) -> Result<(), ControlError> {
            self.drain_called.store(true, Ordering::SeqCst);
            Ok(())
        }

        async fn shutdown(&self) -> Result<(), ControlError> {
            self.shutdown_called.store(true, Ordering::SeqCst);
            Ok(())
        }

        fn status(&self) -> PipelineStatus {
            PipelineStatus {
                name: self.name.clone(),
                state: self.state.clone(),
                uptime_secs: 3600,
                messages_processed: 1000,
                messages_failed: 5,
                node_count: self.nodes.len(),
                swap_in_progress: false,
            }
        }

        fn metrics(&self) -> MetricsSnapshot {
            let mut snapshot = MetricsSnapshot::default();
            snapshot.add_counter(
                "wafer_messages_total",
                "Total messages processed",
                HashMap::from([("node".to_string(), "transform".to_string())]),
                1000,
            );
            snapshot
        }

        fn nodes(&self) -> Vec<NodeInfo> {
            self.nodes.clone()
        }

        fn subscribe(&self) -> crate::control::EventReceiver {
            self.event_tx.subscribe()
        }
    }

    fn create_test_router(controller: Arc<MockController>) -> Router {
        Router::new()
            .route("/health", get(health))
            .route("/ready", get(ready::<MockController>))
            .route("/api/v1/pipeline", get(get_pipeline::<MockController>))
            .route(
                "/api/v1/pipeline/reload",
                post(reload_config::<MockController>),
            )
            .route("/api/v1/pipeline/drain", post(drain::<MockController>))
            .route(
                "/api/v1/pipeline/shutdown",
                post(shutdown::<MockController>),
            )
            .route("/api/v1/nodes", get(list_nodes::<MockController>))
            .route("/api/v1/nodes/{id}", get(get_node::<MockController>))
            .route(
                "/api/v1/nodes/{id}/hot-swap",
                post(hot_swap::<MockController>),
            )
            .route("/metrics", get(metrics::<MockController>))
            .with_state(controller)
    }

    async fn get_body_string(body: Body) -> String {
        let bytes = http_body_util::BodyExt::collect(body)
            .await
            .unwrap()
            .to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    // Health endpoint tests

    #[tokio::test]
    async fn test_health_returns_ok() {
        let controller = Arc::new(MockController::new());
        let router = create_test_router(controller);

        let response = router
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = get_body_string(response.into_body()).await;
        assert!(body.contains("\"status\":\"ok\""));
    }

    // Ready endpoint tests

    #[tokio::test]
    async fn test_ready_returns_ok_when_running() {
        let controller = Arc::new(MockController::new().with_state(PipelineState::Running));
        let router = create_test_router(controller);

        let response = router
            .oneshot(Request::get("/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = get_body_string(response.into_body()).await;
        assert!(body.contains("\"ready\":true"));
    }

    #[tokio::test]
    async fn test_ready_returns_unavailable_when_draining() {
        let controller = Arc::new(MockController::new().with_state(PipelineState::Draining));
        let router = create_test_router(controller);

        let response = router
            .oneshot(Request::get("/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = get_body_string(response.into_body()).await;
        assert!(body.contains("\"ready\":false"));
        assert!(body.contains("\"reason\":\"draining\""));
    }

    #[tokio::test]
    async fn test_ready_returns_unavailable_when_starting() {
        let controller = Arc::new(MockController::new().with_state(PipelineState::Starting));
        let router = create_test_router(controller);

        let response = router
            .oneshot(Request::get("/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = get_body_string(response.into_body()).await;
        assert!(body.contains("\"reason\":\"starting\""));
    }

    // Pipeline status tests

    #[tokio::test]
    async fn test_get_pipeline_returns_status() {
        let controller = Arc::new(MockController::new());
        let router = create_test_router(controller);

        let response = router
            .oneshot(
                Request::get("/api/v1/pipeline")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = get_body_string(response.into_body()).await;
        let status: PipelineStatus = serde_json::from_str(&body).unwrap();
        assert_eq!(status.name, "test-pipeline");
        assert_eq!(status.node_count, 3);
    }

    // Node listing tests

    #[tokio::test]
    async fn test_list_nodes_returns_all_nodes() {
        let controller = Arc::new(MockController::new());
        let router = create_test_router(controller);

        let response = router
            .oneshot(Request::get("/api/v1/nodes").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = get_body_string(response.into_body()).await;
        let nodes: Vec<NodeInfo> = serde_json::from_str(&body).unwrap();
        assert_eq!(nodes.len(), 3);
    }

    #[tokio::test]
    async fn test_get_node_returns_single_node() {
        let controller = Arc::new(MockController::new());
        let router = create_test_router(controller);

        let response = router
            .oneshot(
                Request::get("/api/v1/nodes/transform")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = get_body_string(response.into_body()).await;
        let node: NodeInfo = serde_json::from_str(&body).unwrap();
        assert_eq!(node.id, "transform");
        assert!(node.swappable);
    }

    #[tokio::test]
    async fn test_get_node_returns_404_for_nonexistent() {
        let controller = Arc::new(MockController::new());
        let router = create_test_router(controller);

        let response = router
            .oneshot(
                Request::get("/api/v1/nodes/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = get_body_string(response.into_body()).await;
        assert!(body.contains("NodeNotFound") || body.contains("node_id"));
    }

    // Hot-swap tests

    #[tokio::test]
    async fn test_hot_swap_returns_404_for_nonexistent_node() {
        let controller = Arc::new(MockController::new());
        let router = create_test_router(controller);

        let response = router
            .oneshot(
                Request::post("/api/v1/nodes/nonexistent/hot-swap")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_hot_swap_returns_400_for_non_swappable_node() {
        let controller = Arc::new(MockController::new());
        let router = create_test_router(controller);

        let response = router
            .oneshot(
                Request::post("/api/v1/nodes/source/hot-swap")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = get_body_string(response.into_body()).await;
        assert!(body.contains("not_swappable")); // error code is snake_case
    }

    #[tokio::test]
    async fn test_hot_swap_returns_501_not_implemented() {
        let controller = Arc::new(MockController::new());
        let router = create_test_router(controller);

        let response = router
            .oneshot(
                Request::post("/api/v1/nodes/transform/hot-swap")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn test_hot_swap_returns_409_when_swap_in_progress() {
        let controller =
            Arc::new(MockController::new().with_hot_swap_result(Err(ControlError::SwapInProgress)));
        let router = create_test_router(controller);

        let response = router
            .oneshot(
                Request::post("/api/v1/nodes/transform/hot-swap")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    // Reload config tests

    #[tokio::test]
    async fn test_reload_config_returns_501_not_implemented() {
        let controller = Arc::new(MockController::new());
        let router = create_test_router(controller);

        let response = router
            .oneshot(
                Request::post("/api/v1/pipeline/reload")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn test_reload_config_returns_400_for_config_error() {
        let controller = Arc::new(MockController::new().with_reload_result(Err(
            ControlError::ConfigError {
                message: "invalid TOML".to_string(),
            },
        )));
        let router = create_test_router(controller);

        let response = router
            .oneshot(
                Request::post("/api/v1/pipeline/reload")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    // Drain tests

    #[tokio::test]
    async fn test_drain_calls_controller_and_returns_ok() {
        let controller = Arc::new(MockController::new());
        let router = create_test_router(controller.clone());

        let response = router
            .oneshot(
                Request::post("/api/v1/pipeline/drain")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(controller.drain_called.load(Ordering::SeqCst));
    }

    // Shutdown tests

    #[tokio::test]
    async fn test_shutdown_calls_controller_and_returns_ok() {
        let controller = Arc::new(MockController::new());
        let router = create_test_router(controller.clone());

        let response = router
            .oneshot(
                Request::post("/api/v1/pipeline/shutdown")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(controller.shutdown_called.load(Ordering::SeqCst));
    }

    // Metrics tests

    #[tokio::test]
    async fn test_metrics_returns_prometheus_format() {
        let controller = Arc::new(MockController::new());
        let router = create_test_router(controller);

        let response = router
            .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        // Check content type header
        let content_type = response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(content_type.contains("text/plain"));

        let body = get_body_string(response.into_body()).await;
        assert!(body.contains("wafer_messages_total"));
    }
}
