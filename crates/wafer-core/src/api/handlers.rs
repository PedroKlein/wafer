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
use crate::runner::error_policy::{ErrorCategory, TrapKind};

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
                orch.node_metrics(id).map_or((0, 0), |m| (m.processed(), m.attempts_failed()));
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
        orch.node_metrics(&id).map_or((0, 0), |m| (m.processed(), m.attempts_failed()));
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

    let resolve_capabilities = |capabilities| {
        capabilities_from_config(capabilities)
            .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))
    };
    let (kind, capabilities, memory_limit, transform_fuel) = match engine_config.nodes.get(&id) {
        Some(NodeDef::Transform(wasm)) => (
            SwapKind::Transform,
            resolve_capabilities(&wasm.capabilities)?,
            wasm.memory_limit.unwrap_or(engine_config.engine.memory.transform),
            wasm.fuel.or(engine_config.engine.fuel.transform),
        ),
        Some(NodeDef::Filter(wasm)) => (
            SwapKind::Filter,
            resolve_capabilities(&wasm.capabilities)?,
            wasm.memory_limit.unwrap_or(engine_config.engine.memory.filter),
            None,
        ),
        Some(NodeDef::Router(wasm)) => (
            SwapKind::Router,
            resolve_capabilities(&wasm.capabilities)?,
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

    let (progress, mut completion_rx) = HotSwapProgress::channel();
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

    let progress = timed_result.payload.progress();
    let signal_at = std::time::Instant::now();
    timed_result.timeline.mark_signal_sent();
    orch.send_swap(&id, timed_result.payload)
        .map_err(|e| (StatusCode::NOT_FOUND, format!("{e}")))?;

    let completion = tokio::time::timeout(REPLACEMENT_OUTCOME_WAIT, &mut completion_rx).await;
    let report = match completion {
        Ok(Ok(Ok(report))) => report,
        Ok(Ok(Err(crate::runner::HotSwapError::RolledBack { rollback_time_ns, reason }))) => {
            record_preparation_phases(&orch, &id, &timed_result.timeline);
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
            let adopted_at =
                unfinished_replacement(&orch, &id, &progress, &mut completion_rx, "hot-swap")
                    .await?;
            record_preparation_phases(&orch, &id, &timed_result.timeline);
            let replacement_adopted_ns = adopted_at.map(|at| {
                let ns = crate::util::duration_ns_saturating(at.duration_since(signal_at));
                orch.record_hotswap_phase("replacement_adopted", &id, ns);
                record_wasm_hash(&orch, &id, &wasm_bytes);
                ns
            });
            return Ok((
                StatusCode::ACCEPTED,
                Json(serde_json::json!({
                    "node_id": id,
                    "replacement_adopted": adopted_at.is_some(),
                    "first_post_replacement_local_outcome": null,
                    "timeline": {
                        "compile_ns": timed_result.timeline.compile_duration_ns(),
                        "instantiate_ns": timed_result.timeline.instantiate_duration_ns(),
                        "signal_ns": timed_result.timeline.signal_duration_ns(),
                        "replacement_adopted_ns": replacement_adopted_ns,
                        "first_post_replacement_local_outcome_ns": null,
                    }
                })),
            )
                .into_response());
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
    record_preparation_phases(&orch, &id, &timed_result.timeline);
    orch.record_hotswap_phase("replacement_adopted", &id, replacement_adopted_ns);
    orch.record_hotswap_phase(
        "first_post_replacement_local_outcome",
        &id,
        first_post_replacement_local_outcome_ns,
    );

    record_wasm_hash(&orch, &id, &wasm_bytes);

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

/// How long a swap or reconfigure waits for the first runner-local outcome
/// on the replacement before answering without it.
const REPLACEMENT_OUTCOME_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// How long a replacement the runner already claimed may keep running
/// validate/init after [`REPLACEMENT_OUTCOME_WAIT`] before the caller is
/// answered without knowing whether it was adopted.
const CLAIMED_REPLACEMENT_INIT_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

fn record_preparation_phases(
    orch: &PipelineHandle,
    id: &str,
    timeline: &crate::orchestrator::hotswap::SwapTimeline,
) {
    orch.record_hotswap_phase("compile", id, timeline.compile_duration_ns().unwrap_or(0));
    orch.record_hotswap_phase("instantiate", id, timeline.instantiate_duration_ns().unwrap_or(0));
    orch.record_hotswap_phase("signal", id, timeline.signal_duration_ns().unwrap_or(0));
}

fn record_wasm_hash(orch: &PipelineHandle, id: &str, wasm_bytes: &[u8]) {
    use sha2::{Digest, Sha256};
    orch.record_plugin_hash(id, hex::encode(Sha256::digest(wasm_bytes)));
}

/// Settle a replacement whose runner-local outcome did not arrive in time.
///
/// If the runner never took the payload, withdraw it so it cannot apply
/// after the caller is told it failed, and return 504. Otherwise the runner
/// owns it: wait for its validate/init to adopt the replacement or fail, so
/// the caller is not answered while the swap can still change the node.
/// Returns `None` only if that init outlives [`CLAIMED_REPLACEMENT_INIT_WAIT`].
async fn unfinished_replacement(
    orch: &PipelineHandle,
    id: &str,
    progress: &Arc<crate::runner::HotSwapProgress>,
    completion_rx: &mut tokio::sync::oneshot::Receiver<crate::runner::HotSwapOutcome>,
    operation: &str,
) -> Result<Option<std::time::Instant>, (StatusCode, String)> {
    if progress.try_withdraw() {
        orch.retract_swap(id, progress);
        return Err((
            StatusCode::GATEWAY_TIMEOUT,
            format!(
                "{operation} was not adopted within 5s and was withdrawn; the node keeps its current plugin"
            ),
        ));
    }
    let settled = tokio::time::timeout(CLAIMED_REPLACEMENT_INIT_WAIT, async {
        tokio::select! {
            biased;
            at = progress.replacement_adopted() => Ok(at),
            outcome = completion_rx => match outcome {
                Ok(Ok(report)) => Ok(report.replacement_adopted_at),
                Ok(Err(err)) => Err((StatusCode::CONFLICT, err.to_string())),
                Err(_) => Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("{operation} runner exited before replacement adoption"),
                )),
            },
        }
    })
    .await;
    settled
        .map_or_else(|_| Ok(progress.replacement_adopted_at()), |adopted_at| adopted_at.map(Some))
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

    let (progress, mut completion_rx) = HotSwapProgress::channel();
    let payload = SwapPayload::Reconfigure { new_config_json, progress: Arc::clone(&progress) };

    let signal_at = std::time::Instant::now();
    orch.send_swap(&id, payload).map_err(|e| (StatusCode::NOT_FOUND, format!("{e}")))?;

    let completion = tokio::time::timeout(REPLACEMENT_OUTCOME_WAIT, &mut completion_rx).await;
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
            let adopted_at =
                unfinished_replacement(&orch, &id, &progress, &mut completion_rx, "reconfigure")
                    .await?;
            let replacement_adopted_ns = adopted_at
                .map(|at| crate::util::duration_ns_saturating(at.duration_since(signal_at)));
            return Ok((
                StatusCode::ACCEPTED,
                Json(serde_json::json!({
                    "node_id": id,
                    "replacement_adopted": adopted_at.is_some(),
                    "first_post_replacement_local_outcome": null,
                    "timeline": {
                        "compile_ns": 0u64,
                        "instantiate_ns": 0u64,
                        "signal_ns": 0u64,
                        "replacement_adopted_ns": replacement_adopted_ns,
                        "first_post_replacement_local_outcome_ns": null,
                    }
                })),
            )
                .into_response());
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
            for (name, value) in [
                ("failed", m.attempts_failed()),
                ("filtered_out", m.filtered_out()),
                ("retries", m.retries()),
                ("dlq_sent", m.dlq_sent()),
                ("dlq_lost", m.dlq_lost()),
                ("skipped", m.skipped()),
                ("retry_exhausted_skip", m.exhausted_skips()),
                ("dropped_on_recovery", m.dropped_on_recovery()),
                ("dropped_on_teardown", m.dropped_on_teardown()),
            ] {
                writeln!(output, "wafer_node_{name}_total{{node=\"{node_id}\"}} {value}")
                    .expect("String write is infallible");
            }
            for kind in TrapKind::ALL {
                writeln!(
                    output,
                    "wafer_node_traps_total{{node=\"{node_id}\",kind=\"{}\"}} {}",
                    kind.as_str(),
                    m.traps(kind)
                )
                .expect("String write is infallible");
            }
            for category in ErrorCategory::ALL {
                writeln!(
                    output,
                    "wafer_node_guest_errors_total{{node=\"{node_id}\",category=\"{category}\"}} {}",
                    m.guest_errors(category)
                )
                .expect("String write is infallible");
            }
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

    // A timed-out request the runner never took is withdrawn, so it
    // cannot apply after the caller was told it failed.
    #[tokio::test(start_paused = true)]
    async fn unadopted_reconfigure_times_out_and_is_withdrawn() {
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

        swaps.changed().await.expect("reconfigure sender");
        let progress = swaps.borrow().clone().expect("reconfigure payload").progress();

        let error = response.await.expect("handler task").expect_err("idle runner must time out");
        assert_eq!(error.0, StatusCode::GATEWAY_TIMEOUT);
        assert!(error.1.contains("withdrawn"), "unexpected message: {}", error.1);
        assert!(swaps.borrow().is_none(), "withdrawn payload is still armed");
        assert!(!progress.try_claim(), "runner could still apply a withdrawn payload");
    }

    // When the runner adopted the replacement but no message has
    // arrived yet, the API reports the adoption (202) and records the new
    // plugin hash instead of returning 504.
    #[tokio::test(start_paused = true)]
    async fn adopted_swap_without_traffic_is_accepted_and_hashed() {
        use sha2::Digest as _;

        let (handle, mut swaps) = replacement_handle(true);
        let request = HotSwapRequest { wasm_path: MNIST_COMPONENT.to_string() };
        let api_handle = Arc::clone(&handle);
        let response = tokio::spawn(async move {
            hot_swap(State(api_handle), Path("mnist".to_string()), Json(request))
                .await
                .map(IntoResponse::into_response)
        });

        swaps.changed().await.expect("replacement sender");
        let payload = swaps.borrow_and_update().clone().expect("replacement payload");
        let progress = payload.progress();
        assert!(progress.try_claim(), "runner claims the pending payload");
        progress.mark_replacement_adopted();

        let response: Response =
            response.await.expect("handler task").expect("adopted swap response");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body =
            axum::body::to_bytes(response.into_body(), usize::MAX).await.expect("response body");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("response json");
        assert_eq!(body["replacement_adopted"], true);
        assert!(body["first_post_replacement_local_outcome"].is_null());
        assert!(body["timeline"]["replacement_adopted_ns"].is_u64());

        let wasm_bytes = std::fs::read(MNIST_COMPONENT).expect("fixture bytes");
        let expected = hex::encode(sha2::Sha256::digest(&wasm_bytes));
        assert_eq!(handle.plugin_hashes_snapshot().get("mnist"), Some(&expected));
    }

    // A replacement the runner claimed but is still initializing when the
    // outcome wait ends keeps the request open until it is adopted, so the
    // caller never gets an answer the swap can still change.
    #[tokio::test(start_paused = true)]
    async fn claimed_reconfigure_waits_for_slow_init_to_adopt() {
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

        swaps.changed().await.expect("reconfigure sender");
        let progress = swaps.borrow_and_update().clone().expect("reconfigure payload").progress();
        assert!(progress.try_claim(), "runner claims the pending payload");
        tokio::time::sleep(REPLACEMENT_OUTCOME_WAIT + std::time::Duration::from_secs(1)).await;
        assert!(!response.is_finished(), "answered while init was still running");
        progress.mark_replacement_adopted();

        let response: Response =
            response.await.expect("handler task").expect("adopted reconfigure response");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body =
            axum::body::to_bytes(response.into_body(), usize::MAX).await.expect("response body");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("response json");
        assert_eq!(body["replacement_adopted"], true);
    }

    // A claimed replacement whose slow init then fails reports the failure
    // instead of an accepted response.
    #[tokio::test(start_paused = true)]
    async fn claimed_reconfigure_reports_slow_init_failure() {
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

        swaps.changed().await.expect("reconfigure sender");
        let progress = swaps.borrow_and_update().clone().expect("reconfigure payload").progress();
        assert!(progress.try_claim(), "runner claims the pending payload");
        tokio::time::sleep(REPLACEMENT_OUTCOME_WAIT + std::time::Duration::from_secs(1)).await;
        progress.report_init_failed("init failed");

        let error = response.await.expect("handler task").expect_err("init failure");
        assert_eq!(error.0, StatusCode::CONFLICT);
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
