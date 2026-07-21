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
    let engine_config = orch.config();

    // Acquire the per-node swap slot before doing ANY preparation. This is
    // the P0.10 overlapping-swap guard: a concurrent second call for the
    // same node id short-circuits here with 409 CONFLICT instead of
    // overwriting the pending watch value.
    let _swap_guard = match orch.try_begin_swap(&id) {
        Ok(g) => g,
        Err(e) => {
            let msg = e.to_string();
            let status = if msg.starts_with("swap-in-progress") {
                StatusCode::CONFLICT
            } else if msg.starts_with("node-not-swappable") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            return Err((status, msg));
        }
    };

    let (kind, capabilities, memory_limit) = match engine_config.nodes.get(&id) {
        Some(NodeDef::Transform(wasm)) => {
            // Reject hot-swap attempts on native baseline transforms with a
            // 400 Bad Request. Native nodes are by construction not
            // swappable (RFC-008 §D5) and a 500 Internal Server Error would
            // wrongly imply a runtime bug when the caller supplied an
            // invalid target.
            if wasm.plugin.is_native() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!(
                        "node '{id}' is a native baseline transform (plugin.kind = 'native'); \
                         native transforms do not support hot-swap"
                    ),
                ));
            }
            (
                SwapKind::Transform,
                capabilities_from_config(&wasm.capabilities),
                wasm.memory_limit.unwrap_or(engine_config.engine.memory.transform),
            )
        },
        Some(NodeDef::Filter(wasm)) => (
            SwapKind::Filter,
            capabilities_from_config(&wasm.capabilities),
            wasm.memory_limit.unwrap_or(engine_config.engine.memory.filter),
        ),
        Some(NodeDef::Router(wasm)) => (
            SwapKind::Router,
            capabilities_from_config(&wasm.capabilities),
            wasm.memory_limit.unwrap_or(engine_config.engine.memory.router),
        ),
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
            prepare_transform_swap_timed(engine, &wasm_bytes, &id, capabilities, memory_limit, progress).await
        }
        SwapKind::Filter => {
            prepare_filter_swap_timed(engine, &wasm_bytes, &id, capabilities, memory_limit, progress).await
        }
        SwapKind::Router => {
            prepare_router_swap_timed(engine, &wasm_bytes, &id, capabilities, memory_limit, progress).await
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
    let first_v2_ns = report
        .first_v2_at
        .duration_since(signal_at)
        .as_nanos() as u64;

    // P0.10 AC1: record every phase into the hot_swap_phase_ns histogram
    // labelled {phase, node_id}. Six phases total — curl :9090/metrics | rg
    // hot_swap_phase_ns must show six series after this call.
    let compile_ns = timed_result.timeline.compile_duration_ns().unwrap_or(0);
    let instantiate_ns = timed_result.timeline.instantiate_duration_ns().unwrap_or(0);
    let signal_ns = timed_result.timeline.signal_duration_ns().unwrap_or(0);
    orch.record_hotswap_phase("compile", &id, compile_ns);
    orch.record_hotswap_phase("instantiate", &id, instantiate_ns);
    orch.record_hotswap_phase("signal", &id, signal_ns);
    orch.record_hotswap_phase("ack", &id, ack_ns);
    orch.record_hotswap_phase("first_v2", &id, first_v2_ns);
    orch.record_hotswap_phase("convergence", &id, convergence_ns);

    // P0.12 AC1: cache the SHA-256 of the plugin bytes so `/reconfigure`
    // can reject callers whose mental model has diverged from the
    // actually-running binary.
    {
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(&wasm_bytes);
        orch.record_plugin_hash(&id, hex::encode(hash));
    }

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
        Some(NodeDef::Transform(wasm)) => {
            // Reject reconfigure on native baseline transforms with a
            // 400 Bad Request. Same rationale as /hot-swap: native nodes
            // are by construction not swappable/reconfigurable.
            if wasm.plugin.is_native() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!(
                        "node '{id}' is a native baseline transform (plugin.kind = 'native'); \
                         native transforms do not support reconfigure"
                    ),
                ));
            }
        }
        Some(NodeDef::Filter(_) | NodeDef::Router(_)) => {}
        Some(NodeDef::Source(_) | NodeDef::Sink(_)) => {
            return Err((
                StatusCode::NOT_FOUND,
                format!("node '{id}' does not support reconfigure"),
            ));
        }
        None => return Err((StatusCode::NOT_FOUND, format!("node '{id}' not found"))),
    }

    // P0.12 AC1: verify the caller-supplied plugin hash matches the
    // cached hash for this node. When the caller does not supply one,
    // fall through (backward-compat). When they do and it mismatches,
    // 409 CONFLICT with a body starting `plugin-hash-mismatch`.
    if let Some(expected) = body.expected_plugin_hash.as_deref().filter(|s| !s.is_empty()) {
        if let Err(e) = orch.verify_plugin_hash(&id, expected) {
            let msg = e.to_string();
            let status = if msg.starts_with("plugin-hash-mismatch") {
                StatusCode::CONFLICT
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            return Err((status, msg));
        }
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

    // P0.10 AC1: hot_swap_phase_ns histogram, one series set per
    // (phase, node_id). Emitted whenever the /metrics endpoint is
    // scraped; empty when no swaps have happened yet.
    let hotswap = orch.hotswap_metrics();
    if let Ok(guard) = hotswap.phase_histogram.read() {
        output.push_str(
            "# HELP hot_swap_phase_ns Nanoseconds per hot-swap phase (P0.10, RFC-008 E-Swap-6).\n",
        );
        output.push_str("# TYPE hot_swap_phase_ns histogram\n");
        for ((phase, node_id), hist) in guard.iter() {
            for (i, upper) in crate::metrics::types::PhaseHistogram::BUCKETS_NS.iter().enumerate() {
                let count = hist.buckets[i].load(std::sync::atomic::Ordering::Relaxed);
                output.push_str(&format!(
                    "hot_swap_phase_ns_bucket{{phase=\"{phase}\",node_id=\"{node_id}\",le=\"{upper}\"}} {count}\n"
                ));
            }
            let total = hist.count.load(std::sync::atomic::Ordering::Relaxed);
            let sum = hist.sum_ns.load(std::sync::atomic::Ordering::Relaxed);
            output.push_str(&format!(
                "hot_swap_phase_ns_bucket{{phase=\"{phase}\",node_id=\"{node_id}\",le=\"+Inf\"}} {total}\n"
            ));
            output.push_str(&format!(
                "hot_swap_phase_ns_sum{{phase=\"{phase}\",node_id=\"{node_id}\"}} {sum}\n"
            ));
            output.push_str(&format!(
                "hot_swap_phase_ns_count{{phase=\"{phase}\",node_id=\"{node_id}\"}} {total}\n"
            ));
        }
    }

    // P0.11 AC2: wafer_node_recovery_duration_ms per-node summary
    // (count + sum + max). Runners record durations into NodeMetrics on
    // every Recovering → Running transition; a fuller HdrHistogram-shaped
    // dataset lives in HotSwapMetrics.recovery_duration when explicit
    // record_recovery_duration() calls are made (currently only from
    // tests, since the runners write directly into NodeMetrics). Both
    // surfaces render below — whichever is populated for a given node
    // ID.
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
                output.push_str(&format!(
                    "wafer_node_recovery_duration_ms_count{{node_id=\"{node_id}\"}} {count}\n"
                ));
                output.push_str(&format!(
                    "wafer_node_recovery_duration_ms_sum{{node_id=\"{node_id}\"}} {sum_ms}\n"
                ));
                output.push_str(&format!(
                    "wafer_node_recovery_duration_ms{{node_id=\"{node_id}\",quantile=\"max\"}} {max_ms}\n"
                ));
            }
        }
    }
    // Full histogram (bucketed) is available when record_recovery_duration
    // was called explicitly; preserved for symmetry with hot_swap_phase_ns.
    if let Ok(guard) = hotswap.recovery_duration.read()
        && !guard.is_empty()
    {
        output.push_str(
            "# TYPE wafer_node_recovery_duration_ms_bucket histogram\n",
        );
        for (node_id, hist) in guard.iter() {
            for (i, upper_ns) in crate::metrics::types::PhaseHistogram::BUCKETS_NS.iter().enumerate() {
                let count = hist.buckets[i].load(std::sync::atomic::Ordering::Relaxed);
                let upper_ms = upper_ns / 1_000_000;
                output.push_str(&format!(
                    "wafer_node_recovery_duration_ms_bucket{{node_id=\"{node_id}\",le=\"{upper_ms}\"}} {count}\n"
                ));
            }
            let total = hist.count.load(std::sync::atomic::Ordering::Relaxed);
            let sum_ms = hist.sum_ns.load(std::sync::atomic::Ordering::Relaxed) / 1_000_000;
            output.push_str(&format!(
                "wafer_node_recovery_duration_ms_bucket{{node_id=\"{node_id}\",le=\"+Inf\"}} {total}\n"
            ));
            output.push_str(&format!(
                "wafer_node_recovery_duration_ms_sum{{node_id=\"{node_id}\"}} {sum_ms}\n"
            ));
            output.push_str(&format!(
                "wafer_node_recovery_duration_ms_count{{node_id=\"{node_id}\"}} {total}\n"
            ));
        }
    }

    ([(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4")], output)
}
