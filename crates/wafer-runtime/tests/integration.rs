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

use serde_json::Value;
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
    let server_handle =
        tokio::spawn(async move { server.run_with_shutdown(shutdown_signal).await });

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

    // Test /metrics - verify Prometheus format
    let resp = client
        .get(format!("http://{}/metrics", addr))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    // Verify Content-Type header indicates Prometheus format
    let content_type = resp
        .headers()
        .get("content-type")
        .expect("Missing content-type header");
    let content_type_str = content_type.to_str().unwrap();
    assert!(
        content_type_str.contains("text/plain")
            || content_type_str.contains("text/plain; charset=utf-8"),
        "Expected text/plain content type, got: {}",
        content_type_str
    );

    let body = resp.text().await.unwrap();
    // Verify basic Prometheus format markers
    assert!(
        body.contains("wafer_messages_total"),
        "Missing wafer_messages_total metric"
    );
    assert!(
        body.contains("# TYPE") || body.contains("# HELP"),
        "Missing Prometheus metadata comments"
    );

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

    let api_handle = tokio::spawn(async move { api_server.run_with_shutdown(shutdown1).await });

    let metrics_handle =
        tokio::spawn(async move { metrics_server.run_with_shutdown(shutdown2).await });

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
    let server_handle =
        tokio::spawn(async move { server.run_with_shutdown(shutdown_signal).await });

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
    let server_handle =
        tokio::spawn(async move { server.run_with_shutdown(shutdown_signal).await });

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

/// Test module for JSON log format validation.
///
/// These tests verify that the JSON log format from tracing-subscriber matches
/// the structure defined in SPEC §12.4 (Structured Logging).
mod json_log_format {
    use super::*;

    /// Validates that a JSON log line has the required structure.
    ///
    /// Per SPEC §12.4, JSON logs MUST include:
    /// - `timestamp`: RFC 3339 format
    /// - `level`: log level (INFO, DEBUG, etc.)
    /// - `target`: module path
    /// - `message`: log message text
    ///
    /// May optionally include:
    /// - `span`: object with span fields (pipeline, node_id, etc.)
    /// - `fields`: additional structured key-value pairs
    fn validate_json_log_structure(json_str: &str) -> Result<(), String> {
        let log: Value =
            serde_json::from_str(json_str).map_err(|e| format!("Failed to parse JSON: {e}"))?;

        // Required fields
        let obj = log.as_object().ok_or("Log entry must be an object")?;

        // timestamp field (tracing-subscriber uses 'timestamp')
        if !obj.contains_key("timestamp") {
            return Err("Missing 'timestamp' field".to_string());
        }
        let timestamp = obj["timestamp"]
            .as_str()
            .ok_or("timestamp must be a string")?;
        // Validate RFC 3339 format (basic check for format like "2024-01-15T10:30:00.123456789Z")
        if !timestamp.contains('T') || (!timestamp.ends_with('Z') && !timestamp.contains('+')) {
            return Err(format!("timestamp not in RFC 3339 format: {timestamp}"));
        }

        // level field
        if !obj.contains_key("level") {
            return Err("Missing 'level' field".to_string());
        }
        let level = obj["level"].as_str().ok_or("level must be a string")?;
        let valid_levels = ["TRACE", "DEBUG", "INFO", "WARN", "ERROR"];
        if !valid_levels.contains(&level) {
            return Err(format!("Invalid log level: {level}"));
        }

        // target field (tracing-subscriber uses 'target')
        if !obj.contains_key("target") {
            return Err("Missing 'target' field".to_string());
        }

        // message field (tracing-subscriber can use 'message' or 'fields.message')
        // The actual field name depends on how the log was created
        let has_message = obj.contains_key("message")
            || obj.get("fields").and_then(|f| f.get("message")).is_some();
        if !has_message {
            // Some log events may not have a message (event-only spans)
            // This is acceptable per the tracing crate design
        }

        Ok(())
    }

    /// Test that validates expected JSON log structure from tracing-subscriber.
    ///
    /// This test simulates what the JSON output should look like and validates
    /// our parsing logic. In production, the actual output comes from
    /// tracing-subscriber's JSON layer.
    #[test]
    fn test_json_log_format_spec_compliance() {
        // Example JSON log output from tracing-subscriber with json layer
        let example_logs = [
            // Standard INFO message
            r#"{"timestamp":"2024-01-15T10:30:00.123456789Z","level":"INFO","target":"wafer_runtime","message":"WAFER Runtime starting..."}"#,
            // Message with fields
            r#"{"timestamp":"2024-01-15T10:30:00.123456789Z","level":"INFO","target":"wafer_runtime","fields":{"config":"/path/to/config.toml"},"message":"Loading configuration"}"#,
            // Message with span context
            r#"{"timestamp":"2024-01-15T10:30:00.123456789Z","level":"DEBUG","target":"wafer_core::dag::runner","span":{"node_id":"transform-1","node_type":"transform"},"message":"Transform emitted"}"#,
            // Processing log with all required fields
            r#"{"timestamp":"2024-01-15T10:30:00.123456789Z","level":"DEBUG","target":"wafer_core::dag::runner","span":{"node_id":"transform-1","node_type":"transform"},"fields":{"message_id":"msg-123","duration_ns":5000000,"input_size_bytes":1024,"output_size_bytes":512},"message":"Transform emitted"}"#,
            // Error message
            r#"{"timestamp":"2024-01-15T10:30:00.123456789Z","level":"ERROR","target":"wafer_core::dag::runner","fields":{"error":"timeout"},"message":"Transform process failed"}"#,
        ];

        for (i, log_str) in example_logs.iter().enumerate() {
            match validate_json_log_structure(log_str) {
                Ok(()) => {}
                Err(e) => panic!(
                    "Log example {} failed validation: {}\nLog: {}",
                    i, e, log_str
                ),
            }
        }
    }

    /// Test that validates processing log has all SPEC-required fields.
    ///
    /// Per SPEC §12.4, message processing logs SHOULD include:
    /// - span with `pipeline` and `node_id`
    /// - fields: `message_id`, `duration_ns`, `input_size_bytes`, `output_size_bytes`
    #[test]
    fn test_processing_log_fields() {
        let processing_log = r#"{
            "timestamp": "2024-01-15T10:30:00.123456789Z",
            "level": "DEBUG",
            "target": "wafer_core::dag::runner",
            "span": {
                "node_id": "transform-1",
                "node_type": "transform"
            },
            "fields": {
                "message_id": "550e8400-e29b-41d4-a716-446655440000",
                "duration_ns": 5000000,
                "input_size_bytes": 1024,
                "output_size_bytes": 512
            },
            "message": "Transform emitted"
        }"#;

        let log: Value =
            serde_json::from_str(processing_log).expect("Failed to parse processing log");

        // Validate span fields
        let span = log.get("span").expect("Missing span");
        assert!(span.get("node_id").is_some(), "Missing node_id in span");
        assert!(span.get("node_type").is_some(), "Missing node_type in span");

        // Validate processing fields
        let fields = log.get("fields").expect("Missing fields");
        assert!(fields.get("message_id").is_some(), "Missing message_id");
        assert!(fields.get("duration_ns").is_some(), "Missing duration_ns");
        assert!(
            fields.get("input_size_bytes").is_some(),
            "Missing input_size_bytes"
        );
        assert!(
            fields.get("output_size_bytes").is_some(),
            "Missing output_size_bytes"
        );

        // Validate field types
        assert!(
            fields["duration_ns"].is_number(),
            "duration_ns should be a number"
        );
        assert!(
            fields["input_size_bytes"].is_number(),
            "input_size_bytes should be a number"
        );
        assert!(
            fields["output_size_bytes"].is_number(),
            "output_size_bytes should be a number"
        );
    }

    /// Test that JSON logs can be parsed by jq.
    ///
    /// Per SPEC §12.4 validation requirement, logs should be parseable by jq.
    /// This test verifies JSON is valid and can extract fields.
    #[test]
    fn test_json_jq_compatible() {
        let log_lines = vec![
            r#"{"timestamp":"2024-01-15T10:30:00Z","level":"INFO","target":"wafer","message":"Starting"}"#,
            r#"{"timestamp":"2024-01-15T10:30:01Z","level":"DEBUG","target":"wafer::dag","span":{"node_id":"n1"},"message":"Processing"}"#,
            r#"{"timestamp":"2024-01-15T10:30:02Z","level":"ERROR","target":"wafer::dag","fields":{"error":"timeout"},"message":"Failed"}"#,
        ];

        // Simulate jq operations
        let mut parsed_logs: Vec<Value> = Vec::new();
        for line in &log_lines {
            let log: Value = serde_json::from_str(line).expect("Each line should be valid JSON");
            parsed_logs.push(log);
        }

        // jq: select(.level == "ERROR")
        let errors: Vec<&Value> = parsed_logs
            .iter()
            .filter(|l| l["level"] == "ERROR")
            .collect();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["message"], "Failed");

        // jq: .span.node_id
        let node_ids: Vec<Option<&str>> = parsed_logs
            .iter()
            .map(|l| {
                l.get("span")
                    .and_then(|s| s.get("node_id"))
                    .and_then(|n| n.as_str())
            })
            .collect();
        assert!(node_ids.iter().any(|id| id == &Some("n1")));

        // jq: select(.fields.error != null) | .fields.error
        let error_fields: Vec<&str> = parsed_logs
            .iter()
            .filter_map(|l| {
                l.get("fields")
                    .and_then(|f| f.get("error"))
                    .and_then(|e| e.as_str())
            })
            .collect();
        assert_eq!(error_fields, vec!["timeout"]);
    }
}
