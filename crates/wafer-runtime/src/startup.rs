use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use wafer_core::config::NodeDef;
use wafer_core::orchestrator::{LaunchTimings, PipelineOrchestrator};

pub const STARTUP_OUTPUT_ENV: &str = "WAFER_STARTUP_OUTPUT";
pub const CACHE_STATE_ENV: &str = "WAFER_STARTUP_CACHE_STATE";
pub const CACHE_PREPARATION_ENV: &str = "WAFER_STARTUP_CACHE_PREPARATION";
const HARNESS_OVERHEAD_TOLERANCE_NS: u64 = 5_000_000;

/// Process entry, taken as the first statement of `main` before the tokio
/// runtime is built. `process_started` (the start of every E-Perf-9 phase)
/// stays at the top of the async body; the gap between the two is reported
/// separately so the phase definitions do not move. The Unix-epoch stamp lets
/// the harness relate entry to its own pre-exec `runtime_started_ns`.
#[derive(Debug, Clone, Copy)]
pub struct ProcessEntry {
    instant: Instant,
    unix_epoch_ns: u64,
}

impl ProcessEntry {
    pub fn capture() -> Self {
        let instant = Instant::now();
        let unix_epoch_ns = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, duration_ns);
        Self { instant, unix_epoch_ns }
    }
}

pub fn resolve_output_path() -> Option<PathBuf> {
    std::env::var(STARTUP_OUTPUT_ENV).ok().filter(|value| !value.is_empty()).map(PathBuf::from)
}

pub fn write_startup(
    path: &Path,
    orchestrator: &PipelineOrchestrator,
    process_entry: ProcessEntry,
    process_started: Instant,
    launch_started: Instant,
    task_activation_started: Instant,
    launch_timings: LaunchTimings,
) -> Result<()> {
    let (cache_state, preparation) = validated_cache_state()?;

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
        .checked_duration_since(task_activation_started)
        .context("first sink processing preceded pipeline task activation")?;
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
    let component_load_compile_ns = launch_timings.component_load_compile_ns();
    let instantiation_ns = launch_timings.instantiation_ns();
    let pipeline_setup_ns = accounted_pipeline_setup_ns(
        launch_started,
        task_activation_started,
        launch_timings.component_load_compile_ns(),
        launch_timings.instantiation_ns(),
    )?;
    let first_process_ns = duration_ns(first_process);
    let phase_total = process_config_ns
        .saturating_add(component_load_compile_ns)
        .saturating_add(instantiation_ns)
        .saturating_add(pipeline_setup_ns)
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
            "component_load_compile": component_load_compile_ns,
            "instantiation": instantiation_ns,
            "pipeline_setup": pipeline_setup_ns,
            "first_process": first_process_ns,
        },
        "total_wall_duration_ns": total_wall_duration_ns,
        "process_entry": {
            "unix_epoch_ns": process_entry.unix_epoch_ns,
            "to_process_started_ns":
                duration_ns(process_started.saturating_duration_since(process_entry.instant)),
        },
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

fn validated_cache_state() -> Result<(String, String)> {
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
    Ok((cache_state, preparation))
}

fn accounted_pipeline_setup_ns(
    launch_started: Instant,
    task_activation_started: Instant,
    component_load_compile_ns: u64,
    instantiation_ns: u64,
) -> Result<u64> {
    let launch_before_activation =
        task_activation_started
            .checked_duration_since(launch_started)
            .context("pipeline task activation preceded pipeline launch start")?;
    duration_ns(launch_before_activation)
        .checked_sub(component_load_compile_ns)
        .and_then(|remaining| remaining.checked_sub(instantiation_ns))
        .context("component loading and instantiation exceeded pre-activation launch time")
}

fn duration_ns(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e_perf_9_activation_boundary_tiles_non_overlapping_startup_phases() {
        let process_started = Instant::now();
        let launch_started = process_started + Duration::from_millis(3);
        let task_activation_started = launch_started + Duration::from_millis(50);
        let first_processed_at = task_activation_started + Duration::from_millis(7);
        let simulated_caller_return = task_activation_started + Duration::from_millis(10);
        let component_load_compile_ns = 11_000_000;
        let instantiation_ns = 17_000_000;

        assert!(
            first_processed_at.checked_duration_since(simulated_caller_return).is_none(),
            "the old caller-return boundary must fail for this regression fixture"
        );
        let pipeline_setup_ns = accounted_pipeline_setup_ns(
            launch_started,
            task_activation_started,
            component_load_compile_ns,
            instantiation_ns,
        )
        .expect("valid pre-activation phase accounting");
        let first_process_ns = duration_ns(
            first_processed_at
                .checked_duration_since(task_activation_started)
                .expect("processing follows task activation"),
        );
        let phase_total = duration_ns(launch_started.duration_since(process_started))
            + component_load_compile_ns
            + instantiation_ns
            + pipeline_setup_ns
            + first_process_ns;

        assert_eq!(pipeline_setup_ns, 22_000_000);
        assert_eq!(first_process_ns, 7_000_000);
        assert_eq!(
            phase_total,
            duration_ns(first_processed_at.duration_since(process_started)),
            "startup phases must tile process start through first processing without overlap"
        );
    }
}
