//! End-to-end round-trip test for the `wafer-loadgen` publisher and subscriber.
//!
//! Spins up `eclipse-mosquitto:2.0.18` in a Docker container via `testcontainers`,
//! runs the publisher and subscriber against it in-process, and asserts:
//!
//! - `AC3a`: `10_000` messages round-trip.
//! - `AC3b`: zero loss (no sequence gaps).
//! - `AC3c`: zero duplicates.
//! - `AC3d`: `latency.hdr` starts with the [`HdrHistogram`] V2 cookie prefix (`1c 84 93`).
//! - `AC3e`: `subscriber-metadata.json` deserialises and reports the expected counts.
//!
//! # Skipping
//! Set `WAFER_SKIP_DOCKER_TESTS=1` to skip when Docker is unavailable (e.g. in
//! constrained CI). Otherwise the test WILL fail if the daemon is unreachable —
//! silently skipping tests is a lie by omission and defeats the purpose of the
//! integration harness.
//!
//! # macOS note
//! On this machine Docker is provided by colima; the test picks up `DOCKER_HOST`
//! automatically via bollard. If you use Docker Desktop, no extra setup is
//! needed.

use std::path::PathBuf;
use std::time::Duration;

use testcontainers::runners::AsyncRunner;
use testcontainers_modules::mosquitto::Mosquitto;

use wafer_loadgen::{PublishArgs, SubscribeArgs, run_publisher, run_subscriber, SubscriberMetadata};

const TOTAL_MESSAGES: u64 = 10_000;

fn should_skip() -> bool {
    std::env::var("WAFER_SKIP_DOCKER_TESTS").is_ok()
}

/// Best-effort `DOCKER_HOST` auto-detection.
///
/// Bollard defaults to `/var/run/docker.sock` when `DOCKER_HOST` is unset. On
/// macOS with colima that socket does not exist; the real one lives under
/// `~/.colima/default/docker.sock`. Probe common locations and export
/// `DOCKER_HOST` before any testcontainers call so the test works whether the
/// user runs Docker Desktop, colima, or a rootless setup.
fn ensure_docker_host() {
    if std::env::var_os("DOCKER_HOST").is_some() {
        return;
    }
    let candidates = [
        "/var/run/docker.sock".to_owned(),
        format!(
            "{}/.colima/default/docker.sock",
            std::env::var("HOME").unwrap_or_default()
        ),
        format!(
            "{}/.docker/run/docker.sock",
            std::env::var("HOME").unwrap_or_default()
        ),
    ];
    for path in candidates {
        if std::path::Path::new(&path).exists() {
            // SAFETY: single-threaded before we spawn the runtime; nothing
            // else in the process is reading DOCKER_HOST at this point.
            // SAFETY: We are still inside `#[tokio::test]` setup on the main
            // thread; no other test threads can observe this race.
            unsafe { std::env::set_var("DOCKER_HOST", format!("unix://{path}")) };
            return;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "integration test uses assert_eq! for AC3 verification; failing an assertion should terminate the test even though it returns Result"
)]
async fn round_trip_10k_messages_reports_zero_loss_and_zero_duplicates() -> anyhow::Result<()> {
    if should_skip() {
        // Deliberately silent: the workspace bans print_stderr; the skip is
        // signalled via test-runner output when the caller sets the env var.
        return Ok(());
    }
    ensure_docker_host();
    let _init = tracing_subscriber::fmt()
        .with_env_filter("info,rumqttc=warn,bollard=warn")
        .with_test_writer()
        .try_init();

    // Boot mosquitto (anonymous, ephemeral). testcontainers pulls the image
    // lazily on the first run of this test; subsequent runs are cached.
    let broker = Mosquitto::default().start().await?;
    let host = broker.get_host().await?;
    let port = broker.get_host_port_ipv4(1883).await?;
    let broker_uri = format!("{host}:{port}");

    // Use a unique topic per test run to avoid cross-test bleed even if the
    // broker outlives an aborted previous run (which testcontainers cleans up
    // via the reaper but belt-and-suspenders is cheap).
    let topic = format!("wafer/roundtrip/{}", std::process::id());
    let output_dir: PathBuf = tempfile::tempdir()?.keep(); // keep on failure for post-mortem

    // Subscriber: start FIRST so it is subscribed before the publisher fires
    // its first message. The subscriber advertises via ConnAck-subscribe;
    // rumqttc's default flow means we should wait ~200ms after subscribe
    // returns before publishing. We handle this with a small delay below.
    let sub_args = SubscribeArgs {
        broker: broker_uri.clone(),
        topic: topic.clone(),
        output_dir: output_dir.clone(),
        total_messages: TOTAL_MESSAGES,
        client_id: format!("wafer-loadgen-sub-{}", std::process::id()),
        host_tag: Some("shakedown-macos".into()),
        qos: 1,
    };
    let sub_handle = tokio::spawn(async move { run_subscriber(sub_args).await });

    // Give the subscriber time to connect + subscribe. 1s is generous even
    // under Docker Desktop's cold-start network overhead.
    tokio::time::sleep(Duration::from_secs(1)).await;

    let pub_args = PublishArgs {
        broker_host: host.to_string(),
        broker_port: port,
        topic: topic.clone(),
        rate: 5_000,             // 10k msgs / 2s
        duration_secs: 2,
        payload_size: 120,       // Approximate telemetry-120b shape (P0.2 will formalise).
        payload_template: None,  // Legacy 'x' filler path; template validation lives in payload tests.
        profile: "steady".into(),
        client_id: format!("wafer-loadgen-pub-{}", std::process::id()),
        profile_file: None,
        dry_run: false,
    };
    let pub_report = run_publisher(pub_args).await?;
    assert_eq!(
        pub_report.published, TOTAL_MESSAGES,
        "publisher reported unexpected sent count"
    );

    // Subscriber will exit once TOTAL_MESSAGES observed. Cap the wait so a
    // hung test fails loudly instead of blocking CI.
    let sub_report = tokio::time::timeout(Duration::from_secs(30), sub_handle)
        .await
        .map_err(|e| anyhow::anyhow!("subscriber did not finish within 30s of publisher exit: {e}"))?
        ??;

    // AC3a: 10k messages observed.
    assert_eq!(
        sub_report.total_messages, TOTAL_MESSAGES,
        "expected {TOTAL_MESSAGES} messages, got {}",
        sub_report.total_messages
    );
    assert_eq!(
        sub_report.total_recorded, TOTAL_MESSAGES,
        "expected {TOTAL_MESSAGES} recorded latencies, got {}",
        sub_report.total_recorded
    );
    assert_eq!(sub_report.parse_errors, 0, "unexpected JSON parse errors");

    // AC3b + AC3c: zero loss, zero duplicates.
    assert_eq!(sub_report.total_gaps, 0, "expected zero gaps, got {}", sub_report.total_gaps);
    assert_eq!(
        sub_report.total_duplicates, 0,
        "expected zero duplicates, got {}",
        sub_report.total_duplicates
    );

    // AC3d: latency.hdr magic bytes.
    let hdr = std::fs::read(output_dir.join("latency.hdr"))?;
    assert!(
        hdr.len() > 4,
        "latency.hdr suspiciously small: {} bytes",
        hdr.len()
    );
    assert_eq!(
        &hdr[..3],
        &[0x1c, 0x84, 0x93],
        "latency.hdr missing V2 cookie prefix; got {:02x?}",
        &hdr[..hdr.len().min(8)]
    );

    // AC3e: subscriber-metadata.json.
    let meta_text = std::fs::read_to_string(output_dir.join("subscriber-metadata.json"))?;
    let meta: SubscriberMetadata = serde_json::from_str(&meta_text)?;
    assert_eq!(meta.total_recorded, TOTAL_MESSAGES);
    assert_eq!(meta.sequence.total_received, TOTAL_MESSAGES);
    assert_eq!(meta.sequence.total_gaps, 0);
    assert_eq!(meta.sequence.total_duplicates, 0);
    assert_eq!(meta.topic, topic);
    assert_eq!(meta.exit_reason, "total-messages");
    assert_eq!(meta.host_tag.as_deref(), Some("shakedown-macos"));
    assert_eq!(meta.histogram_sig_digits, 3);
    // Latency should be positive (broker adds ≥ a few µs). Zero would indicate
    // that intended_publish_ns == receive_ns i.e. broken measurement.
    assert!(meta.latency_p50_ns > 0, "p50 must be > 0 for real traffic");

    // sequence.csv is header-only when zero gaps/dups.
    let csv = std::fs::read_to_string(output_dir.join("sequence.csv"))?;
    assert_eq!(csv, "event_type,seq_start,seq_end,count\n");

    Ok(())
}
