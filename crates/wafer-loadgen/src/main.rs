//! WAFER Evaluation: MQTT load generator.
//!
//! Publishes JSON messages at a constant arrival rate to an MQTT topic with
//! embedded timestamps for end-to-end latency measurement.
//!
//! See docs/decisions/2025-07-12-evaluation-harness-design.md — Session 8 D3B.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::Parser;
use rumqttc::{AsyncClient, EventLoop, MqttOptions, QoS};
use tracing::{error, info, warn};

#[derive(Parser, Debug)]
#[command(name = "wafer-loadgen", about = "MQTT load generator for WAFER evaluation")]
struct Args {
    /// MQTT broker hostname.
    #[arg(long, default_value = "localhost")]
    broker_host: String,

    /// MQTT broker port.
    #[arg(long, default_value_t = 1883)]
    broker_port: u16,

    /// MQTT topic to publish to.
    #[arg(long, default_value = "wafer/bench/input")]
    topic: String,

    /// Target messages per second.
    #[arg(long, default_value_t = 1000)]
    rate: u32,

    /// Total duration in seconds.
    #[arg(long, default_value_t = 60)]
    duration_secs: u64,

    /// Payload size in bytes (padded with 'x').
    #[arg(long, default_value_t = 128)]
    payload_size: usize,

    /// Load profile: steady, burst, ramp.
    #[arg(long, default_value = "steady")]
    profile: String,
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

async fn connect_with_retry(
    host: &str,
    port: u16,
) -> anyhow::Result<(AsyncClient, EventLoop)> {
    let mut opts = MqttOptions::new("wafer-loadgen", host, port);
    opts.set_keep_alive(Duration::from_secs(30));
    opts.set_clean_session(true);

    let (client, eventloop) = AsyncClient::new(opts, 1024);
    Ok((client, eventloop))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    info!(
        broker = %args.broker_host,
        port = args.broker_port,
        topic = %args.topic,
        rate = args.rate,
        duration = args.duration_secs,
        profile = %args.profile,
        "Starting WAFER load generator"
    );

    let (client, mut eventloop) = connect_with_retry(&args.broker_host, args.broker_port).await?;

    // Spawn MQTT event loop handler (must poll to maintain connection)
    tokio::spawn(async move {
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

    // Brief delay to allow connection
    tokio::time::sleep(Duration::from_millis(500)).await;

    let total_messages = u64::from(args.rate) * args.duration_secs;
    let base_interval = Duration::from_secs_f64(1.0 / f64::from(args.rate));

    let padding: String = "x".repeat(args.payload_size.saturating_sub(80));
    let mut seq: u64 = 0;
    let mut errors: u64 = 0;
    let start = tokio::time::Instant::now();

    let mut ticker = tokio::time::interval(base_interval);
    // Consume first immediate tick
    ticker.tick().await;

    info!("Publishing {total_messages} messages at {} msg/s ({} profile)", args.rate, args.profile);

    while seq < total_messages {
        ticker.tick().await;

        // Profile-based rate adjustment
        let elapsed_secs = start.elapsed().as_secs_f64();
        match args.profile.as_str() {
            "burst" => {
                // 2x rate for 10s every 60s
                let cycle_pos = elapsed_secs % 60.0;
                if cycle_pos < 10.0 && seq % 2 == 0 {
                    // Extra message during burst (skip one tick worth of waiting)
                    // Effectively doubles rate for the burst window
                }
            }
            "ramp" => {
                // Linearly increase: start at rate/10, end at rate
                let progress = elapsed_secs / args.duration_secs as f64;
                let current_rate = (f64::from(args.rate) * (0.1 + 0.9 * progress)).max(1.0);
                let new_interval = Duration::from_secs_f64(1.0 / current_rate);
                ticker = tokio::time::interval(new_interval);
                ticker.tick().await; // consume immediate tick
            }
            _ => {} // steady: no adjustment
        }

        let ts = now_ns();
        let payload = format!(
            r#"{{"ts":{ts},"seq":{seq},"device_id":"bench","temperature":42.5,"pad":"{padding}"}}"#
        );

        if let Err(e) = client
            .publish(&args.topic, QoS::AtMostOnce, false, payload.as_bytes())
            .await
        {
            warn!("Publish error (seq={seq}): {e}");
            errors += 1;
        }

        seq += 1;
    }

    let elapsed = start.elapsed();
    let actual_rate = seq as f64 / elapsed.as_secs_f64();

    info!(
        total = seq,
        errors = errors,
        elapsed_ms = elapsed.as_millis(),
        actual_rate = format!("{actual_rate:.1}"),
        "Load generation complete"
    );

    Ok(())
}
