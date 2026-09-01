use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use wafer_core::config::NodeDef;
use wafer_core::orchestrator::{LaunchTimings, PipelineOrchestrator};

pub const STARTUP_OUTPUT_ENV: &str = "WAFER_STARTUP_OUTPUT";
pub const CACHE_STATE_ENV: &str = "WAFER_STARTUP_CACHE_STATE";
pub const CACHE_PREPARATION_ENV: &str = "WAFER_STARTUP_CACHE_PREPARATION";
const HARNESS_OVERHEAD_TOLERANCE_NS: u64 = 5_000_000;

pub fn resolve_output_path() -> Option<PathBuf> {
    std::env::var(STARTUP_OUTPUT_ENV).ok().filter(|value| !value.is_empty()).map(PathBuf::from)
}

pub fn write_startup(
    path: &Path,
    orchestrator: &PipelineOrchestrator,
    process_started: Instant,
    launch_started: Instant,
    launch_completed: Instant,
    launch_timings: LaunchTimings,
) -> Result<()> {
    let cache_state = std::env::var(CACHE_STATE_ENV).with_context(|| {
        format!("{CACHE_STATE_ENV} is required when {STARTUP_OUTPUT_ENV} is set")
    })?;
    let preparation = std::env::var(CACHE_PREPARATION_ENV).with_context(|| {
        format!("{CACHE_PREPARATION_ENV} is required when {STARTUP_OUTPUT_ENV} is set")
    })?;
    if !matches!(cache_state.as_str(), "cold" | "warm") {
        bail!("invalid startup cache state: {cache_state}");
    }
    let expected_preparation = if cache_state == "cold" { "drop-linux-page-cache" } else { "none" };
    if preparation != expected_preparation {
        bail!("startup cache preparation {preparation:?} does not match {cache_state:?}");
    }

    let sink_metrics = orchestrator
        .config()
        .nodes
        .iter()
        .filter(|(_, node)| matches!(node, NodeDef::Sink(_)))
        .filter_map(|(node_id, _)| orchestrator.node_metrics(node_id));
    let (processed_messages, first_processed_at) =
        sink_metrics.fold((0_u64, None::<Instant>), |(count, earliest), metrics| {
            let first = match (earliest, metrics.first_processed_at()) {
                (Some(current), Some(candidate)) => Some(current.min(candidate)),
                (None, candidate) => candidate,
                (current, None) => current,
            };
            (count.saturating_add(metrics.processed()), first)
        });
    let first_processed_at = first_processed_at.context("no sink processed a message")?;
    if processed_messages != 1 {
        bail!("startup probe processed {processed_messages} sink messages; expected exactly one");
    }
    let first_process = first_processed_at
        .checked_duration_since(launch_completed)
        .context("first sink processing preceded pipeline launch completion")?;
    let total_wall = first_processed_at
        .checked_duration_since(process_started)
        .context("first sink processing preceded process start")?;

    let plugin_sha256: Map<String, Value> = orchestrator
        .handle()
        .plugin_hashes_snapshot()
        .into_iter()
        .map(|(node, hash)| (node, Value::String(hash)))
        .collect();
    if plugin_sha256.is_empty() {
        bail!("startup probe did not load a Wasm plugin");
    }

    let process_config_ns = duration_ns(launch_started.duration_since(process_started));
    let first_process_ns = duration_ns(first_process);
    let phase_total = process_config_ns
        .saturating_add(launch_timings.component_load_compile_ns())
        .saturating_add(launch_timings.instantiation_ns())
        .saturating_add(launch_timings.pipeline_setup_ns())
        .saturating_add(first_process_ns);
    let total_wall_duration_ns = duration_ns(total_wall);
    if phase_total > total_wall_duration_ns {
        bail!("startup phase durations overlap");
    }
    if total_wall_duration_ns.saturating_sub(phase_total) > HARNESS_OVERHEAD_TOLERANCE_NS {
        bail!("startup harness overhead exceeded 5 ms");
    }

    let payload = json!({
        "schema_version": 1,
        "clock": "monotonic",
        "cache_state": cache_state,
        "cache_preparation": {
            "action": preparation,
            "completed_before_timing": true,
        },
        "compiled_component_cache": {
            "mode": "disabled",
            "hit": false,
            "artifact": Value::Null,
            "identity": Value::Null,
        },
        "plugin_sha256": plugin_sha256,
        "processed_messages": processed_messages,
        "phases_ns": {
            "process_config": process_config_ns,
            "component_load_compile": launch_timings.component_load_compile_ns(),
            "instantiation": launch_timings.instantiation_ns(),
            "pipeline_setup": launch_timings.pipeline_setup_ns(),
            "first_process": first_process_ns,
        },
        "total_wall_duration_ns": total_wall_duration_ns,
        "harness_overhead_tolerance_ns": HARNESS_OVERHEAD_TOLERANCE_NS,
    });

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create startup output directory {}", parent.display()))?;
    }
    std::fs::write(path, serde_json::to_string_pretty(&payload)? + "\n")
        .with_context(|| format!("write startup timings to {}", path.display()))
}

fn duration_ns(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}
