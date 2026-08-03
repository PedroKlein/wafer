#![cfg(test)]
//! P0.14 regression: WASI async host calls (`std::thread::sleep` in guest,
//! `wasi:clocks/monotonic-clock.subscribe-duration` on the wire) must NOT
//! panic when invoked from a Tokio worker thread.
//!
//! Before the fix, `add_to_linker_sync`'s internal `Handle::current().block_on`
//! panicked with "Cannot start a runtime from within a runtime" on the first
//! guest call. The transform-runner task died; `BenchSink` recorded zero
//! samples; every downstream methodology claim would have been unverifiable.
//!
//! This test drives the real runner (not the harness) end-to-end with the
//! delay-injector plugin. Passes iff (a) the pipeline completes without
//! panicking and (b) recorded p99 lands in the E-Val-1 honesty window.
//!
//! See docs/status/implementation-gaps.md §A16.

use std::path::{Path, PathBuf};
use std::time::Duration;

use wafer_core::orchestrator::launch_pipeline;
use wafer_types::config::Config;

/// Delay-injector artefact path. Missing artefact skips the test (matches
/// the pattern in `tests/attack_containment.rs`).
const DELAY_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/delay-injector/target/wasm32-wasip2/release/wafer_delay_injector.wasm"
);

/// Inline TOML for a tiny pipeline. Uses the same node types as
/// `eval/configs/pipeline-c-with-delay.toml` but scaled down so the test
/// finishes in ~3 s wall time.
///
/// Source rate (10 msg/s) sits below sink capacity (1 s / 50 ms = 20 msg/s)
/// so the queue never back-pressures and recorded p99 reflects only the
/// injected delay, not queue wait. This is the E-Val-1 methodology
/// invariant: measurement rig must not exaggerate latency via queueing.
fn build_config(bench_dir: &Path) -> Config {
    let toml = format!(
        r#"
[pipeline]
name = "p0-14-regression"

[engine]
epoch_deadline = 500

[nodes.source]
type = "source"
kind = "bench-source"
rate = 10.0
total_messages = 30
warmup_messages = 5
payload_size = 64

[nodes.delay]
type = "transform"
plugin = {DELAY_WASM:?}

[nodes.delay.config]
delay_ms = 50

[nodes.sink]
type = "sink"
kind = "bench-sink"
warmup_secs = 1
track_sequences = true
track_hotswap = false

[[edges]]
from = "source"
to = "delay"

[[edges]]
from = "delay"
to = "sink"
"#,
    );
    let _ = bench_dir; // BenchSink reads WAFER_BENCH_OUTPUT_DIR env var
    toml::from_str(&toml).expect("inline config must parse")
}

/// Read p99 from the latency.hdr the `BenchSink` wrote. Uses `wafer-loadgen
/// hdr-summary` — same tool the shakedown scripts use — so this test
/// exercises the same path we'd exercise on Pi.
fn p99_ms_from(bench_dir: &Path) -> f64 {
    let hdr = bench_dir.join("latency.hdr");
    assert!(hdr.exists(), "latency.hdr missing at {hdr:?}");

    let loadgen = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/release/wafer-loadgen");
    assert!(
        loadgen.exists(),
        "wafer-loadgen binary missing at {loadgen:?} — run `cargo build --release -p wafer-loadgen`"
    );

    let out = std::process::Command::new(&loadgen)
        .arg("hdr-summary")
        .arg("--hdr")
        .arg(&hdr)
        .output()
        .expect("hdr-summary invocation");
    assert!(out.status.success(), "hdr-summary failed: {out:?}");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let count = json["total_count"].as_u64().unwrap_or(0);
    assert!(count > 0, "empty histogram — runner never recorded a sample: {json}");
    json["p99_ns"].as_u64().unwrap() as f64 / 1_000_000.0
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delay_injector_runs_without_wasi_runtime_panic() {
    if !Path::new(DELAY_WASM).exists() {
        eprintln!("SKIP: delay-injector.wasm not built at {DELAY_WASM}");
        return;
    }

    let tmp = tempfile::tempdir().expect("tmp dir");
    let bench_dir = tmp.path().to_path_buf();
    // BenchSink honours this env var; run_until_complete drives the sink to
    // flush latency.hdr into it.
    // SAFETY: single-threaded test setup; no other thread reads env yet.
    unsafe { std::env::set_var("WAFER_BENCH_OUTPUT_DIR", &bench_dir); }

    let config = build_config(&bench_dir);
    let mut orchestrator = launch_pipeline(config, None)
        .await
        .expect("launch_pipeline");

    // 30 s wall-time ceiling: 100 × 50 ms sleep = 5 s ideal. Anything over
    // 30 s means something is hung — fail fast.
    let run = orchestrator.run_until_complete();
    let bounded = tokio::time::timeout(Duration::from_secs(30), run).await;

    let result = bounded.expect("pipeline exceeded 30 s wall time");
    result.expect("pipeline must complete without a task panic");

    let p99 = p99_ms_from(&bench_dir);
    assert!(
        (45.0..=55.0).contains(&p99),
        "E-Val-1 honesty window violated: p99 = {p99} ms, expected [45, 55]"
    );
}
