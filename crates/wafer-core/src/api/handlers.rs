//! HTTP request handlers.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Serialize;
use wafer_types::{ControlError, ErrorResponse, NodeInfo, PipelineState, PipelineStatus};

use crate::orchestrator::PipelineOrchestrator;

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
pub async fn ready(State(controller): State<Arc<PipelineOrchestrator>>) -> impl IntoResponse {
    let status = controller.status();

    match status.state {
        PipelineState::Running => {
            (StatusCode::OK, Json(ReadyResponse { ready: true, reason: None }))
        }
        PipelineState::Starting => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ReadyResponse { ready: false, reason: Some("starting".to_string()) }),
        ),
        PipelineState::Draining => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ReadyResponse { ready: false, reason: Some("draining".to_string()) }),
        ),
        PipelineState::Stopped => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ReadyResponse { ready: false, reason: Some("stopped".to_string()) }),
        ),
        PipelineState::Error => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ReadyResponse { ready: false, reason: Some("error".to_string()) }),
        ),
    }
}

/// GET /api/v1/pipeline - Get pipeline status
pub async fn get_pipeline(
    State(controller): State<Arc<PipelineOrchestrator>>,
) -> Json<PipelineStatus> {
    Json(controller.status())
}

/// GET /api/v1/nodes - List all nodes
pub async fn list_nodes(
    State(controller): State<Arc<PipelineOrchestrator>>,
) -> Json<Vec<NodeInfo>> {
    Json(controller.nodes())
}

/// GET /api/v1/nodes/:id - Get specific node
pub async fn get_node(
    State(controller): State<Arc<PipelineOrchestrator>>,
    Path(id): Path<String>,
) -> Result<Json<NodeInfo>, (StatusCode, Json<ErrorResponse>)> {
    let nodes = controller.nodes();

    nodes.into_iter().find(|n| n.id == id).map(Json).ok_or_else(|| {
        let err = ControlError::NodeNotFound { node_id: id };
        (StatusCode::NOT_FOUND, Json(err.into()))
    })
}

/// POST /api/v1/nodes/:id/hot-swap - Trigger hot-swap
pub async fn hot_swap(
    State(controller): State<Arc<PipelineOrchestrator>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    controller.control_hot_swap(&id).await.map(Json).map_err(|err| {
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
pub async fn reload_config(
    State(controller): State<Arc<PipelineOrchestrator>>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    controller.reload_config().await.map(Json).map_err(|err| {
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
pub async fn drain(
    State(controller): State<Arc<PipelineOrchestrator>>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    controller.drain().await.map(|()| StatusCode::OK).map_err(|err| {
        let status = match &err {
            ControlError::InvalidState { .. } => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(err.into()))
    })
}

/// POST /api/v1/pipeline/shutdown - Shutdown pipeline
pub async fn shutdown(
    State(controller): State<Arc<PipelineOrchestrator>>,
) -> Result<impl IntoResponse, (StatusCode, Json<ErrorResponse>)> {
    controller
        .control_shutdown()
        .await
        .map(|()| StatusCode::OK)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, Json(err.into())))
}

/// GET /metrics - Prometheus metrics
pub async fn metrics(State(controller): State<Arc<PipelineOrchestrator>>) -> impl IntoResponse {
    let snapshot = controller.metrics();
    let prometheus_text = snapshot.to_prometheus();

    ([(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4")], prometheus_text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        Config, EdgeDefinition, NodeDefinition, NodeType as ConfigNodeType, OverflowPolicy,
        PipelineConfig,
    };
    use crate::dag::graph::DagGraph;
    use crate::orchestrator::pipeline::ControlState;
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
        routing::{get, post},
    };
    use std::collections::HashMap;
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;
    use tower::ServiceExt;

    fn create_test_orchestrator() -> Arc<PipelineOrchestrator> {
        let config = Config {
            pipeline: PipelineConfig {
                name: "test-pipeline".to_string(),
                description: Some("Test pipeline".to_string()),
            },
            engine: Default::default(),
            api: Default::default(),
            metrics: Default::default(),
            default_queue_capacity: 1024,
            nodes: vec![
                NodeDefinition {
                    id: "source".to_string(),
                    node_type: ConfigNodeType::Source,
                    source_type: Some("stdin".to_string()),
                    sink_type: None,
                    config: toml::Value::Table(toml::map::Map::new()),
                    capabilities: Default::default(),
                },
                NodeDefinition {
                    id: "transform".to_string(),
                    node_type: ConfigNodeType::Transform,
                    source_type: None,
                    sink_type: None,
                    config: toml::Value::Table({
                        let mut map = toml::map::Map::new();
                        map.insert(
                            "plugin_path".to_string(),
                            toml::Value::String("test.wasm".to_string()),
                        );
                        map
                    }),
                    capabilities: Default::default(),
                },
                NodeDefinition {
                    id: "sink".to_string(),
                    node_type: ConfigNodeType::Sink,
                    source_type: None,
                    sink_type: Some("stdout".to_string()),
                    config: toml::Value::Table(toml::map::Map::new()),
                    capabilities: Default::default(),
                },
            ],
            edges: vec![
                EdgeDefinition {
                    from: "source".to_string(),
                    to: "transform".to_string(),
                    from_port: None,
                    to_port: None,
                    queue_capacity: None,
                    overflow: OverflowPolicy::default(),
                },
                EdgeDefinition {
                    from: "transform".to_string(),
                    to: "sink".to_string(),
                    from_port: None,
                    to_port: None,
                    queue_capacity: None,
                    overflow: OverflowPolicy::default(),
                },
            ],
            registry: Default::default(),
            dead_letter: None,
        };

        let dag_config = config.dag_config();
        let dag_graph = DagGraph::from_config(&dag_config).unwrap();
        let control_state = Arc::new(ControlState::new("test-pipeline".to_string()));

        Arc::new(PipelineOrchestrator {
            dag_graph,
            config,
            dlq_config: None,
            config_path: None,
            nodes: Mutex::new(HashMap::new()),
            run_state: Mutex::new(None),
            cancel_token: CancellationToken::new(),
            control_state,
            factory_ctx: None,
            swap_locks: Mutex::new(HashMap::new()),
        })
    }

    fn create_test_router(controller: Arc<PipelineOrchestrator>) -> Router {
        Router::new()
            .route("/health", get(health))
            .route("/ready", get(ready))
            .route("/api/v1/pipeline", get(get_pipeline))
            .route("/api/v1/pipeline/reload", post(reload_config))
            .route("/api/v1/pipeline/drain", post(drain))
            .route("/api/v1/pipeline/shutdown", post(shutdown))
            .route("/api/v1/nodes", get(list_nodes))
            .route("/api/v1/nodes/{id}", get(get_node))
            .route("/api/v1/nodes/{id}/hot-swap", post(hot_swap))
            .route("/metrics", get(metrics))
            .with_state(controller)
    }

    async fn get_body_string(body: Body) -> String {
        let bytes = http_body_util::BodyExt::collect(body).await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    // Health endpoint tests

    #[tokio::test]
    async fn test_health_returns_ok() {
        let controller = create_test_orchestrator();
        let router = create_test_router(controller);

        let response =
            router.oneshot(Request::get("/health").body(Body::empty()).unwrap()).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = get_body_string(response.into_body()).await;
        assert!(body.contains("\"status\":\"ok\""));
    }

    // Ready endpoint tests

    #[tokio::test]
    async fn test_ready_returns_ok_when_running() {
        let controller = create_test_orchestrator();
        // Transition to running state
        {
            let mut state = controller.control_state.state.lock().await;
            *state = PipelineState::Running;
        }
        let router = create_test_router(controller);

        let response =
            router.oneshot(Request::get("/ready").body(Body::empty()).unwrap()).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = get_body_string(response.into_body()).await;
        assert!(body.contains("\"ready\":true"));
    }

    #[tokio::test]
    async fn test_ready_returns_unavailable_when_draining() {
        let controller = create_test_orchestrator();
        // Cancel token to simulate draining
        controller.cancel_token.cancel();
        let router = create_test_router(controller);

        let response =
            router.oneshot(Request::get("/ready").body(Body::empty()).unwrap()).await.unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = get_body_string(response.into_body()).await;
        assert!(body.contains("\"ready\":false"));
        assert!(body.contains("\"reason\":\"draining\""));
    }

    #[tokio::test]
    async fn test_ready_returns_unavailable_when_starting() {
        let controller = create_test_orchestrator();
        // Default state is Starting, no need to change
        let router = create_test_router(controller);

        let response =
            router.oneshot(Request::get("/ready").body(Body::empty()).unwrap()).await.unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = get_body_string(response.into_body()).await;
        assert!(body.contains("\"reason\":\"starting\""));
    }

    // Pipeline status tests

    #[tokio::test]
    async fn test_get_pipeline_returns_status() {
        let controller = create_test_orchestrator();
        let router = create_test_router(controller);

        let response = router
            .oneshot(Request::get("/api/v1/pipeline").body(Body::empty()).unwrap())
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
        let controller = create_test_orchestrator();
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
        let controller = create_test_orchestrator();
        let router = create_test_router(controller);

        let response = router
            .oneshot(Request::get("/api/v1/nodes/transform").body(Body::empty()).unwrap())
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
        let controller = create_test_orchestrator();
        let router = create_test_router(controller);

        let response = router
            .oneshot(Request::get("/api/v1/nodes/nonexistent").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = get_body_string(response.into_body()).await;
        assert!(body.contains("NodeNotFound") || body.contains("node_id"));
    }

    // Hot-swap tests

    #[tokio::test]
    async fn test_hot_swap_returns_404_for_nonexistent_node() {
        let controller = create_test_orchestrator();
        let router = create_test_router(controller);

        let response = router
            .oneshot(
                Request::post("/api/v1/nodes/nonexistent/hot-swap").body(Body::empty()).unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_hot_swap_returns_400_for_non_swappable_node() {
        let controller = create_test_orchestrator();
        let router = create_test_router(controller);

        let response = router
            .oneshot(Request::post("/api/v1/nodes/source/hot-swap").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = get_body_string(response.into_body()).await;
        assert!(body.contains("not_swappable"));
    }

    #[tokio::test]
    async fn test_hot_swap_requires_config_path() {
        let controller = create_test_orchestrator();
        let router = create_test_router(controller);

        let response = router
            .oneshot(Request::post("/api/v1/nodes/transform/hot-swap").body(Body::empty()).unwrap())
            .await
            .unwrap();

        // Transform is swappable but no config_path set => ConfigError => 500
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    // Reload config tests

    #[tokio::test]
    async fn test_reload_config_requires_config_path() {
        let controller = create_test_orchestrator();
        let router = create_test_router(controller);

        let response = router
            .oneshot(Request::post("/api/v1/pipeline/reload").body(Body::empty()).unwrap())
            .await
            .unwrap();

        // No config_path => ConfigError => 400
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    // Drain tests

    #[tokio::test]
    async fn test_drain_calls_controller_and_returns_ok() {
        let controller = create_test_orchestrator();
        let router = create_test_router(controller);

        let response = router
            .oneshot(Request::post("/api/v1/pipeline/drain").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    // Shutdown tests

    #[tokio::test]
    async fn test_shutdown_calls_controller_and_returns_ok() {
        let controller = create_test_orchestrator();
        let router = create_test_router(controller);

        let response = router
            .oneshot(Request::post("/api/v1/pipeline/shutdown").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    // Metrics tests

    #[tokio::test]
    async fn test_metrics_returns_prometheus_format() {
        let controller = create_test_orchestrator();
        let router = create_test_router(controller);

        let response =
            router.oneshot(Request::get("/metrics").body(Body::empty()).unwrap()).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        // Check content type header
        let content_type =
            response.headers().get(axum::http::header::CONTENT_TYPE).unwrap().to_str().unwrap();
        assert!(content_type.contains("text/plain"));
    }
}
