#![cfg(test)]
#![expect(
    clippy::print_stderr,
    reason = "test diagnostic output explains when the pre-built Wasm fixture is unavailable"
)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::Value;
use sha2::{Digest, Sha256};

fn repo_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join(relative)
}

#[test]
fn one_message_startup_records_non_overlapping_phases_and_plugin_identity() {
    let config = repo_path("eval/configs/e-perf-9/pipeline-tier-small.toml");
    let plugin =
        repo_path("plugins/pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm");
    if !plugin.exists() {
        eprintln!("skipping — build plugins first with `mise run build-plugins`");
        return;
    }

    let output = tempfile::tempdir().expect("tempdir");
    let startup_path = output.path().join("startup.json");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_wafer"))
        .arg("--config")
        .arg(&config)
        .arg("--no-api")
        .env("WAFER_BENCH_OUTPUT_DIR", output.path())
        .env("WAFER_STARTUP_OUTPUT", &startup_path)
        .env("WAFER_STARTUP_CACHE_STATE", "warm")
        .env("WAFER_STARTUP_CACHE_PREPARATION", "none")
        .spawn()
        .expect("spawn runtime");

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll runtime") {
            break status;
        }
        if started.elapsed() >= Duration::from_secs(30) {
            child.kill().expect("kill timed-out runtime");
            panic!("runtime timed out");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "runtime exited with {status}");

    let startup: Value =
        serde_json::from_slice(&std::fs::read(&startup_path).expect("read startup artifact"))
            .expect("parse startup artifact");
    assert_eq!(startup["clock"], "monotonic");
    assert_eq!(startup["cache_state"], "warm");
    assert_eq!(startup["cache_preparation"]["action"], "none");
    assert_eq!(startup["compiled_component_cache"]["mode"], "disabled");
    assert_eq!(startup["compiled_component_cache"]["hit"], false);
    assert_eq!(startup["processed_messages"], 1);

    let expected_hash = hex::encode(Sha256::digest(std::fs::read(plugin).expect("read plugin")));
    assert_eq!(startup["plugin_sha256"]["t1"], expected_hash);

    let phases = startup["phases_ns"].as_object().expect("phase object");
    let phase_total: u64 = [
        "process_config",
        "component_load_compile",
        "instantiation",
        "pipeline_setup",
        "first_process",
    ]
    .into_iter()
    .map(|phase| {
        let duration = phases[phase].as_u64().expect("phase duration");
        assert!(duration > 0, "{phase} must be measured");
        duration
    })
    .sum();
    let total = startup["total_wall_duration_ns"].as_u64().expect("total duration");
    assert!(phase_total <= total, "phase durations overlap");
    assert!(total - phase_total <= 5_000_000, "unmeasured startup overhead exceeded 5 ms");
}
