#![cfg(test)]
#![expect(clippy::large_futures, reason = "integration test: large launch_pipeline future")]
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
//! panicking and (b) the recorded median preserves the injected delay.
//!
//! See docs/history/status/implementation-gaps-closed.md §A16.

use std::num::NonZeroU64;
use std::path::Path;
use std::process::Command;

use base64::Engine as _;
use hdrhistogram::Histogram;
use hdrhistogram::serialization::Deserializer;
use hdrhistogram::serialization::interval_log::{IntervalLogIterator, LogEntry};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use wafer_core::orchestrator::launch_pipeline;
use wafer_core::queue::RuntimeEnvelope;
use wafer_core::runner::error_policy::WasmProcessError;
use wafer_core::testing::{PluginTestHarness, artifact_available};
use wafer_types::config::{Config, EngineConfig, FuelBudgets};

const DELAY_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/delay-injector/target/wasm32-wasip2/release/wafer_delay_injector.wasm"
);
const EXPECTED_MIN_MEDIAN_NS: u64 = 45_000_000;
// A sleep never ends early, so only the lower bound proves the delay survived;
// the upper bound only catches a stalled runner, with headroom for loaded CI hosts.
const EXPECTED_MAX_MEDIAN_NS: u64 = 100_000_000;
const UPPERCASE_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/uppercase/target/wasm32-wasip2/release/wafer_uppercase.wasm"
);
const ACTIVE_CALL_CHILD: &str = "WAFER_ACTIVE_CALL_SHUTDOWN_CHILD";

/// Inline TOML for a tiny pipeline. Uses the same node types as
/// `eval/configs/pipeline-c-with-delay.toml` but scaled down so the test
/// finishes in ~3 s wall time.
///
/// Source rate (10 msg/s) sits below sink capacity (1 s / 50 ms = 20 msg/s)
/// so the queue never back-pressures and the recorded median reflects only
/// the injected delay, not queue wait. This is the E-Val-1 methodology
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

/// Read p50 from the latency.hdr the `BenchSink` wrote. Uses `wafer-loadgen
/// hdr-summary` — same tool the shakedown scripts use — so this test
/// exercises the same path we'd exercise on Pi.
fn build_active_call_config() -> Config {
    toml::from_str(&format!(
        r#"
[pipeline]
name = "active-call-shutdown"

[engine]
epoch_deadline = 500

[nodes.source]
type = "source"
kind = "bench-source"
rate = 1000.0
total_messages = 1
warmup_messages = 0
payload_size = 64

[nodes.delay_a]
type = "transform"
plugin = {DELAY_WASM:?}

[nodes.delay_a.config]
delay_ms = 1000

[nodes.delay_b]
type = "transform"
plugin = {DELAY_WASM:?}

[nodes.delay_b.config]
delay_ms = 1000

[nodes.sink_a]
type = "sink"
kind = "stdout"

[nodes.sink_b]
type = "sink"
kind = "stdout"

[[edges]]
from = "source"
to = "delay_a"

[[edges]]
from = "source"
to = "delay_b"

[[edges]]
from = "delay_a"
to = "sink_a"

[[edges]]
from = "delay_b"
to = "sink_b"
"#,
    ))
    .expect("active-call config must parse")
}

/// Median of the single interval `BenchSink` writes to `latency.hdr`.
fn p50_ns_from(bench_dir: &Path) -> u64 {
    let log = std::fs::read(bench_dir.join("latency.hdr")).expect("read latency.hdr");
    let mut deserializer = Deserializer::new();
    let mut histogram: Histogram<u64> = Histogram::new(3).expect("histogram");
    for entry in IntervalLogIterator::new(&log) {
        if let LogEntry::Interval(interval) = entry.expect("well-formed interval log") {
            let encoded = interval.encoded_histogram();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .expect("base64 histogram");
            let decoded: Histogram<u64> =
                deserializer.deserialize(&mut &bytes[..]).expect("V2 histogram");
            histogram.add(&decoded).expect("merge interval");
        }
    }
    assert!(!histogram.is_empty(), "empty histogram: the runner never recorded a sample");
    histogram.value_at_quantile(0.5)
}

#[tokio::test]
async fn repeated_success_and_guest_error_reset_per_call_state() {
    if !artifact_available(UPPERCASE_WASM) {
        return;
    }

    let fuel_limit = NonZeroU64::new(10_000_000).unwrap();
    let config = EngineConfig {
        fuel: FuelBudgets { transform: Some(fuel_limit), ..FuelBudgets::default() },
        ..EngineConfig::default()
    };
    let harness = PluginTestHarness::with_engine_config(&config).expect("engine");
    let mut transform = harness.load_transform(UPPERCASE_WASM).await.expect("uppercase must load");

    let make_input = || {
        let mut input = RuntimeEnvelope::from_string("host-source", "hello");
        input.set_parent_id("host-parent");
        input.ensure_trace_id();
        input.retry_count = 3;
        let header = Arc::make_mut(&mut input.header);
        header.id = "host-id".into();
        header.timestamp = 1_700_000_000_000_000_123;
        header.content_type = "text/plain".into();
        header.metadata = vec![("host-key".into(), "host-value".into())];
        input
    };

    let mut remaining_fuel = Vec::new();
    for _ in 0..3 {
        let input = make_input();
        let trace_id = input.trace_id().expect("trace id").to_string();
        let output = transform.process(input).await.expect("repeated success");
        assert_eq!(&*output.payload, b"HELLO");
        assert_eq!(&*output.header.id, "host-id");
        assert_eq!(output.header.timestamp, 1_700_000_000_000_000_123);
        assert_eq!(&*output.header.source, "host-source");
        assert_eq!(&*output.header.content_type, "text/plain");
        assert_eq!(output.header.metadata, vec![("host-key".into(), "host-value".into())]);
        assert_eq!(output.parent_id(), Some("host-parent"));
        assert_eq!(output.trace_id(), Some(trace_id.as_str()));
        assert_eq!(output.retry_count, 0, "retry budget must not leak downstream");
        assert!(
            transform.node_mut().store_mut().data().table().is_empty(),
            "success must release the borrowed buffer"
        );
        remaining_fuel.push(transform.node_mut().store_mut().get_fuel().expect("fuel enabled"));
    }
    assert_eq!(
        remaining_fuel.get(1),
        remaining_fuel.get(2),
        "steady-state calls must start with a reset fuel budget: {remaining_fuel:?}"
    );

    let error = transform
        .process(RuntimeEnvelope::new("host-source", Bytes::from_static(&[0xff])))
        .await
        .expect_err("invalid UTF-8 must be a guest error");
    assert!(matches!(error, WasmProcessError::BadInput(_)), "unexpected guest error: {error:?}");
    assert!(
        transform.node_mut().store_mut().data().table().is_empty(),
        "guest error must release the borrowed buffer"
    );

    let output =
        transform.process(make_input()).await.expect("Store must remain usable after guest error");
    assert_eq!(&*output.payload, b"HELLO");
    assert!(transform.node_mut().store_mut().data().table().is_empty());
}

#[test]
fn active_wasi_calls_finish_before_small_runtime_shutdown() {
    assert!(Path::new(DELAY_WASM).is_file(), "required delay component missing: {DELAY_WASM}");

    let mut child = Command::new(std::env::current_exe().expect("current test executable"))
        .args([
            "--ignored",
            "--exact",
            "active_wasi_calls_finish_before_small_runtime_shutdown_child",
            "--nocapture",
        ])
        .env(ACTIVE_CALL_CHILD, "1")
        .spawn()
        .expect("spawn bounded active-call child");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait().expect("poll active-call child") {
            assert!(status.success(), "active-call child failed with {status}");
            break;
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill timed-out active-call child");
            child.wait().expect("reap timed-out active-call child");
            panic!("active-call child exceeded the 15 s hard outer timeout");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "executed in a hard-bounded subprocess by active_wasi_calls_finish_before_small_runtime_shutdown"]
async fn active_wasi_calls_finish_before_small_runtime_shutdown_child() {
    assert_eq!(std::env::var(ACTIVE_CALL_CHILD).as_deref(), Ok("1"));
    let mut orchestrator = launch_pipeline(build_active_call_config(), None)
        .await
        .expect("two-delay pipeline must launch");
    let handle = orchestrator.handle();

    tokio::time::timeout(Duration::from_secs(3), async {
        while handle.node_metrics("source").map_or(0, |metrics| metrics.processed()) < 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("source must enqueue the fan-out message");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(handle.node_metrics("delay_a").map_or(0, |metrics| metrics.processed()), 0);
    assert_eq!(handle.node_metrics("delay_b").map_or(0, |metrics| metrics.processed()), 0);

    let cancel_started = Instant::now();
    orchestrator.cancel();
    tokio::time::timeout(Duration::from_secs(5), orchestrator.run_until_complete())
        .await
        .expect("shutdown must stay within the five-second child bound")
        .expect("pipeline must shut down without a runner panic");
    let shutdown_elapsed = cancel_started.elapsed();

    assert!(
        shutdown_elapsed >= Duration::from_millis(700),
        "shutdown returned before active one-second guest calls completed: {shutdown_elapsed:?}"
    );
    assert_eq!(
        handle.node_metrics("delay_a").map_or(0, |metrics| metrics.processed()),
        1,
        "first active call must finish exactly once before cancellation is observed"
    );
    assert_eq!(
        handle.node_metrics("delay_b").map_or(0, |metrics| metrics.processed()),
        1,
        "second active call must finish exactly once before cancellation is observed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delay_injector_runs_without_wasi_runtime_panic() {
    if !artifact_available(DELAY_WASM) {
        return;
    }

    let tmp = tempfile::tempdir().expect("tmp dir");
    let bench_dir = tmp.path().to_path_buf();
    // BenchSink honours this env var; run_until_complete drives the sink to
    // flush latency.hdr into it.
    // SAFETY: single-threaded test setup; no other thread reads env yet.
    unsafe {
        std::env::set_var("WAFER_BENCH_OUTPUT_DIR", &bench_dir);
    }

    let config = build_config(&bench_dir);
    let mut orchestrator = launch_pipeline(config, None).await.expect("launch_pipeline");

    // 30 s wall-time ceiling: 100 × 50 ms sleep = 5 s ideal. Anything over
    // 30 s means something is hung — fail fast.
    let run = orchestrator.run_until_complete();
    let bounded = tokio::time::timeout(Duration::from_secs(30), run).await;

    let result = bounded.expect("pipeline exceeded 30 s wall time");
    result.expect("pipeline must complete without a task panic");

    let p50_ns = p50_ns_from(&bench_dir);
    assert!(
        (EXPECTED_MIN_MEDIAN_NS..=EXPECTED_MAX_MEDIAN_NS).contains(&p50_ns),
        "delay-injector median = {p50_ns} ns, expected [45, 100] ms"
    );
}
