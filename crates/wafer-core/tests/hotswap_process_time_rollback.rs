#![cfg(test)]
#![expect(
    clippy::large_futures,
    reason = "test: launch_pipeline future is large due to WASM Store/Component loading"
)]
//! A17 — Process-time hot-swap rollback integration tests.
//!
//! Verifies that when a v2 plugin passes `validate()/init()` but traps on
//! `process()`, the runtime automatically rolls back to v1 within a bounded
//! canary window.
//!
//! Fixture plugins:
//! - `pass-through` (v1): passes all messages unchanged.
//! - `pass-through-v2-panics` (v2): passes validate+init, traps on first `process()`.

use std::path::Path;
use std::time::Duration;

use wafer_core::orchestrator::launch_pipeline;
use wafer_core::runner::{HotSwapError, HotSwapProgress};
use wafer_core::testing::artifact_available;
use wafer_types::config::Config;

/// RAII guard to clean up the `WAFER_BENCH_OUTPUT_DIR` env var on test exit
/// so parallel tests don't leak state to each other.
///
/// L-2 fix (2026-08-02): tests in this file previously used
/// `unsafe std::env::set_var` and `remove_var` unguarded. The Rust 2024
/// edition marks these APIs `unsafe` because they mutate a process-global
/// resource shared across all threads. When the test binary runs tests in
/// parallel (the default for cargo test) two `BenchDirEnv::set` calls
/// could interleave, causing one test to see the other's directory.
///
/// The static mutex below serialises access. `BenchDirEnv::set` acquires
/// the lock inside the guard and holds it for the guard's lifetime, so
/// only one test in this file mutates the env at a time. Any test that
/// panics while holding the guard poisons the lock; subsequent tests
/// recover via `.into_inner()`.
struct BenchDirEnv {
    _lock: std::sync::MutexGuard<'static, ()>,
}

static BENCH_DIR_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl BenchDirEnv {
    fn set(dir: &Path) -> Self {
        let lock = match BENCH_DIR_ENV_LOCK.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        // SAFETY: we hold BENCH_DIR_ENV_LOCK for the entire lifetime of
        // the returned guard, so no other test in this file can concurrently
        // observe or mutate WAFER_BENCH_OUTPUT_DIR. This is the discipline
        // Rust 2024's `unsafe { set_var }` requires.
        unsafe {
            std::env::set_var("WAFER_BENCH_OUTPUT_DIR", dir);
        }
        Self { _lock: lock }
    }
}
impl Drop for BenchDirEnv {
    fn drop(&mut self) {
        // SAFETY: still holding the lock, so no other test can race the
        // removal. See BenchDirEnv::set for the invariant.
        unsafe {
            std::env::remove_var("WAFER_BENCH_OUTPUT_DIR");
        }
    }
}

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
/// `BenchSource` emitting `total_messages` at high rate, `BenchSink` recording.
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

[nodes.source]
type = "source"
kind = "bench-source"
rate = 1000.0
total_messages = {total_messages}
warmup_messages = 0
payload_size = 64

[nodes.transform]
type = "transform"
plugin = {PASS_THROUGH_WASM:?}
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
/// 5. v2 traps on first `process()`.
/// 6. Assert: within 10s, subsequent messages are processed by v1 (pass-through).
/// 7. Assert: rollback metric >= 1.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hotswap_process_time_rollback() {
    if !artifact_available(PASS_THROUGH_WASM) || !artifact_available(PASS_THROUGH_V2_PANICS_WASM) {
        return;
    }

    let tmp = tempfile::tempdir().expect("tmp dir");
    let bench_dir = tmp.path().to_path_buf();
    // BenchSink needs WAFER_BENCH_OUTPUT_DIR for sequence tracking
    let _env_guard = BenchDirEnv::set(&bench_dir);

    let config = build_config(1000);
    let mut orchestrator = launch_pipeline(config, None).await.expect("launch_pipeline");

    let handle = orchestrator.handle();

    // Let some messages flow through v1 first
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Verify v1 is processing (metrics should show > 0 processed)
    let pre_swap_processed = handle.node_metrics("transform").map_or(0, |m| m.processed());
    assert!(pre_swap_processed > 0, "v1 should have processed messages before swap");

    // Prepare hot-swap payload to v2-panics
    let engine = handle.engine();
    let v2_bytes = std::fs::read(PASS_THROUGH_V2_PANICS_WASM).expect("read v2 wasm");
    let (progress, rx) = HotSwapProgress::channel();
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
    handle.send_swap("transform", timed_swap.payload).expect("send_swap");

    // B1 (A17): the API caller must receive `RolledBack`, NOT a fabricated
    // `swap_converged`. Await the completion channel with the 10s canary
    // window budget. Before this fix the runner would drop the sender
    // silently and the next v1 message would call mark_first_v2, so this
    // await would return `Ok(swap_converged)`.
    let outcome = tokio::time::timeout(Duration::from_secs(10), rx)
        .await
        .expect("progress must complete within canary window")
        .expect("progress sender must not be dropped");
    match outcome {
        Err(HotSwapError::RolledBack { rollback_time_ns, ref reason }) => {
            assert!(
                rollback_time_ns > 0,
                "rollback_time_ns must be populated in production path (M1), got 0"
            );
            assert!(
                reason.contains("intentional trap")
                    || reason.contains("panic")
                    || reason.contains("unreachable")
                    || reason.contains("trap"),
                "rollback reason should mention the trap origin, got: {reason}"
            );
        }
        other => panic!("B1: expected RolledBack outcome, got {other:?}"),
    }

    let rollbacks = handle.node_metrics("transform").map_or(0, |m| m.rollbacks());
    assert_eq!(rollbacks, 1, "one trapping replacement rolls back once");

    // After rollback, v1 should continue processing messages
    let post_rollback_processed = handle.node_metrics("transform").map_or(0, |m| m.processed());

    // Wait a bit more for additional messages to flow through v1
    tokio::time::sleep(Duration::from_millis(300)).await;

    let final_processed = handle.node_metrics("transform").map_or(0, |m| m.processed());

    assert!(
        final_processed > post_rollback_processed,
        "After rollback, v1 should continue processing: post_rollback={post_rollback_processed}, final={final_processed}"
    );

    // Verify recovery count increased (the rollback path transitions Error → Recovering → Running)
    let recovery_count = handle.node_metrics("transform").map_or(0, |m| m.recovery_count());
    assert!(
        recovery_count >= 1,
        "Expected at least 1 recovery event from rollback, got {recovery_count}"
    );

    tokio::time::timeout(Duration::from_secs(5), orchestrator.run_until_complete())
        .await
        .expect("pipeline should complete after rollback")
        .expect("pipeline should shut down cleanly");

    let sequence = std::fs::read_to_string(bench_dir.join("sequence.csv"))
        .expect("BenchSink should export sequence.csv");
    let row = sequence.lines().nth(1).expect("sequence.csv data row");
    let columns: Vec<_> = row.split(',').collect();
    assert_eq!(columns[0], columns[1], "rollback must preserve every message: {row}");
    assert_eq!(columns[3], "0", "rollback must replay the trapping message: {row}");
    assert_eq!(columns[4], "0", "rollback must not duplicate messages: {row}");
}

/// Path to the pre-built infinite-loop attack plugin.
const INFINITE_LOOP_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/attacks/infinite-loop/target/wasm32-wasip2/release/wafer_attack_infinite_loop.wasm"
);

/// A replacement that exhausts its epoch budget inside the canary window is
/// rolled back like one that traps.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hotswap_budget_trap_in_canary_window_rolls_back() {
    if !artifact_available(PASS_THROUGH_WASM) || !artifact_available(INFINITE_LOOP_WASM) {
        return;
    }
    let tmp = tempfile::tempdir().expect("tmp dir");
    let _env_guard = BenchDirEnv::set(tmp.path());
    let mut orchestrator = launch_pipeline(build_config(1000), None).await.expect("launch");
    let handle = orchestrator.handle();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let v2_bytes = std::fs::read(INFINITE_LOOP_WASM).expect("read v2 wasm");
    let (progress, rx) = HotSwapProgress::channel();
    let timed_swap = wafer_core::orchestrator::hotswap::prepare_transform_swap_timed(
        handle.engine(),
        &v2_bytes,
        "transform",
        wafer_core::engine::Capabilities::sandbox(),
        64 * 1024 * 1024,
        progress,
    )
    .await
    .expect("prepare v2 swap");
    handle.send_swap("transform", timed_swap.payload).expect("send_swap");

    let outcome = tokio::time::timeout(Duration::from_secs(10), rx)
        .await
        .expect("progress must complete within canary window")
        .expect("progress sender must not be dropped");
    match outcome {
        Err(HotSwapError::RolledBack { ref reason, .. }) => {
            assert!(reason.contains("interrupt"), "rollback reason lost the trap: {reason}");
        }
        other => panic!("expected RolledBack outcome, got {other:?}"),
    }
    assert!(handle.node_metrics("transform").is_some_and(|m| m.rollbacks() > 0));

    tokio::time::timeout(Duration::from_secs(10), orchestrator.run_until_complete())
        .await
        .expect("pipeline should complete after rollback")
        .expect("pipeline should shut down cleanly");
}
