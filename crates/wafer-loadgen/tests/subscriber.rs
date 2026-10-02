//! Subscriber run-end behaviour against the in-process broker.

mod common;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::FakeBroker;
use wafer_loadgen::{SubscribeArgs, run_subscriber};

fn payload(seq: u64) -> Vec<u8> {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    format!(r#"{{"ts":{ts},"seq":{seq}}}"#).into_bytes()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_duplicate_does_not_stop_the_subscriber_before_the_last_measured_message() {
    let broker = FakeBroker::start().await;
    broker.deliver_on_subscribe([0, 0, 1, 2].map(payload).to_vec());
    let output = tempfile::tempdir().unwrap();
    let args = SubscribeArgs {
        broker: format!("{}:{}", broker.host(), broker.port()),
        topic: "wafer/test".into(),
        output_dir: output.path().to_path_buf(),
        total_messages: 3,
        client_id: "wafer-loadgen-sub-test".into(),
        host_tag: None,
        qos: 1,
        trace_file: None,
        sequence_example_limit: None,
        sequence_end_exclusive: Some(3),
        measurement_secs: 1,
        drain_grace_secs: 5,
        publisher_timing_receipt: None,
        action_timing_receipt: None,
    };

    let report =
        tokio::time::timeout(Duration::from_secs(10), run_subscriber(args)).await.unwrap().unwrap();

    assert_eq!(report.exit_reason, "total-messages");
    assert_eq!(report.total_duplicates, 1);
    assert_eq!(report.total_gaps, 0);
    let intervals: serde_json::Value = serde_json::from_slice(
        &std::fs::read(output.path().join("interval-latency.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(intervals["drain_grace_ns"], 5_000_000_000_u64);
    assert_eq!(intervals["publisher_drain_ns"], 6_000_000_000_u64);
    assert_eq!(intervals["maximum_rows"], 1 + 6 + 5 + 2);
}
