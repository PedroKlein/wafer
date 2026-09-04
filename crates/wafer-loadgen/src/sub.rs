//! MQTT subscriber: connects to the pipeline's output topic, drives the
//! `LatencyRecorder`, and flushes artifacts on graceful exit.
//!
//! Runs one broker + one topic. Multi-topic recording is explicitly out of
//! scope (see P0.1 non-goals in the plan). One subscriber instance per output
//! topic is the intended composition model — the eval scripts (P1.1) start
//! one per SUT/comparator.
//!
//! Cancel model: two exit signals compete. Whichever fires first wins.
//! 1. SIGINT / Ctrl-C via `tokio::signal::ctrl_c()`.
//! 2. `--total-messages` reached (or its default).
//!
//! Both paths flush artifacts before returning.

use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::Duration;

use clap::Args;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::recorder::{
    EventBucketRecorder, LatencyRecorder, PublisherTimingReceipt, RecordOutcome, SequenceReport,
    SubscriberMetadata, now_ns,
};

/// Arguments for the `subscribe` subcommand.
#[derive(Args, Debug, Clone)]
pub struct SubscribeArgs {
    /// MQTT broker in `host` or `host:port` form.
    #[arg(long, default_value = "localhost:1883")]
    pub broker: String,

    /// Topic to subscribe to.
    #[arg(long, default_value = "wafer/bench/output")]
    pub topic: String,

    /// Output directory to write latency.hdr, sequence.csv, and
    /// subscriber-metadata.json into.
    #[arg(long)]
    pub output_dir: PathBuf,

    /// Exit gracefully after this many messages have been *observed* (parsed
    /// or not). Set to 0 to run until SIGINT.
    #[arg(long, default_value_t = 0)]
    pub total_messages: u64,

    /// Client ID for the MQTT session. Deterministic to avoid ID collisions.
    #[arg(long, default_value = "wafer-loadgen-sub")]
    pub client_id: String,

    /// Host tag baked into subscriber-metadata.json. Set by the eval scripts
    /// (P1.1) — `shakedown-macos` for this plan.
    #[arg(long)]
    pub host_tag: Option<String>,

    /// Optional [`QoS`] override for the subscription. [`QoS`] 1 (at-least-once) is
    /// the default per RFC-008 / edge-IoT guidance in the mqtt-iot skill.
    #[arg(long, default_value_t = 1)]
    pub qos: u8,

    /// Write every recorded sequence and timestamp pair as CSV.
    /// Intended for diagnostics; omit during canonical measurements.
    #[arg(long)]
    pub trace_file: Option<PathBuf>,

    /// Retain at most this many gap and duplicate examples while preserving exact totals.
    #[arg(long)]
    pub sequence_example_limit: Option<usize>,

    /// Ignore messages at or above this sequence number.
    #[arg(long)]
    pub sequence_end_exclusive: Option<u64>,

    /// Publisher timing receipt used to align bounded disruption buckets.
    #[arg(long)]
    pub publisher_timing_receipt: Option<PathBuf>,
}

fn parse_broker(s: &str) -> (String, u16) {
    if let Some((host, port)) = s.rsplit_once(':') {
        if let Ok(port) = port.parse::<u16>() {
            return (host.to_owned(), port);
        }
    }
    (s.to_owned(), 1883)
}

const fn qos_from_u8(q: u8) -> QoS {
    match q {
        0 => QoS::AtMostOnce,
        2 => QoS::ExactlyOnce,
        _ => QoS::AtLeastOnce,
    }
}

/// Run the subscriber loop. Returns after either SIGINT or `total_messages`
/// (whichever comes first). Artifacts are written to `args.output_dir` before
/// return, regardless of exit path.
///
/// # Errors
/// - MQTT connection failure (never reached; rumqttc reconnects internally).
/// - Artifact write failure.
#[expect(
    clippy::too_many_lines,
    reason = "single-function driver keeps the cancel-safe select! and the metadata\n     construction in one place; splitting into helpers would obscure the exit-path invariant."
)]
pub async fn run_subscriber(args: SubscribeArgs) -> anyhow::Result<SubscriberReport> {
    let (host, port) = parse_broker(&args.broker);
    let broker_display = format!("{host}:{port}");
    info!(
        broker = %broker_display,
        topic = %args.topic,
        output_dir = %args.output_dir.display(),
        total_messages = args.total_messages,
        sequence_end_exclusive = args.sequence_end_exclusive,
        "Starting WAFER loadgen subscriber"
    );

    let mut opts = MqttOptions::new(&args.client_id, &host, port);
    opts.set_keep_alive(Duration::from_secs(30));
    opts.set_clean_session(true);
    let (client, mut eventloop) = AsyncClient::new(opts, 1024);

    // Bounded channel from eventloop task → recording loop. Bounded so an
    // overwhelmed recorder pushes back on the eventloop and, in turn, the broker.
    let (tx, mut rx) = mpsc::channel::<(u64, Vec<u8>)>(2048);
    let topic_for_task = args.topic.clone();
    let qos = qos_from_u8(args.qos);

    let eventloop_task = tokio::spawn(async move {
        loop {
            match eventloop.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    // Every reconnect: re-subscribe. rumqttc + clean_session
                    // drops broker-side subs on disconnect — see the mqtt-iot
                    // skill for the #1 rumqttc bug this guards against.
                    if let Err(e) = client.subscribe(&topic_for_task, qos).await {
                        warn!("subscribe failed: {e}");
                    }
                }
                Ok(Event::Incoming(Packet::Publish(msg))) => {
                    let receive_ns = now_ns();
                    if tx.send((receive_ns, msg.payload.to_vec())).await.is_err() {
                        // Recorder loop has hung up — we're shutting down.
                        break;
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    // rumqttc auto-reconnects on the next poll(); sleep to
                    // avoid tight error loops burning CPU.
                    debug!("mqtt eventloop error (auto-recovering): {e}");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }
    });

    let mut recorder = LatencyRecorder::with_sequence_example_limit(args.sequence_example_limit);
    let mut event_buckets = match &args.publisher_timing_receipt {
        Some(path) => {
            let deadline = tokio::time::Instant::now()
                .checked_add(Duration::from_secs(10))
                .unwrap_or_else(tokio::time::Instant::now);
            let receipt = loop {
                match tokio::fs::read(path).await {
                    Ok(bytes) => break serde_json::from_slice::<PublisherTimingReceipt>(&bytes)?,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::NotFound
                            && tokio::time::Instant::now() < deadline =>
                    {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(error) => return Err(error.into()),
                }
            };
            Some(Box::new(EventBucketRecorder::new(&receipt)?))
        }
        None => None,
    };
    let mut trace =
        args.trace_file.as_ref().map(std::fs::File::create).transpose()?.map(BufWriter::new);
    if let Some(trace) = &mut trace {
        writeln!(trace, "seq,payload_ts_ns,receive_ns,latency_ns")?;
    }
    let started_at_ns = now_ns();

    // Ctrl-C exit path. On non-unix builds `ctrl_c` still resolves.
    let sigint = tokio::signal::ctrl_c();
    tokio::pin!(sigint);

    let mut exit_reason = "eof";
    loop {
        tokio::select! {
            biased;
            _ = &mut sigint => {
                info!("SIGINT received — flushing artifacts");
                exit_reason = "sigint";
                break;
            }
            recv = rx.recv() => {
                match recv {
                    Some((receive_ns, payload)) => {
                        let outcome = match args.sequence_end_exclusive {
                            Some(end) => recorder.record_json_before(&payload, receive_ns, end),
                            None => recorder.record_json(&payload, receive_ns),
                        };
                        if let RecordOutcome::Recorded { latency_ns, seq } = outcome {
                            if let Some(buckets) = &mut event_buckets {
                                buckets.record(receive_ns, recorder.last_record_duplicate());
                            }
                            if let Some(trace) = &mut trace {
                                let payload_ts_ns = receive_ns.saturating_sub(latency_ns);
                                writeln!(trace, "{seq},{payload_ts_ns},{receive_ns},{latency_ns}")?;
                            }
                        }
                        if args.total_messages > 0 && recorder.total_messages() >= args.total_messages {
                            exit_reason = "total-messages";
                            info!(
                                total = recorder.total_messages(),
                                "Reached --total-messages; flushing artifacts"
                            );
                            break;
                        }
                    }
                    None => {
                        // Eventloop task ended (broker gone forever) — flush anyway.
                        break;
                    }
                }
            }
        }
    }

    let ended_at_ns = now_ns();
    eventloop_task.abort();
    if let Some(trace) = &mut trace {
        trace.flush()?;
    }

    let metadata = SubscriberMetadata {
        broker: broker_display,
        topic: args.topic.clone(),
        started_at_ns,
        ended_at_ns,
        exit_reason: exit_reason.to_owned(),
        git_sha: std::env::var("WAFER_GIT_SHA").ok(),
        host_tag: args.host_tag.clone(),
        sequence_end_exclusive: args.sequence_end_exclusive,
        ignored_sequence_count: 0,
        unexpected_sequence_count: 0,
        // Measurement fields are filled by write_artifacts.
        total_recorded: 0,
        total_messages: 0,
        parse_errors: 0,
        negative_latency_count: 0,
        latency_min_ns: 0,
        latency_max_ns: 0,
        latency_mean_ns: 0.0,
        latency_p50_ns: 0,
        latency_p95_ns: 0,
        latency_p99_ns: 0,
        latency_p999_ns: 0,
        histogram_lowest_ns: 0,
        histogram_highest_ns: 0,
        histogram_sig_digits: 0,
        sequence: SequenceReport {
            total_received: 0,
            total_gaps: 0,
            total_duplicates: 0,
            gap_ranges: vec![],
            duplicate_seqs: vec![],
            examples_truncated: false,
        },
    };

    recorder.write_artifacts(&args.output_dir, metadata)?;
    if let Some(buckets) = event_buckets {
        buckets.write(&args.output_dir.join("throughput-buckets.json"))?;
    }

    let report = SubscriberReport {
        total_messages: recorder.total_messages(),
        total_recorded: recorder.total_recorded(),
        parse_errors: recorder.parse_errors(),
        ignored_sequences: recorder.ignored_sequences(),
        total_gaps: recorder.sequence().total_gaps(),
        total_duplicates: recorder.sequence().total_duplicates(),
        p50_ns: recorder.p50_ns(),
        p99_ns: recorder.p99_ns(),
        exit_reason: exit_reason.to_owned(),
    };

    info!(
        total = report.total_messages,
        ignored_sequences = report.ignored_sequences,
        gaps = report.total_gaps,
        duplicates = report.total_duplicates,
        p50_ms = ms_from_ns(report.p50_ns),
        p99_ms = ms_from_ns(report.p99_ns),
        "Subscriber done"
    );

    Ok(report)
}

/// Post-run summary returned by `run_subscriber`. Test harnesses assert on this.
#[derive(Debug, Clone)]
pub struct SubscriberReport {
    pub total_messages: u64,
    pub total_recorded: u64,
    pub parse_errors: u64,
    pub ignored_sequences: u64,
    pub total_gaps: u64,
    pub total_duplicates: u64,
    pub p50_ns: u64,
    pub p99_ns: u64,
    pub exit_reason: String,
}

/// Convert nanoseconds to milliseconds as an `f64`.
///
/// Precision loss above ~2^53 ns (~104 days) is acceptable — the histogram is
/// bounded to 10 s so this can never approach that range.
#[expect(
    clippy::cast_precision_loss,
    clippy::as_conversions,
    reason = "latency values are bounded to 10 s = 10^10 ns, well below f64 mantissa capacity"
)]
fn ms_from_ns(ns: u64) -> f64 {
    ns as f64 / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::parse_broker;

    #[test]
    fn parse_broker_host_only_defaults_port_1883() {
        assert_eq!(parse_broker("localhost"), ("localhost".into(), 1883));
    }

    #[test]
    fn parse_broker_host_and_port() {
        assert_eq!(parse_broker("mosquitto:8883"), ("mosquitto".into(), 8883));
    }

    #[test]
    fn parse_broker_invalid_port_treated_as_host() {
        // The rsplit_once will yield ("mosquitto", "notaport"); parse fails so
        // we fall back to the full string as host, default port.
        assert_eq!(parse_broker("mosquitto:notaport"), ("mosquitto:notaport".into(), 1883));
    }
}
