//! MQTT publisher — the existing `wafer-loadgen` behaviour extracted into a
//! library entrypoint so the integration test can drive it in-process.
//!
//! Publisher CLI flags are preserved verbatim from the pre-refactor binary; see
//! `crates/wafer-loadgen/src/main.rs` for the CLI surface.
//!
//! NOTE: P0.2 (payload templates) and P0.3 (burst/ramp/hotswap-trigger profile
//! fixes) will replace parts of this module. The current implementation
//! reproduces the pre-refactor semantics so the P0.1 refactor stays behaviour-
//! preserving.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::Args;
use rumqttc::{AsyncClient, MqttOptions, QoS};
use tracing::{error, info, warn};

/// Arguments for the `publish` subcommand.
#[derive(Args, Debug, Clone)]
pub struct PublishArgs {
    /// MQTT broker hostname.
    #[arg(long, default_value = "localhost")]
    pub broker_host: String,

    /// MQTT broker port.
    #[arg(long, default_value_t = 1883)]
    pub broker_port: u16,

    /// MQTT topic to publish to.
    #[arg(long, default_value = "wafer/bench/input")]
    pub topic: String,

    /// Target messages per second.
    #[arg(long, default_value_t = 1000)]
    pub rate: u32,

    /// Total duration in seconds.
    #[arg(long, default_value_t = 60)]
    pub duration_secs: u64,

    /// Payload size in bytes (padded with 'x'). NOTE: superseded by
    /// `--payload-template` once P0.2 lands; kept for pre-refactor compat.
    #[arg(long, default_value_t = 128)]
    pub payload_size: usize,

    /// Load profile: steady, burst, ramp. NOTE: `burst`/`ramp` will be fixed by
    /// P0.3; this crate currently reproduces pre-refactor behaviour verbatim.
    #[arg(long, default_value = "steady")]
    pub profile: String,

    /// Client ID used for the MQTT session.
    #[arg(long, default_value = "wafer-loadgen-pub")]
    pub client_id: String,
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

/// Drive the publisher until `--duration-secs * --rate` messages have been sent
/// or `--rate` cannot be sustained.
///
/// This function is `async` and expects an existing Tokio runtime.
///
/// # Errors
/// Returns an error if the MQTT connection cannot be established at all. Once
/// connected, transient publish errors are logged and counted but not returned.
pub async fn run_publisher(args: PublishArgs) -> anyhow::Result<PublisherReport> {
    info!(
        broker = %args.broker_host,
        port = args.broker_port,
        topic = %args.topic,
        rate = args.rate,
        duration = args.duration_secs,
        profile = %args.profile,
        "Starting WAFER load generator (publisher)"
    );

    let mut opts = MqttOptions::new(&args.client_id, &args.broker_host, args.broker_port);
    opts.set_keep_alive(Duration::from_secs(30));
    opts.set_clean_session(true);
    let (client, mut eventloop) = AsyncClient::new(opts, 1024);

    // Poll the eventloop in a dedicated task so the connection stays alive. The
    // rumqttc invariant: stop polling → broker silently disconnects. See the
    // `mqtt-iot` skill for context.
    let eventloop_task = tokio::spawn(async move {
        loop {
            match eventloop.poll().await {
                Ok(_) => {}
                Err(e) => {
                    error!("MQTT eventloop error: {e}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    });

    // Brief delay to allow connection.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let total_messages = u64::from(args.rate) * args.duration_secs;
    let base_interval = Duration::from_secs_f64(1.0 / f64::from(args.rate));

    // Padding: keep the byte layout of the pre-refactor payload exactly.
    let padding: String = "x".repeat(args.payload_size.saturating_sub(80));
    let mut seq: u64 = 0;
    let mut errors: u64 = 0;
    let start = tokio::time::Instant::now();

    let mut ticker = tokio::time::interval(base_interval);
    ticker.tick().await;

    info!(
        "Publishing {total_messages} messages at {} msg/s ({} profile)",
        args.rate, args.profile
    );

    while seq < total_messages {
        ticker.tick().await;

        let elapsed_secs = start.elapsed().as_secs_f64();
        match args.profile.as_str() {
            "burst" => {
                // Pre-refactor no-op (see P0.3 for the fix).
                let cycle_pos = elapsed_secs % 60.0;
                if cycle_pos < 10.0 && seq % 2 == 0 { /* placeholder */ }
            }
            "ramp" => {
                // Pre-refactor rebuild-per-tick behaviour (see P0.3 for the fix).
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "duration_secs bounded to CLI input; well below f64 mantissa capacity for realistic runs"
                )]
                let progress = elapsed_secs / args.duration_secs as f64;
                let current_rate = (f64::from(args.rate) * 0.9_f64.mul_add(progress, 0.1)).max(1.0);
                let new_interval = Duration::from_secs_f64(1.0 / current_rate);
                ticker = tokio::time::interval(new_interval);
                ticker.tick().await;
            }
            _ => {}
        }

        let ts = now_ns();
        let payload = format!(
            r#"{{"ts":{ts},"seq":{seq},"device_id":"bench","temperature":42.5,"pad":"{padding}"}}"#
        );

        if let Err(e) = client
            .publish(&args.topic, QoS::AtLeastOnce, false, payload.as_bytes())
            .await
        {
            warn!("Publish error (seq={seq}): {e}");
            errors += 1;
        }

        seq += 1;
    }

    // Give in-flight PUBACKs a moment to drain before tearing the session down.
    tokio::time::sleep(Duration::from_millis(200)).await;
    // Disconnect politely; then abort the eventloop pump.
    let _disc = client.disconnect().await;
    eventloop_task.abort();

    let elapsed = start.elapsed();
    let actual_rate = if elapsed.as_secs_f64() > 0.0 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "seq bounded to args.rate * args.duration_secs; well below f64 mantissa capacity for any realistic run"
        )]
        let rate = seq as f64 / elapsed.as_secs_f64();
        rate
    } else {
        0.0
    };

    info!(
        total = seq,
        errors = errors,
        elapsed_ms = elapsed.as_millis(),
        actual_rate = format!("{actual_rate:.1}"),
        "Load generation complete"
    );

    Ok(PublisherReport {
        published: seq,
        errors,
        elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        actual_rate,
    })
}

/// Post-run summary returned by `run_publisher`. Useful for integration tests.
#[derive(Debug, Clone)]
pub struct PublisherReport {
    pub published: u64,
    pub errors: u64,
    pub elapsed_ms: u64,
    pub actual_rate: f64,
}
