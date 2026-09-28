//! The publisher waits for the broker before its clock starts, counts what
//! the broker acknowledged, and drains before it disconnects.

mod common;

use std::path::PathBuf;
use std::time::Duration;

use common::FakeBroker;
use wafer_loadgen::{PublishArgs, run_publisher};

fn args(broker: &FakeBroker, rate: u32, duration_secs: u64) -> PublishArgs {
    PublishArgs {
        broker_host: broker.host(),
        broker_port: broker.port(),
        topic: "wafer/test/input".into(),
        rate,
        duration_secs,
        payload_size: 128,
        payload_template: None,
        profile: "steady".into(),
        client_id: format!("publisher-test-{}", std::process::id()),
        profile_file: None,
        dry_run: false,
        burst_multiplier: 2,
        burst_on_secs: 10,
        burst_cycle_secs: 60,
        ramp_start_rate: 100,
        ramp_step_rate: 100,
        ramp_step_interval_secs: 10,
        ramp_max_rate: 10_000,
        hotswap_target_node: None,
        hotswap_wasm_path: None,
        hotswap_swap_at_secs: 30.0,
        hotswap_api_url: "http://localhost:9090".into(),
        hotswap_result_path: None,
        timing_receipt: None,
        trace_file: None,
        summary_file: None,
        sequence_start: 0,
        drop_when_full: false,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_enqueued_message_is_acknowledged_before_the_publisher_exits() {
    let broker = FakeBroker::start().await;
    let dir = tempfile::tempdir().unwrap();
    let summary_path = dir.path().join("publisher-summary.json");
    let mut args = args(&broker, 200, 1);
    args.summary_file = Some(summary_path.clone());

    let report = run_publisher(args).await.unwrap();

    assert_eq!(report.intended, 200);
    assert_eq!(report.enqueued, 200);
    assert_eq!(report.acked, 200);
    assert_eq!(report.unacked_at_exit, 0);
    assert_eq!(report.connects, 1);
    assert_eq!(report.connection_errors, 0);
    assert_eq!(broker.publishes(), 200);
    let summary: serde_json::Value =
        serde_json::from_slice(&std::fs::read(summary_path).unwrap()).unwrap();
    assert_eq!(summary["acked"], 200);
    assert_eq!(summary["unacked_at_exit"], 0);
    assert_eq!(summary["connects"], 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unacknowledged_messages_are_reported_not_counted_as_delivered() {
    let broker = FakeBroker::start().await;
    broker.hold_acks(true);

    let report = run_publisher(args(&broker, 10, 1)).await.unwrap();

    assert_eq!(report.enqueued, 10);
    assert_eq!(report.acked, 0);
    assert_eq!(report.unacked_at_exit, 10);
    assert_eq!(broker.publishes(), 10, "the messages did reach the broker");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_broker_restart_mid_run_shows_as_a_second_connect() {
    let broker = FakeBroker::start().await;
    let publisher = tokio::spawn(run_publisher(args(&broker, 100, 3)));
    tokio::time::sleep(Duration::from_secs(1)).await;
    broker.drop_connections();

    let report = publisher.await.unwrap().unwrap();

    assert_eq!(report.connects, 2, "one connect before the restart and one after");
    assert!(report.connection_errors >= 1);
    assert_eq!(report.intended, 300);
    assert_eq!(report.enqueued, report.acked + report.unacked_at_exit);
    assert!(report.acked >= 100, "messages after the reconnect are acknowledged again");
    assert_eq!(broker.connections(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_session_fails_the_run_before_anything_is_offered() {
    let broker = FakeBroker::start().await;
    broker.refuse_connections();
    let dir = tempfile::tempdir().unwrap();
    let summary_path = dir.path().join("publisher-summary.json");
    let mut args = args(&broker, 100, 60);
    args.summary_file = Some(summary_path.clone());

    let error = run_publisher(args).await.unwrap_err();

    assert!(error.to_string().contains("refused"), "{error:#}");
    assert!(!summary_path.exists(), "no summary for a run that never started");
    assert_eq!(broker.publishes(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_hotswap_request_fails_the_run_and_records_the_error() {
    let broker = FakeBroker::start().await;
    // Reserve a port, then close it so the POST is refused.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let api_url = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let dir = tempfile::tempdir().unwrap();
    let result_path = dir.path().join("swap_timeline.json");
    let mut args = args(&broker, 10, 1);
    args.profile = "hotswap-trigger".into();
    args.hotswap_target_node = Some("t1".into());
    args.hotswap_wasm_path = Some(PathBuf::from("plugins/v2.wasm"));
    args.hotswap_swap_at_secs = 0.1;
    args.hotswap_api_url = api_url;
    args.hotswap_result_path = Some(result_path.clone());

    let err = run_publisher(args).await.expect_err("a swap that never happened must fail");

    assert!(format!("{err:#}").contains("hot-swap"), "{err:#}");
    let artifact: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&result_path).unwrap()).unwrap();
    let request = &artifact["requests"][0];
    assert!(request["http_status"].is_null());
    assert!(request["error"].as_str().is_some_and(|e| e.contains("failed")), "{artifact}");
}
