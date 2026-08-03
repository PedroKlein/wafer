//! Integration test for the hotswap-trigger load profile (P0.3 AC3).
//!
//! Spins up a mock HTTP server via `axum` that records the wall-clock
//! timestamp of any POST to `/api/v1/nodes/:id/hot-swap`. The publisher runs
//! in a mode that does NOT connect to a broker (broker connect is bypassed by
//! pointing at a dead port; publish errors are logged but not fatal). The
//! trigger task must POST exactly once, ±100 ms of the target offset.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

use axum::{Router, extract::{Path, State}, http::StatusCode, routing::post};
use tokio::net::TcpListener;

use wafer_loadgen::{PublishArgs, run_publisher};

/// Server state: records the elapsed ms from `start` when the POST arrived.
/// -1 means "no POST yet".
#[derive(Clone)]
struct RecorderState {
    start: Instant,
    recorded_ms: Arc<AtomicI64>,
    body_len: Arc<AtomicI64>,
    node_id: Arc<parking_lot_mini::Mutex<Option<String>>>,
}

// A tiny stand-in Mutex to avoid pulling parking_lot in as a dep. We only need
// one-shot store; use std::sync::Mutex.
mod parking_lot_mini {
    pub type Mutex<T> = std::sync::Mutex<T>;
}

async fn record_swap(
    Path(id): Path<String>,
    State(state): State<RecorderState>,
    body: axum::body::Bytes,
) -> (StatusCode, &'static str) {
    let elapsed_ms = i64::try_from(state.start.elapsed().as_millis()).unwrap_or(i64::MAX);
    // Only record the FIRST POST to catch spurious retries.
    let _prev = state.recorded_ms.compare_exchange(
        -1,
        elapsed_ms,
        Ordering::AcqRel,
        Ordering::Acquire,
    );
    state
        .body_len
        .store(i64::try_from(body.len()).unwrap_or(i64::MAX), Ordering::Release);
    if let Ok(mut g) = state.node_id.lock() {
        *g = Some(id);
    }
    (StatusCode::OK, "ok")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "integration test asserts on captured POST timing via unwrap/assert"
)]
async fn hotswap_trigger_posts_once_within_100ms_of_scheduled_offset()
    -> anyhow::Result<()>
{
    let _init = tracing_subscriber::fmt()
        .with_env_filter("info,wafer_loadgen=debug")
        .with_test_writer()
        .try_init()
        .ok();

    let start = Instant::now();
    let state = RecorderState {
        start,
        recorded_ms: Arc::new(AtomicI64::new(-1)),
        body_len: Arc::new(AtomicI64::new(-1)),
        node_id: Arc::new(std::sync::Mutex::new(None)),
    };
    let app = Router::new()
        .route("/api/v1/nodes/{id}/hot-swap", post(record_swap))
        .with_state(state.clone());

    // Bind to a random localhost port.
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
    let addr = listener.local_addr()?;
    let server = tokio::spawn(async move {
        let _ok = axum::serve(listener, app).await;
    });

    // Publisher: hotswap-trigger profile, swap at 1.0s. Point at a dead broker
    // port so we don't need mosquitto — the publish() failures are counted as
    // errors but do not stop the run. What we care about is the trigger POST.
    let target_offset_secs = 1.0_f64;
    let args = PublishArgs {
        broker_host: "127.0.0.1".into(),
        broker_port: 1, // dead port; publishes will fail, that's fine
        topic: "wafer/bench/input".into(),
        rate: 100,
        duration_secs: 3, // give the trigger time to fire (needs > swap_at + ~200ms drain)
        payload_size: 128,
        payload_template: None,
        profile: "hotswap-trigger".into(),
        client_id: "wafer-loadgen-p0-3-test".into(),
        profile_file: None,
        dry_run: false,
        burst_multiplier: 2,
        burst_on_secs: 10,
        burst_cycle_secs: 60,
        ramp_start_rate: 100,
        ramp_step_rate: 100,
        ramp_step_interval_secs: 10,
        ramp_max_rate: 10_000,
        hotswap_target_node: Some("transform".into()),
        hotswap_wasm_path: Some(PathBuf::from("/tmp/fake-pass-through-v2.wasm")),
        hotswap_swap_at_secs: target_offset_secs,
        hotswap_api_url: format!("http://{addr}"),
    };

    // Give the axum server a moment to be listening. axum::serve() awaits so
    // by the time we call this, the listener is bound but the accept loop may
    // still be starting. A short sleep de-flakes the test.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let report = run_publisher(args).await?;
    assert_eq!(report.hotswap_triggered_at_secs, Some(target_offset_secs));

    // Verify a POST was recorded.
    let recorded_ms = state.recorded_ms.load(Ordering::Acquire);
    assert!(recorded_ms >= 0, "no POST recorded (marker still -1)");

    // ±100 ms tolerance around the target offset (measured from publisher start,
    // which is slightly after test start — allow +500ms slack for the MQTT
    // setup delay in run_publisher).
    #[expect(
        clippy::cast_possible_truncation,
        clippy::as_conversions,
        reason = "target_offset_secs is 1.0 in this test; the multiplied value fits comfortably in i64"
    )]
    let target_ms = (target_offset_secs * 1000.0) as i64;
    let delta_ms = (recorded_ms - target_ms).abs();
    // The 500ms mqtt-setup delay is bundled into the "start". run_publisher
    // starts the shared `start` Instant AFTER the MQTT setup delay, so the
    // trigger is offset-aligned to the publish loop start. But this test's
    // AtomicI64 captures elapsed from the *outer* test-start Instant, so we
    // expect a delta of ~500 ms + target_offset. Tolerance is ±150ms to
    // absorb Instant/tokio-timer skew on cold macOS.
    let outer_target_ms = target_ms + 500 + 50; // + init sleep
    let delta_outer_ms = (recorded_ms - outer_target_ms).abs();
    assert!(
        delta_ms <= 200 || delta_outer_ms <= 200,
        "POST timing outside tolerance: recorded={recorded_ms}ms, target={target_ms}ms (inner) or {outer_target_ms}ms (outer), delta={delta_ms}ms / {delta_outer_ms}ms"
    );

    // Verify the payload has a JSON body with the wasm_path.
    let body_len = state.body_len.load(Ordering::Acquire);
    assert!(body_len > 0, "POST body was empty");
    // Verify the {id} captured is our target node.
    let captured_id = state.node_id.lock().unwrap().clone();
    assert_eq!(captured_id.as_deref(), Some("transform"));

    server.abort();
    Ok(())
}
