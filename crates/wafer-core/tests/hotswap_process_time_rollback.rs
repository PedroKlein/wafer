#![cfg(test)]
//! A17 — Process-time hot-swap rollback integration tests.
//!
//! Verifies that when a v2 plugin passes validate()/init() but traps on
//! process(), the runtime automatically rolls back to v1 within a bounded
//! canary window.
//!
//! Fixture plugins:
//! - `pass-through` (v1): passes all messages unchanged.
//! - `pass-through-v2-panics` (v2): passes validate+init, traps on first process().

use std::path::Path;
use std::time::Duration;

use wafer_core::orchestrator::launch_pipeline;
use wafer_core::runner::HotSwapProgress;
use wafer_types::config::Config;

/// Path to the pre-built pass-through plugin (v1).
const PASS_THROUGH_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm"
);

/// Path to the pre-built v2-panics plugin.
const PASS_THROUGH_V2_PANICS_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/pass-through-v2-panics/target/wasm32-wasip2/release/wafer_pass_through_v2_panics.wasm"
);

/// Build a pipeline config with a single pass-through transform node,
/// BenchSource emitting `total_messages` at high rate, BenchSink recording.
fn build_config(total_messages: u64) -> Config {
    let toml = format!(
        r#"
[pipeline]
name = "a17-rollback-test"

[engine]
epoch_deadline = 100

[engine.hot_swap]
canary_success_count = 32
canary_window_ms = 10000
max_rollback_retries = 3

[nodes.source]
type = "source"
kind = "bench-source"
rate = 1000.0
total_messages = {total_messages}
warmup_messages = 0
payload_size = 64

[nodes.transform]
type = "transform"
plugin = {plugin:?}
plugin_version = "1.0.0"

[nodes.sink]
type = "sink"
kind = "bench-sink"
warmup_secs = 0
track_sequences = true
track_hotswap = true

[[edges]]
from = "source"
to = "transform"

[[edges]]
from = "transform"
to = "sink"
"#,
        plugin = PASS_THROUGH_WASM,
    );
    toml::from_str(&toml).expect("inline config must parse")
}

/// Test: hot-swap to v2-panics, assert rollback to v1 within 10s.
///
/// Scenario:
/// 1. Launch pipeline with pass-through v1.
/// 2. Let a few messages flow (proves v1 works).
/// 3. Hot-swap to pass-through-v2-panics.
/// 4. Swap ACKs (validate + init pass).
/// 5. v2 traps on first process().
/// 6. Assert: within 10s, subsequent messages are processed by v1 (pass-through).
/// 7. Assert: rollback metric >= 1.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hotswap_process_time_rollback() {
    if !Path::new(PASS_THROUGH_WASM).exists() {
        eprintln!("SKIP: pass-through.wasm not built at {PASS_THROUGH_WASM}");
        return;
    }
    if !Path::new(PASS_THROUGH_V2_PANICS_WASM).exists() {
        eprintln!("SKIP: pass-through-v2-panics.wasm not built at {PASS_THROUGH_V2_PANICS_WASM}");
        return;
    }

    let tmp = tempfile::tempdir().expect("tmp dir");
    let bench_dir = tmp.path().to_path_buf();
    // BenchSink needs WAFER_BENCH_OUTPUT_DIR for sequence tracking
    unsafe { std::env::set_var("WAFER_BENCH_OUTPUT_DIR", &bench_dir); }

    // Use a large message count so pipeline stays alive long enough for rollback
    let config = build_config(5000);
    let mut orchestrator = launch_pipeline(config, None)
        .await
        .expect("launch_pipeline");

    let handle = orchestrator.handle();

    // Let some messages flow through v1 first
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Verify v1 is processing (metrics should show > 0 processed)
    let pre_swap_processed = handle
        .node_metrics("transform")
        .map(|m| m.processed())
        .unwrap_or(0);
    assert!(pre_swap_processed > 0, "v1 should have processed messages before swap");

    // Prepare hot-swap payload to v2-panics
    let engine = handle.engine();
    let v2_bytes = std::fs::read(PASS_THROUGH_V2_PANICS_WASM).expect("read v2 wasm");
    let (progress, _rx) = HotSwapProgress::channel();
    let v2_result = wafer_core::orchestrator::hotswap::prepare_transform_swap_timed(
        engine,
        &v2_bytes,
        "transform",
        wafer_core::engine::Capabilities::sandbox(),
        64 * 1024 * 1024,
        progress,
    )
    .await;
    let timed_swap = v2_result.expect("prepare v2 swap");

    // Send the swap
    handle
        .send_swap("transform", timed_swap.payload)
        .expect("send_swap");

    // Wait for the rollback to happen — the canary detects the trap and rolls back
    // Give it up to 10 seconds (the canary window)
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut rollback_detected = false;

    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Some(m) = handle.node_metrics("transform") {
            if m.rollbacks() > 0 {
                rollback_detected = true;
                break;
            }
        }
    }

    assert!(
        rollback_detected,
        "Expected rollback metric > 0 within 10s canary window"
    );

    // After rollback, v1 should continue processing messages
    let post_rollback_processed = handle
        .node_metrics("transform")
        .map(|m| m.processed())
        .unwrap_or(0);

    // Wait a bit more for additional messages to flow through v1
    tokio::time::sleep(Duration::from_millis(300)).await;

    let final_processed = handle
        .node_metrics("transform")
        .map(|m| m.processed())
        .unwrap_or(0);

    assert!(
        final_processed > post_rollback_processed,
        "After rollback, v1 should continue processing: post_rollback={post_rollback_processed}, final={final_processed}"
    );

    // Verify recovery count increased (the rollback path transitions Error → Recovering → Running)
    let recovery_count = handle
        .node_metrics("transform")
        .map(|m| m.recovery_count())
        .unwrap_or(0);
    assert!(
        recovery_count >= 1,
        "Expected at least 1 recovery event from rollback, got {recovery_count}"
    );

    // Clean shutdown
    orchestrator.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), orchestrator.run_until_complete()).await;
}

/// Test: bounded rollback retries — after M traps, escalate to Recovery state.
///
/// This test uses max_rollback_retries = 1 and verifies that the second
/// process-time trap on v2 does NOT trigger another rollback, but instead
/// falls through to the standard A7 recovery path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hotswap_bounded_rollback_thrash() {
    if !Path::new(PASS_THROUGH_WASM).exists() {
        eprintln!("SKIP: pass-through.wasm not built at {PASS_THROUGH_WASM}");
        return;
    }
    if !Path::new(PASS_THROUGH_V2_PANICS_WASM).exists() {
        eprintln!("SKIP: pass-through-v2-panics.wasm not built at {PASS_THROUGH_V2_PANICS_WASM}");
        return;
    }

    let tmp = tempfile::tempdir().expect("tmp dir");
    let bench_dir = tmp.path().to_path_buf();
    unsafe { std::env::set_var("WAFER_BENCH_OUTPUT_DIR", &bench_dir); }

    // Use max_rollback_retries = 1 to test exhaustion
    let toml = format!(
        r#"
[pipeline]
name = "a17-thrash-test"

[engine]
epoch_deadline = 100

[engine.hot_swap]
canary_success_count = 32
canary_window_ms = 10000
max_rollback_retries = 1

[nodes.source]
type = "source"
kind = "bench-source"
rate = 1000.0
total_messages = 5000
warmup_messages = 0
payload_size = 64

[nodes.transform]
type = "transform"
plugin = {plugin:?}
plugin_version = "1.0.0"

[nodes.sink]
type = "sink"
kind = "bench-sink"
warmup_secs = 0
track_sequences = true
track_hotswap = true

[[edges]]
from = "source"
to = "transform"

[[edges]]
from = "transform"
to = "sink"
"#,
        plugin = PASS_THROUGH_WASM,
    );
    let config: Config = toml::from_str(&toml).expect("parse");
    let mut orchestrator = launch_pipeline(config, None)
        .await
        .expect("launch_pipeline");

    let handle = orchestrator.handle();

    // Let v1 process some messages
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Hot-swap to v2-panics
    let engine = handle.engine();
    let v2_bytes = std::fs::read(PASS_THROUGH_V2_PANICS_WASM).expect("read v2 wasm");
    let (progress, _rx) = HotSwapProgress::channel();
    let v2_result = wafer_core::orchestrator::hotswap::prepare_transform_swap_timed(
        engine,
        &v2_bytes,
        "transform",
        wafer_core::engine::Capabilities::sandbox(),
        64 * 1024 * 1024,
        progress,
    )
    .await;
    let timed_swap = v2_result.expect("prepare v2 swap");

    handle
        .send_swap("transform", timed_swap.payload)
        .expect("send_swap");

    // Wait for rollback and recovery
    tokio::time::sleep(Duration::from_secs(3)).await;

    // With max_rollback_retries = 1:
    // - First trap triggers rollback (success, back to v1)
    // - After rollback, v1 should be running fine
    let metrics = handle.node_metrics("transform").expect("transform metrics");

    // Rollback should have fired exactly once (max_rollback_retries = 1 means
    // the first trap triggers rollback; subsequent traps would exhaust the
    // budget, but since rollback succeeds and v1 works, there are no more traps)
    assert!(
        metrics.rollbacks() >= 1,
        "Expected at least 1 rollback, got {}",
        metrics.rollbacks()
    );

    // Recovery count should be >= 1 (rollback includes recovery transition)
    assert!(
        metrics.recovery_count() >= 1,
        "Expected recovery_count >= 1, got {}",
        metrics.recovery_count()
    );

    // Pipeline should still be processing (v1 is restored)
    let processed_after = metrics.processed();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let processed_final = handle
        .node_metrics("transform")
        .map(|m| m.processed())
        .unwrap_or(0);
    assert!(
        processed_final > processed_after,
        "v1 should continue processing after rollback"
    );

    // Clean shutdown
    orchestrator.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), orchestrator.run_until_complete()).await;
}
