#![cfg(test)]
#![expect(clippy::print_stderr, reason = "integration test diagnostic output")]
#![expect(
    clippy::large_futures,
    reason = "test: launch_pipeline future is large due to WASM Store/Component loading"
)]
//! Hot-swap adoption with and without traffic.
//!
//! - Idle: the source emits one message and then stays quiet for 2.5 s. A swap
//!   sent in that gap must be adopted within milliseconds, and no message may
//!   be processed between the signal and the adoption.
//! - Under 1 000 msg/s: swapping `pass-through-v1` for `pass-through-v2` loses
//!   and duplicates nothing, and `plugin.version` flips exactly once.

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use wafer_core::orchestrator::launch_pipeline;
use wafer_core::runner::HotSwapProgress;
use wafer_types::config::Config;

const PASS_THROUGH_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm"
);

const PASS_THROUGH_V1_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/pass-through-v1/target/wasm32-wasip2/release/wafer_pass_through_v1.wasm"
);

const PASS_THROUGH_V2_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/pass-through-v2/target/wasm32-wasip2/release/wafer_pass_through_v2.wasm"
);

/// Serialises `WAFER_BENCH_OUTPUT_DIR` writes; see
/// `hotswap_process_time_rollback.rs` for why the env mutation needs a lock.
static BENCH_DIR_ENV_LOCK: Mutex<()> = Mutex::new(());

struct BenchDirEnv {
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl BenchDirEnv {
    fn set(dir: &Path) -> Self {
        let lock = BENCH_DIR_ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // SAFETY: the lock is held for the guard's lifetime, so no other test
        // in this binary reads or writes the variable concurrently.
        unsafe {
            std::env::set_var("WAFER_BENCH_OUTPUT_DIR", dir);
        }
        Self { _lock: lock }
    }
}

impl Drop for BenchDirEnv {
    fn drop(&mut self) {
        // SAFETY: still holding the lock; see `BenchDirEnv::set`.
        unsafe {
            std::env::remove_var("WAFER_BENCH_OUTPUT_DIR");
        }
    }
}

fn idle_config() -> Config {
    let toml = format!(
        r#"
[pipeline]
name = "idle-swap"

[nodes.source]
type = "source"
kind = "bench-source"
rate = 0.4
total_messages = 2
warmup_messages = 0
payload_size = 16

[nodes.transform]
type = "transform"
plugin = {PASS_THROUGH_WASM:?}

[nodes.sink]
type = "sink"
kind = "stdout"

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_transform_adopts_swap_without_input() {
    if !Path::new(PASS_THROUGH_WASM).exists() {
        eprintln!("SKIP: pass-through.wasm not built at {PASS_THROUGH_WASM}");
        return;
    }

    let mut orchestrator = launch_pipeline(idle_config(), None).await.expect("launch_pipeline");
    let handle = orchestrator.handle();

    // The first message goes through v1; the next one is 2.5 s away.
    let processed = || handle.node_metrics("transform").map_or(0, |m| m.processed());
    let deadline = Instant::now() + Duration::from_secs(10);
    while processed() == 0 {
        assert!(Instant::now() < deadline, "first message never reached the transform");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let before_signal = processed();

    let v2_bytes = std::fs::read(PASS_THROUGH_WASM).expect("read v2 wasm");
    let (progress, _completion) = HotSwapProgress::channel();
    let prepared = wafer_core::orchestrator::hotswap::prepare_transform_swap_timed(
        handle.engine(),
        &v2_bytes,
        "transform",
        wafer_core::engine::Capabilities::sandbox(),
        64 * 1024 * 1024,
        progress.clone(),
    )
    .await
    .expect("prepare v2 swap");

    let signal_at = Instant::now();
    handle.send_swap("transform", prepared.payload).expect("send_swap");

    let adopted_at = loop {
        if let Some(at) = progress.replacement_adopted_at() {
            break at;
        }
        assert!(
            signal_at.elapsed() < Duration::from_secs(1),
            "idle node did not adopt the swap without new input"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    };
    let adoption = adopted_at.duration_since(signal_at);
    eprintln!("idle adoption took {adoption:?}");
    assert!(adoption < Duration::from_millis(500), "adoption took {adoption:?}");
    assert_eq!(processed(), before_signal, "a message was processed between signal and adoption");
    assert!(!progress.try_withdraw(), "an adopted swap must not be withdrawable");

    orchestrator.shutdown().await.expect("pipeline shuts down");
}

fn traffic_config() -> Config {
    let toml = format!(
        r#"
[pipeline]
name = "traffic-swap"

[nodes.source]
type = "source"
kind = "bench-source"
rate = 1000.0
total_messages = 1000
warmup_messages = 0
payload_size = 64

[nodes.transform]
type = "transform"
plugin = {PASS_THROUGH_V1_WASM:?}

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn swap_under_traffic_flips_version_once_without_loss() {
    for plugin in [PASS_THROUGH_V1_WASM, PASS_THROUGH_V2_WASM] {
        if !Path::new(plugin).exists() {
            eprintln!("SKIP: {plugin} not built");
            return;
        }
    }

    let tmp = tempfile::tempdir().expect("tmp dir");
    let bench_dir = tmp.path().to_path_buf();
    let _env_guard = BenchDirEnv::set(&bench_dir);

    let mut orchestrator = launch_pipeline(traffic_config(), None).await.expect("launch_pipeline");
    let handle = orchestrator.handle();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        handle.node_metrics("transform").is_some_and(|m| m.processed() > 0),
        "v1 should process messages before the swap"
    );

    let v2_bytes = std::fs::read(PASS_THROUGH_V2_WASM).expect("read v2 wasm");
    let (progress, completion) = HotSwapProgress::channel();
    let prepared = wafer_core::orchestrator::hotswap::prepare_transform_swap_timed(
        handle.engine(),
        &v2_bytes,
        "transform",
        wafer_core::engine::Capabilities::sandbox(),
        64 * 1024 * 1024,
        progress,
    )
    .await
    .expect("prepare v2 swap");
    handle.send_swap("transform", prepared.payload).expect("send_swap");

    tokio::time::timeout(Duration::from_secs(5), completion)
        .await
        .expect("swap completes under traffic")
        .expect("progress sender")
        .expect("swap succeeds");

    tokio::time::timeout(Duration::from_secs(10), orchestrator.run_until_complete())
        .await
        .expect("pipeline completes")
        .expect("pipeline shuts down cleanly");

    let sequence = std::fs::read_to_string(bench_dir.join("sequence.csv")).expect("sequence.csv");
    let row = sequence.lines().nth(1).expect("sequence.csv data row");
    let columns: Vec<_> = row.split(',').collect();
    assert_eq!(columns[0], columns[1], "swap must not lose messages: {row}");
    assert_eq!(columns[3], "0", "swap must not leave gaps: {row}");
    assert_eq!(columns[4], "0", "swap must not duplicate messages: {row}");

    let timeline =
        std::fs::read_to_string(bench_dir.join("swap_timeline.json")).expect("swap_timeline.json");
    let timeline: serde_json::Value = serde_json::from_str(&timeline).expect("timeline json");
    let transitions = timeline["transitions"].as_array().expect("transitions array");
    assert_eq!(transitions.len(), 1, "version must flip exactly once: {timeline}");
    assert_eq!(transitions[0]["from"], "1.0.0");
    assert_eq!(transitions[0]["to"], "2.0.0");
}
