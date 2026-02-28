//! Integration tests for wafer-runtime.
//!
//! These tests verify that the API server components work correctly.
//! Process-level tests are skipped due to pipeline completion timing issues
//! (file-based pipelines complete too quickly for HTTP endpoint testing).
//!
//! For full E2E testing, use a long-running pipeline (e.g., stdin source)
//! manually or in CI with appropriate fixtures.

use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;
use wafer_core::api::{ApiConfig, ApiServer, MetricsServer, MetricsServerConfig};
use wafer_core::control::PipelineControl;
use wafer_types::{
    ControlError, HotSwapResult, MetricsSnapshot, NodeInfo, NodeState, NodeType, PipelineEvent,
    PipelineState, PipelineStatus, ReloadResult,
};

/// Find an available port for testing.
fn find_available_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Mock controller for testing.
struct MockController {
    name: String,
    state: PipelineState,
    nodes: Vec<NodeInfo>,
    event_tx: broadcast::Sender<PipelineEvent>,
    drain_called: AtomicBool,
    shutdown_called: AtomicBool,
}

impl MockController {
    fn new(name: &str) -> Self {
        let (event_tx, _) = broadcast::channel(16);
        Self {
            name: name.to_string(),
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
        }
    }
}

impl PipelineControl for MockController {
    async fn hot_swap(&self, node_id: &str) -> Result<HotSwapResult, ControlError> {
        Err(ControlError::NodeNotFound {
            node_id: node_id.to_string(),
        })
    }

    async fn reload_config(&self) -> Result<ReloadResult, ControlError> {
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
            HashMap::from([("node".to_string(), "source".to_string())]),
            1000,
        );
        snapshot
    }

    fn nodes(&self) -> Vec<NodeInfo> {
        self.nodes.clone()
    }

    fn subscribe(&self) -> wafer_core::control::EventReceiver {
        self.event_tx.subscribe()
    }
}

#[tokio::test]
async fn test_api_server_starts_and_serves_health() {
    let port = find_available_port();
    let controller = Arc::new(MockController::new("test-pipeline"));

    let config = ApiConfig {
        bind: format!("127.0.0.1:{}", port).parse().unwrap(),
        serve_metrics: true,
    };

    let server = ApiServer::new(config, Arc::clone(&controller))
        .await
        .expect("Failed to create API server");

    let addr = server.local_addr().unwrap();

    // Spawn server in background
    let shutdown = tokio_util::sync::CancellationToken::new();
    let shutdown_signal = shutdown.clone().cancelled_owned();
    let server_handle = tokio::spawn(async move {
        server.run_with_shutdown(shutdown_signal).await
    });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();

    // Test /health
    let resp = client
        .get(format!("http://{}/health", addr))
        .send()
        .await
        .expect("Failed to send request");
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");

    // Test /api/v1/pipeline
    let resp = client
        .get(format!("http://{}/api/v1/pipeline", addr))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "test-pipeline");

    // Test /api/v1/nodes
    let resp = client
        .get(format!("http://{}/api/v1/nodes", addr))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert_eq!(body.len(), 2);

    // Test /metrics
    let resp = client
        .get(format!("http://{}/metrics", addr))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let body = resp.text().await.unwrap();
    assert!(body.contains("wafer_messages_total"));

    // Shutdown
    shutdown.cancel();
    let _ = server_handle.await;
}

#[tokio::test]
async fn test_separate_metrics_server() {
    let api_port = find_available_port();
    let metrics_port = find_available_port();
    let controller = Arc::new(MockController::new("test-pipeline"));

    // API server without metrics
    let api_config = ApiConfig {
        bind: format!("127.0.0.1:{}", api_port).parse().unwrap(),
        serve_metrics: false, // Metrics served separately
    };

    let api_server = ApiServer::new(api_config, Arc::clone(&controller))
        .await
        .expect("Failed to create API server");

    // Metrics server
    let metrics_config = MetricsServerConfig {
        bind: format!("127.0.0.1:{}", metrics_port).parse().unwrap(),
        path: "/metrics".to_string(),
    };

    let metrics_server = MetricsServer::new(metrics_config, Arc::clone(&controller))
        .await
        .expect("Failed to create metrics server");

    let api_addr = api_server.local_addr().unwrap();
    let metrics_addr = metrics_server.local_addr().unwrap();

    // Spawn servers
    let shutdown = tokio_util::sync::CancellationToken::new();
    let shutdown1 = shutdown.clone().cancelled_owned();
    let shutdown2 = shutdown.clone().cancelled_owned();

    let api_handle = tokio::spawn(async move {
        api_server.run_with_shutdown(shutdown1).await
    });

    let metrics_handle = tokio::spawn(async move {
        metrics_server.run_with_shutdown(shutdown2).await
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();

    // API server should NOT serve metrics
    let resp = client
        .get(format!("http://{}/metrics", api_addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404);

    // Metrics server should serve metrics
    let resp = client
        .get(format!("http://{}/metrics", metrics_addr))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let body = resp.text().await.unwrap();
    assert!(body.contains("wafer_messages_total"));

    // Cleanup
    shutdown.cancel();
    let _ = api_handle.await;
    let _ = metrics_handle.await;
}

#[tokio::test]
async fn test_drain_endpoint_calls_controller() {
    let port = find_available_port();
    let controller = Arc::new(MockController::new("test-pipeline"));

    let config = ApiConfig {
        bind: format!("127.0.0.1:{}", port).parse().unwrap(),
        serve_metrics: false,
    };

    let server = ApiServer::new(config, Arc::clone(&controller))
        .await
        .unwrap();

    let addr = server.local_addr().unwrap();

    let shutdown = tokio_util::sync::CancellationToken::new();
    let shutdown_signal = shutdown.clone().cancelled_owned();
    let server_handle = tokio::spawn(async move {
        server.run_with_shutdown(shutdown_signal).await
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();

    // Call drain
    let resp = client
        .post(format!("http://{}/api/v1/pipeline/drain", addr))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    // Verify drain was called
    assert!(controller.drain_called.load(Ordering::SeqCst));

    shutdown.cancel();
    let _ = server_handle.await;
}

#[tokio::test]
async fn test_shutdown_endpoint_calls_controller() {
    let port = find_available_port();
    let controller = Arc::new(MockController::new("test-pipeline"));

    let config = ApiConfig {
        bind: format!("127.0.0.1:{}", port).parse().unwrap(),
        serve_metrics: false,
    };

    let server = ApiServer::new(config, Arc::clone(&controller))
        .await
        .unwrap();

    let addr = server.local_addr().unwrap();

    let shutdown = tokio_util::sync::CancellationToken::new();
    let shutdown_signal = shutdown.clone().cancelled_owned();
    let server_handle = tokio::spawn(async move {
        server.run_with_shutdown(shutdown_signal).await
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();

    // Call shutdown
    let resp = client
        .post(format!("http://{}/api/v1/pipeline/shutdown", addr))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    // Verify shutdown was called
    assert!(controller.shutdown_called.load(Ordering::SeqCst));

    shutdown.cancel();
    let _ = server_handle.await;
}
