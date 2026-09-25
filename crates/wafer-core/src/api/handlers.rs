//! HTTP request handlers for the new pipeline orchestrator.

use std::fmt::Write as _;
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
    pub replacement_eligible: bool,
}

/// Reconfigure request body.
#[derive(Deserialize)]
pub struct ReconfigureRequest {
    /// New node configuration as an arbitrary JSON object.
    pub config: serde_json::Value,
    /// P0.12 (A5 residual): optional SHA-256 (hex) of the plugin bytes the
    /// caller believes are currently loaded. When set and non-empty, the
    /// server compares this against the cached hash from the last
    /// successful hot-swap. Mismatch → 409 CONFLICT.
    #[serde(default)]
    pub expected_plugin_hash: Option<String>,
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
            let state =
                orch.node_state(id).map_or_else(|| "Unknown".to_string(), |s| format!("{s:?}"));
            let (processed, failed) =
                orch.node_metrics(id).map_or((0, 0), |m| (m.processed(), m.failed()));
            NodeInfoResponse {
                id: id.clone(),
                state,
                processed,
                failed,
                replacement_eligible: swappable.contains(&id.as_str()),
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
    let (processed, failed) =
        orch.node_metrics(&id).map_or((0, 0), |m| (m.processed(), m.failed()));
    let swappable = orch.swappable_nodes().contains(&id.as_str());

    Ok(Json(NodeInfoResponse {
        id,
        state: format!("{state:?}"),
        processed,
        failed,
        replacement_eligible: swappable,
    }))
}

fn replacement_guard_error(error: &crate::error::WaferError) -> (StatusCode, String) {
    let message = error.to_string();
    let status = if message.starts_with("swap-in-progress") {
        StatusCode::CONFLICT
    } else if message.starts_with("node-not-swappable") {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (status, message)
}

/// POST /api/v1/nodes/:id/hot-swap — trigger hot-swap with new Wasm binary
#[expect(
    clippy::too_many_lines,
    reason = "multi-step hot-swap procedure (guard → load → compile → swap → canary → respond): linear sequence"
)]
pub async fn hot_swap(
    State(orch): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<HotSwapRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    use crate::config::NodeDef;
    use crate::orchestrator::hotswap::{
        prepare_filter_swap_timed, prepare_router_swap_timed,
        prepare_transform_swap_timed_with_fuel,
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
    let engine_config = orch.config();

    let _replacement_guard =
        orch.try_begin_swap(&id).map_err(|error| replacement_guard_error(&error))?;

    let (kind, capabilities, memory_limit, transform_fuel) = match engine_config.nodes.get(&id) {
        Some(NodeDef::Transform(wasm)) => (
            SwapKind::Transform,
            capabilities_from_config(&wasm.capabilities),
            wasm.memory_limit.unwrap_or(engine_config.engine.memory.transform),
            wasm.fuel.or(engine_config.engine.fuel.transform),
        ),
        Some(NodeDef::Filter(wasm)) => (
            SwapKind::Filter,
            capabilities_from_config(&wasm.capabilities),
            wasm.memory_limit.unwrap_or(engine_config.engine.memory.filter),
            None,
        ),
        Some(NodeDef::Router(wasm)) => (
            SwapKind::Router,
            capabilities_from_config(&wasm.capabilities),
            wasm.memory_limit.unwrap_or(engine_config.engine.memory.router),
            None,
        ),
        Some(NodeDef::Source(_) | NodeDef::Sink(_)) => {
            return Err((StatusCode::NOT_FOUND, format!("node '{id}' does not support hot-swap")));
        }
        None => return Err((StatusCode::NOT_FOUND, format!("node '{id}' not found"))),
    };

    let wasm_bytes = tokio::fs::read(&body.wasm_path)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("failed to read wasm file: {e}")))?;

    let (progress, completion_rx) = HotSwapProgress::channel();
    let timed_result = match kind {
        SwapKind::Transform => {
            prepare_transform_swap_timed_with_fuel(
                engine,
                &wasm_bytes,
                &id,
                capabilities,
                memory_limit,
                transform_fuel,
                progress,
            )
            .await
        }
        SwapKind::Filter => {
            prepare_filter_swap_timed(
                engine,
                &wasm_bytes,
                &id,
                capabilities,
                memory_limit,
                progress,
            )
            .await
        }
        SwapKind::Router => {
            prepare_router_swap_timed(
                engine,
                &wasm_bytes,
                &id,
                capabilities,
                memory_limit,
                progress,
            )
            .await
        }
    };
    let mut timed_result = timed_result.map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("swap preparation failed: {e}"))
    })?;

    let signal_at = std::time::Instant::now();
    timed_result.timeline.mark_signal_sent();
    orch.send_swap(&id, timed_result.payload)
        .map_err(|e| (StatusCode::NOT_FOUND, format!("{e}")))?;

    let completion = tokio::time::timeout(std::time::Duration::from_secs(5), completion_rx).await;
    let report = match completion {
        Ok(Ok(Ok(report))) => report,
        Ok(Ok(Err(crate::runner::HotSwapError::RolledBack { rollback_time_ns, reason }))) => {
            let compile_ns = timed_result.timeline.compile_duration_ns().unwrap_or(0);
            let instantiate_ns = timed_result.timeline.instantiate_duration_ns().unwrap_or(0);
            let signal_ns = timed_result.timeline.signal_duration_ns().unwrap_or(0);
            orch.record_hotswap_phase("compile", &id, compile_ns);
            orch.record_hotswap_phase("instantiate", &id, instantiate_ns);
            orch.record_hotswap_phase("signal", &id, signal_ns);
            orch.record_hotswap_phase("rollback", &id, rollback_time_ns);
            return Ok(Json(serde_json::json!({
                "node_id": id,
                "status": "rolled_back",
                "reason": reason,
                "timeline": {
                    "compile_ns": timed_result.timeline.compile_duration_ns(),
                    "instantiate_ns": timed_result.timeline.instantiate_duration_ns(),
                    "signal_ns": timed_result.timeline.signal_duration_ns(),
                    "rollback_ns": rollback_time_ns,
                }
            }))
            .into_response());
        }
        Ok(Ok(Err(err))) => return Err((StatusCode::CONFLICT, err.to_string())),
        Ok(Err(_)) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "hot-swap runner exited before replacement adoption".to_string(),
            ));
        }
        Err(_) => {
            return Err((
                StatusCode::GATEWAY_TIMEOUT,
                "hot-swap did not report a local post-replacement outcome within 5s".to_string(),
            ));
        }
    };

    let replacement_adopted_ns = crate::util::duration_ns_saturating(
        report.replacement_adopted_at.duration_since(signal_at),
    );
    let first_post_replacement_local_outcome_ns = crate::util::duration_ns_saturating(
        report
            .first_post_replacement_local_outcome_at
            .duration_since(report.replacement_adopted_at),
    );
    let compile_ns = timed_result.timeline.compile_duration_ns().unwrap_or(0);
    let instantiate_ns = timed_result.timeline.instantiate_duration_ns().unwrap_or(0);
    let signal_ns = timed_result.timeline.signal_duration_ns().unwrap_or(0);
    orch.record_hotswap_phase("compile", &id, compile_ns);
    orch.record_hotswap_phase("instantiate", &id, instantiate_ns);
    orch.record_hotswap_phase("signal", &id, signal_ns);
    orch.record_hotswap_phase("replacement_adopted", &id, replacement_adopted_ns);
    orch.record_hotswap_phase(
        "first_post_replacement_local_outcome",
        &id,
        first_post_replacement_local_outcome_ns,
    );

    {
        use sha2::{Digest, Sha256};
        orch.record_plugin_hash(&id, hex::encode(Sha256::digest(&wasm_bytes)));
    }

    Ok(Json(serde_json::json!({
        "node_id": id,
        "replacement_adopted": true,
        "first_post_replacement_local_outcome": {
            "disposition": report.first_post_replacement_local_outcome.as_str(),
            "after_adoption_ns": first_post_replacement_local_outcome_ns,
        },
        "timeline": {
            "compile_ns": timed_result.timeline.compile_duration_ns(),
            "instantiate_ns": timed_result.timeline.instantiate_duration_ns(),
            "signal_ns": timed_result.timeline.signal_duration_ns(),
            "replacement_adopted_ns": replacement_adopted_ns,
            "first_post_replacement_local_outcome_ns": first_post_replacement_local_outcome_ns,
        }
    }))
    .into_response())
}

/// POST /api/v1/nodes/:id/reconfigure — warm reconfigure via cached InstancePre
pub async fn reconfigure(
    State(orch): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ReconfigureRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    use crate::config::NodeDef;
    use crate::runner::{HotSwapProgress, SwapPayload};

    let _replacement_guard =
        orch.try_begin_swap(&id).map_err(|error| replacement_guard_error(&error))?;

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

    if let Some(expected) = body.expected_plugin_hash.as_deref().filter(|s| !s.is_empty())
        && let Err(e) = orch.verify_plugin_hash(&id, expected)
    {
        let msg = e.to_string();
        let status = if msg.starts_with("plugin-hash-mismatch") {
            StatusCode::CONFLICT
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        return Err((status, msg));
    }

    let new_config_json = serde_json::to_string(&body.config)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid config json: {e}")))?;

    let (progress, completion_rx) = HotSwapProgress::channel();
    let payload = SwapPayload::Reconfigure { new_config_json, progress };

    let signal_at = std::time::Instant::now();
    orch.send_swap(&id, payload).map_err(|e| (StatusCode::NOT_FOUND, format!("{e}")))?;

    let completion = tokio::time::timeout(std::time::Duration::from_secs(5), completion_rx).await;
    let report = match completion {
        Ok(Ok(Ok(report))) => report,
        Ok(Ok(Err(crate::runner::HotSwapError::RolledBack { rollback_time_ns, reason }))) => {
            return Ok(Json(serde_json::json!({
                "node_id": id,
                "status": "rolled_back",
                "reason": reason,
                "timeline": { "rollback_ns": rollback_time_ns }
            }))
            .into_response());
        }
        Ok(Ok(Err(err))) => return Err((StatusCode::CONFLICT, err.to_string())),
        Ok(Err(_)) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "reconfigure runner exited before replacement adoption".to_string(),
            ));
        }
        Err(_) => {
            return Err((
                StatusCode::GATEWAY_TIMEOUT,
                "reconfigure did not report a local post-replacement outcome within 5s".to_string(),
            ));
        }
    };

    let replacement_adopted_ns = crate::util::duration_ns_saturating(
        report.replacement_adopted_at.duration_since(signal_at),
    );
    let first_post_replacement_local_outcome_ns = crate::util::duration_ns_saturating(
        report
            .first_post_replacement_local_outcome_at
            .duration_since(report.replacement_adopted_at),
    );

    Ok(Json(serde_json::json!({
        "node_id": id,
        "replacement_adopted": true,
        "first_post_replacement_local_outcome": {
            "disposition": report.first_post_replacement_local_outcome.as_str(),
            "after_adoption_ns": first_post_replacement_local_outcome_ns,
        },
        "timeline": {
            "compile_ns": 0u64,
            "instantiate_ns": 0u64,
            "signal_ns": 0u64,
            "replacement_adopted_ns": replacement_adopted_ns,
            "first_post_replacement_local_outcome_ns": first_post_replacement_local_outcome_ns,
        }
    }))
    .into_response())
}

/// POST /api/v1/pipeline/shutdown — trigger graceful shutdown
pub async fn shutdown(State(orch): State<AppState>) -> StatusCode {
    orch.cancel();
    StatusCode::OK
}

/// GET /metrics — Prometheus-style text metrics
#[expect(
    clippy::indexing_slicing,
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "bucket indices come from enumerate() over same-length arrays; write!/writeln! into String is infallible per std::fmt::Write for String; Prometheus text format has many series to emit"
)]
pub async fn metrics(State(orch): State<AppState>) -> impl IntoResponse {
    let mut output = String::new();

    for node_id in orch.config().nodes.keys() {
        if let Some(m) = orch.node_metrics(node_id) {
            writeln!(output, "wafer_node_processed_total{{node=\"{node_id}\"}} {}", m.processed())
                .expect("String write is infallible");
            writeln!(output, "wafer_node_failed_total{{node=\"{node_id}\"}} {}", m.failed())
                .expect("String write is infallible");
            writeln!(
                output,
                "wafer_node_retry_exhausted_skip_total{{node=\"{node_id}\"}} {}",
                m.exhausted_skips()
            )
            .expect("String write is infallible");
        }
    }

    let hotswap = orch.hotswap_metrics();
    if let Ok(guard) = hotswap.phase_histogram.read() {
        output
            .push_str("# HELP hot_swap_phase_ns Nanoseconds per runner-local replacement phase.\n");
        output.push_str("# TYPE hot_swap_phase_ns histogram\n");
        for ((phase, node_id), hist) in guard.iter() {
            for (i, upper) in crate::metrics::types::PhaseHistogram::BUCKETS_NS.iter().enumerate() {
                let count = hist.buckets[i].load(std::sync::atomic::Ordering::Relaxed);
                writeln!(
                    output,
                    "hot_swap_phase_ns_bucket{{phase=\"{phase}\",node_id=\"{node_id}\",le=\"{upper}\"}} {count}"
                )
                .expect("String write is infallible");
            }
            let total = hist.count.load(std::sync::atomic::Ordering::Relaxed);
            let sum = hist.sum_ns.load(std::sync::atomic::Ordering::Relaxed);
            writeln!(
                output,
                "hot_swap_phase_ns_bucket{{phase=\"{phase}\",node_id=\"{node_id}\",le=\"+Inf\"}} {total}"
            )
            .expect("String write is infallible");
            writeln!(
                output,
                "hot_swap_phase_ns_sum{{phase=\"{phase}\",node_id=\"{node_id}\"}} {sum}"
            )
            .expect("String write is infallible");
            writeln!(
                output,
                "hot_swap_phase_ns_count{{phase=\"{phase}\",node_id=\"{node_id}\"}} {total}"
            )
            .expect("String write is infallible");
        }
    }

    output.push_str(
        "# HELP wafer_node_recovery_duration_ms Node Error → Recovering → Running duration (P0.11).\n",
    );
    output.push_str("# TYPE wafer_node_recovery_duration_ms summary\n");
    for node_id in orch.config().nodes.keys() {
        if let Some(m) = orch.node_metrics(node_id) {
            let count = m.recovery_count();
            if count > 0 {
                let sum_ms = m.recovery_ns_total() / 1_000_000;
                let max_ms = m.recovery_max_ns() / 1_000_000;
                writeln!(
                    output,
                    "wafer_node_recovery_duration_ms_count{{node_id=\"{node_id}\"}} {count}"
                )
                .expect("String write is infallible");
                writeln!(
                    output,
                    "wafer_node_recovery_duration_ms_sum{{node_id=\"{node_id}\"}} {sum_ms}"
                )
                .expect("String write is infallible");
                writeln!(
                    output,
                    "wafer_node_recovery_duration_ms{{node_id=\"{node_id}\",quantile=\"max\"}} {max_ms}"
                )
                .expect("String write is infallible");
            }
        }
    }
    if let Ok(guard) = hotswap.recovery_duration.read()
        && !guard.is_empty()
    {
        output.push_str("# TYPE wafer_node_recovery_duration_ms_bucket histogram\n");
        for (node_id, hist) in guard.iter() {
            for (i, upper_ns) in
                crate::metrics::types::PhaseHistogram::BUCKETS_NS.iter().enumerate()
            {
                let count = hist.buckets[i].load(std::sync::atomic::Ordering::Relaxed);
                let upper_ms = upper_ns / 1_000_000;
                writeln!(
                    output,
                    "wafer_node_recovery_duration_ms_bucket{{node_id=\"{node_id}\",le=\"{upper_ms}\"}} {count}"
                )
                .expect("String write is infallible");
            }
            let total = hist.count.load(std::sync::atomic::Ordering::Relaxed);
            let sum_ms = hist.sum_ns.load(std::sync::atomic::Ordering::Relaxed) / 1_000_000;
            writeln!(
                output,
                "wafer_node_recovery_duration_ms_bucket{{node_id=\"{node_id}\",le=\"+Inf\"}} {total}"
            )
            .expect("String write is infallible");
            writeln!(
                output,
                "wafer_node_recovery_duration_ms_sum{{node_id=\"{node_id}\"}} {sum_ms}"
            )
            .expect("String write is infallible");
            writeln!(
                output,
                "wafer_node_recovery_duration_ms_count{{node_id=\"{node_id}\"}} {total}"
            )
            .expect("String write is infallible");
        }
    }

    ([(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4")], output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::FirstPostReplacementLocalOutcome;
    use axum::response::Response;
    use tokio::sync::watch;

    const MNIST_COMPONENT: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../wafer-runtime/tests/fixtures/mnist-inference.component.bin"
    );

    fn replacement_handle(
        allow_inference: bool,
    ) -> (Arc<PipelineHandle>, watch::Receiver<Option<crate::runner::SwapPayload>>) {
        let config: crate::config::Config = toml::from_str(&format!(
            r#"
[engine.fuel]
transform = 1

[engine.memory]
transform = 1

[nodes.mnist]
type = "transform"
plugin = {MNIST_COMPONENT:?}
fuel = 100000000
memory_limit = 67108864

[nodes.mnist.capabilities]
allow_inference = {allow_inference}
"#,
        ))
        .expect("inference replacement config");
        let (sender, receiver) = watch::channel(None);
        (
            Arc::new(
                PipelineHandle::for_replacement_test(config, "mnist", sender)
                    .expect("replacement test handle"),
            ),
            receiver,
        )
    }

    #[tokio::test]
    async fn granted_inference_endpoint_reports_local_adoption_separately() {
        let (handle, mut swaps) = replacement_handle(true);
        let request = HotSwapRequest { wasm_path: MNIST_COMPONENT.to_string() };
        let response = tokio::spawn(async move {
            hot_swap(State(handle), Path("mnist".to_string()), Json(request))
                .await
                .map(IntoResponse::into_response)
        });

        tokio::time::timeout(std::time::Duration::from_secs(5), swaps.changed())
            .await
            .expect("replacement signal timeout")
            .expect("replacement sender");
        let payload = swaps.borrow_and_update().clone().expect("replacement payload");
        assert!(payload.is_inference_transform(), "endpoint downgraded inference preparation");
        assert!(!response.is_finished(), "endpoint completed before runner-local evidence");

        let progress = payload.progress();
        progress.mark_replacement_adopted();
        tokio::task::yield_now().await;
        assert!(!response.is_finished(), "adoption alone must not imply a local outcome");
        progress.mark_first_post_replacement_local_outcome(
            FirstPostReplacementLocalOutcome::ForwardedEnqueued,
        );

        let response: Response =
            response.await.expect("handler task").expect("granted inference response");
        assert_eq!(response.status(), StatusCode::OK);
        let body =
            axum::body::to_bytes(response.into_body(), usize::MAX).await.expect("response body");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("response json");
        assert_eq!(body["replacement_adopted"], true);
        assert_eq!(
            body["first_post_replacement_local_outcome"]["disposition"],
            "forwarded/enqueued"
        );
    }

    #[tokio::test]
    async fn inference_reconfigure_waits_for_runner_local_outcome() {
        let (handle, mut swaps) = replacement_handle(true);
        let request = ReconfigureRequest {
            config: serde_json::json!({"execution_target": "cpu"}),
            expected_plugin_hash: None,
        };
        let response = tokio::spawn(async move {
            reconfigure(State(handle), Path("mnist".to_string()), Json(request))
                .await
                .map(IntoResponse::into_response)
        });

        tokio::time::timeout(std::time::Duration::from_secs(5), swaps.changed())
            .await
            .expect("reconfigure signal timeout")
            .expect("reconfigure sender");
        let payload = swaps.borrow_and_update().clone().expect("reconfigure payload");
        match &payload {
            crate::runner::SwapPayload::Reconfigure { new_config_json, .. } => {
                assert_eq!(new_config_json, r#"{"execution_target":"cpu"}"#);
            }
            _ => panic!("expected reconfigure payload"),
        }
        assert!(!response.is_finished(), "reconfigure completed before runner-local evidence");

        let progress = payload.progress();
        progress.mark_replacement_adopted();
        assert!(!response.is_finished(), "adoption alone must not imply a local outcome");
        progress.mark_first_post_replacement_local_outcome(
            FirstPostReplacementLocalOutcome::ForwardedEnqueued,
        );

        let response: Response =
            response.await.expect("handler task").expect("granted reconfigure response");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn ungranted_inference_endpoint_fails_before_signaling_runner() {
        let (handle, swaps) = replacement_handle(false);
        let request = HotSwapRequest { wasm_path: MNIST_COMPONENT.to_string() };

        let error = hot_swap(State(handle), Path("mnist".to_string()), Json(request))
            .await
            .map(IntoResponse::into_response)
            .expect_err("ungranted inference replacement must fail");

        assert_eq!(error.0, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(error.1.contains("wasi:nn/"), "unexpected denial: {}", error.1);
        assert!(swaps.borrow().is_none(), "denied swap reached the runner");
    }

    #[tokio::test]
    async fn metrics_omit_deferred_rollback_counter() {
        let handle = PipelineHandle::for_p0_10_test(&["transform"]);
        let response = metrics(State(Arc::new(handle))).await.into_response();
        let body =
            axum::body::to_bytes(response.into_body(), usize::MAX).await.expect("metrics body");
        let text = std::str::from_utf8(&body).expect("metrics utf8");
        assert!(
            !text.contains("wafer_hot_swap_rollbacks_total"),
            "A20 is deferred and must not be exposed through Prometheus"
        );
    }
}
