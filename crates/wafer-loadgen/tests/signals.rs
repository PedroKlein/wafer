//! SIGTERM and SIGINT stop the subscriber and publisher with their artifacts written.
//!
//! The binaries run against a closed broker port, so no Docker is needed.

#![cfg(test)]
#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

fn spawn_loadgen(args: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_wafer-loadgen"))
        .args(args)
        .env("RUST_LOG", "info")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn wait_for_log_line(child: &mut Child, needle: &'static str) {
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line.contains(needle) && tx.send(()).is_err() {
                break;
            }
        }
    });
    rx.recv_timeout(Duration::from_secs(20))
        .unwrap_or_else(|_| panic!("loadgen never logged {needle:?}"));
}

fn send_signal(child: &Child, signal: &str) {
    let status = Command::new("kill").args([signal, &child.id().to_string()]).status().unwrap();
    assert!(status.success());
}

fn wait_with_timeout(child: &mut Child) -> std::process::ExitStatus {
    for _ in 0..200 {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.kill().unwrap();
    panic!("loadgen did not exit within 10 s of the signal");
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// Checks what `path` holds against its `eval/result-schema.json` entry.
fn assert_matches_schema(producer: &str, path: &Path) {
    let schema_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../eval/result-schema.json");
    let schema = read_json(&schema_path);
    let name = path.file_name().unwrap().to_str().unwrap();
    let expected = &schema["producers"][producer][name];
    let bytes = std::fs::read(path).unwrap();
    if let Some(header) = expected["csv_header"].as_str() {
        assert_eq!(std::str::from_utf8(&bytes).unwrap().lines().next(), Some(header), "{name}");
    } else if let Some(keys) = expected["json_keys"].as_array() {
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let mut actual: Vec<&str> = value.as_object().unwrap().keys().map(String::as_str).collect();
        actual.sort_unstable();
        let expected: Vec<&str> = keys.iter().map(|key| key.as_str().unwrap()).collect();
        assert_eq!(actual, expected, "{name}");
    } else {
        assert_eq!(expected["hdr_encoding"], "v2", "{producer} {name} is not in the schema");
        assert_eq!(&bytes[..3], &[0x1c, 0x84, 0x93], "{name} is not a V2 histogram");
    }
}

fn assert_subscriber_stops_with_artifacts(signal: &str, exit_reason: &str) {
    let dir = tempfile::tempdir().unwrap();
    let output_dir = dir.path().to_str().unwrap();
    let mut child = spawn_loadgen(&[
        "subscribe",
        "--broker",
        "127.0.0.1:1",
        "--output-dir",
        output_dir,
        "--measurement-secs",
        "5",
    ]);
    wait_for_log_line(&mut child, "Starting WAFER loadgen subscriber");

    send_signal(&child, signal);
    let status = wait_with_timeout(&mut child);

    assert!(status.success(), "subscriber exited with {status}");
    for artifact in
        ["latency.hdr", "sequence.csv", "interval-latency.json", "subscriber-metadata.json"]
    {
        assert!(dir.path().join(artifact).is_file(), "{artifact} missing after {signal}");
        assert_matches_schema("wafer-loadgen subscribe", &dir.path().join(artifact));
    }
    let metadata = read_json(&dir.path().join("subscriber-metadata.json"));
    assert_eq!(metadata["exit_reason"], exit_reason);
    assert_eq!(metadata["status"], "complete");
}

#[test]
fn sigterm_stops_subscriber_with_artifacts() {
    assert_subscriber_stops_with_artifacts("-TERM", "sigterm");
}

#[test]
fn sigint_stops_subscriber_with_artifacts() {
    assert_subscriber_stops_with_artifacts("-INT", "sigint");
}

#[test]
fn sigterm_stops_publisher_with_summary() {
    let dir = tempfile::tempdir().unwrap();
    let summary = dir.path().join("publisher-summary.json");
    let mut child = spawn_loadgen(&[
        "publish",
        "--broker-host",
        "127.0.0.1",
        "--broker-port",
        "1",
        "--rate",
        "10",
        "--duration-secs",
        "600",
        "--summary-file",
        summary.to_str().unwrap(),
    ]);
    wait_for_log_line(&mut child, "Starting WAFER load generator");

    send_signal(&child, "-TERM");
    let status = wait_with_timeout(&mut child);

    assert!(status.success(), "publisher exited with {status}");
    assert_matches_schema("wafer-loadgen publish", &summary);
    let report = read_json(&summary);
    assert_eq!(report["exit_reason"], "sigterm");
    assert_eq!(
        report["intended"].as_u64().unwrap(),
        report["enqueued"].as_u64().unwrap() + report["rejected"].as_u64().unwrap()
    );
}

#[test]
fn second_sigterm_exits_a_subscriber_that_is_still_starting() {
    let dir = tempfile::tempdir().unwrap();
    let missing_receipt = dir.path().join("publisher-timing.json");
    let mut child = spawn_loadgen(&[
        "subscribe",
        "--broker",
        "127.0.0.1:1",
        "--output-dir",
        dir.path().to_str().unwrap(),
        "--publisher-timing-receipt",
        missing_receipt.to_str().unwrap(),
    ]);
    wait_for_log_line(&mut child, "Starting WAFER loadgen subscriber");

    send_signal(&child, "-TERM");
    std::thread::sleep(Duration::from_millis(200));
    assert!(child.try_wait().unwrap().is_none(), "first signal should not exit mid-startup");
    send_signal(&child, "-TERM");
    let status = wait_with_timeout(&mut child);

    assert_eq!(status.code(), Some(143));
}
