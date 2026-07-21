//! MQTT publisher — the existing `wafer-loadgen` behaviour extracted into a
//! library entrypoint so the integration test can drive it in-process.
//!
//! Publisher CLI flags are preserved verbatim from the pre-refactor binary; see
//! `crates/wafer-loadgen/src/main.rs` for the CLI surface. P0.2 adds
//! `--payload-template`, `--profile`, and `--dry-run` on top.
//!
//! NOTE: P0.3 (burst/ramp/hotswap-trigger profile fixes) will replace the
//! profile-selection logic in this module. The current implementation
//! reproduces the pre-refactor semantics so the P0.1/P0.2 refactor stays
//! behaviour-preserving for `--profile steady`.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::Args;
use rumqttc::{AsyncClient, MqttOptions, QoS};
use serde::Deserialize;
use tracing::{error, info, warn};

use crate::payload::PayloadTemplate;

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

    /// Payload size in bytes (padded with 'x'). Ignored when
    /// `--payload-template` is set. Kept for pre-refactor CLI compat.
    #[arg(long, default_value_t = 128)]
    pub payload_size: usize,

    /// Deterministic payload template. Preferred over `--payload-size`.
    /// Templates: `telemetry-120b`, `generic-1kb`, `generic-10kb`, `generic-100kb`.
    #[arg(long)]
    pub payload_template: Option<PayloadTemplate>,

    /// Load profile: steady, burst, ramp. NOTE: `burst`/`ramp` will be fixed by
    /// P0.3; this crate currently reproduces pre-refactor behaviour verbatim.
    #[arg(long, default_value = "steady")]
    pub profile: String,

    /// Client ID used for the MQTT session.
    #[arg(long, default_value = "wafer-loadgen-pub")]
    pub client_id: String,

    /// Path to a TOML profile that pre-fills any of the above flags. Explicit
    /// CLI flags override profile values (following the `clap` default logic).
    #[arg(long)]
    pub profile_file: Option<PathBuf>,

    /// Print the resolved configuration + template info and exit without
    /// contacting the broker. Verify AC3 for P0.2 by combining with `--profile`.
    #[arg(long, default_value_t = false)]
    pub dry_run: bool,
}

/// A subset of `PublishArgs` deserialised from a `[loadgen]` TOML table.
///
/// Fields are optional so a profile can fill in only what it needs; CLI flags
/// then override on top.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ProfileFile {
    pub loadgen: LoadgenProfile,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct LoadgenProfile {
    pub broker_host: Option<String>,
    pub broker_port: Option<u16>,
    pub topic: Option<String>,
    pub rate: Option<u32>,
    pub duration_secs: Option<u64>,
    pub payload_size: Option<usize>,
    pub payload_template: Option<String>,
    pub profile: Option<String>,
    pub warmup_secs: Option<u64>,
    pub client_id: Option<String>,
}

impl PublishArgs {
    /// Fold a `--profile-file` TOML into `self`. Values set on `self` via the
    /// CLI take precedence over profile values, EXCEPT for defaulted fields
    /// (which are indistinguishable from unset on the CLI without extra
    /// bookkeeping). For P0.2 shakedown scope we prefer profile when set,
    /// unless the CLI value diverges from the compile-time default.
    ///
    /// # Errors
    /// Returns an error if the TOML cannot be parsed or if
    /// `payload_template` names an unknown template.
    pub fn apply_profile_file(&mut self) -> anyhow::Result<()> {
        let Some(path) = self.profile_file.clone() else {
            return Ok(());
        };
        let text = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("read profile {}: {e}", path.display()))?;
        let cfg: ProfileFile = toml::from_str(&text)
            .map_err(|e| anyhow::anyhow!("parse profile {}: {e}", path.display()))?;
        let lg = cfg.loadgen;
        // Only apply when the CLI still holds its compile-time default.
        if self.broker_host == "localhost" {
            if let Some(v) = lg.broker_host { self.broker_host = v; }
        }
        if self.broker_port == 1883 {
            if let Some(v) = lg.broker_port { self.broker_port = v; }
        }
        if self.topic == "wafer/bench/input" {
            if let Some(v) = lg.topic { self.topic = v; }
        }
        if self.rate == 1000 {
            if let Some(v) = lg.rate { self.rate = v; }
        }
        if self.duration_secs == 60 {
            if let Some(v) = lg.duration_secs { self.duration_secs = v; }
        }
        if self.payload_size == 128 {
            if let Some(v) = lg.payload_size { self.payload_size = v; }
        }
        if self.profile == "steady" {
            if let Some(v) = lg.profile { self.profile = v; }
        }
        if self.client_id == "wafer-loadgen-pub" {
            if let Some(v) = lg.client_id { self.client_id = v; }
        }
        if self.payload_template.is_none() {
            if let Some(name) = lg.payload_template {
                self.payload_template = Some(
                    name.parse::<PayloadTemplate>()
                        .map_err(|e| anyhow::anyhow!("{e}"))?,
                );
            }
        }
        Ok(())
    }

    /// Human-readable summary used by `--dry-run` and by tests. Includes the
    /// template fingerprint so an operator can eyeball reproducibility.
    #[must_use]
    pub fn dry_run_report(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let _r = writeln!(out, "wafer-loadgen publish (dry-run)");
        let _r = writeln!(out, "  broker         = {}:{}", self.broker_host, self.broker_port);
        let _r = writeln!(out, "  topic          = {}", self.topic);
        let _r = writeln!(out, "  rate           = {} msg/s", self.rate);
        let _r = writeln!(out, "  duration_secs  = {}", self.duration_secs);
        let _r = writeln!(out, "  profile        = {}", self.profile);
        if let Some(tpl) = self.payload_template {
            let bytes = tpl.render(crate::payload::CANONICAL_TS_NS, crate::payload::CANONICAL_SEQ);
            let _r = writeln!(out, "  payload        = template `{}` ({} bytes at canonical ts/seq)", tpl.name(), bytes.len());
            let _r = writeln!(out, "  fingerprint    = sha256:{}", tpl.fingerprint_hex());
        } else {
            let _r = writeln!(out, "  payload        = size {} (legacy 'x' filler)", self.payload_size);
        }
        if let Some(pf) = &self.profile_file {
            let _r = writeln!(out, "  profile_file   = {}", pf.display());
        }
        out
    }
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
#[expect(
    clippy::too_many_lines,
    reason = "single-function driver mirrors pre-refactor structure; P0.3 splits burst/ramp into a profile module and this will shrink naturally"
)]
pub async fn run_publisher(mut args: PublishArgs) -> anyhow::Result<PublisherReport> {
    args.apply_profile_file()?;
    if args.dry_run {
        // The workspace bans print_stdout; go through tracing::info so the
        // report is captured in structured logs (grep-friendly for the eval
        // scripts). Using a multi-line info! keeps it as one span.
        info!(target: "wafer_loadgen::publish::dry_run", "{}", args.dry_run_report());
        return Ok(PublisherReport {
            published: 0,
            errors: 0,
            elapsed_ms: 0,
            actual_rate: 0.0,
        });
    }
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

    // Padding: keep the byte layout of the pre-refactor payload exactly when
    // no template is set. When `--payload-template` is set, the template is
    // the source of truth and `--payload-size` is ignored (already logged via
    // dry-run above).
    let padding: String = "x".repeat(args.payload_size.saturating_sub(80));
    let payload_template = args.payload_template;
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
        let payload_vec: Vec<u8> = payload_template.map_or_else(
            || format!(
                r#"{{"ts":{ts},"seq":{seq},"device_id":"bench","temperature":42.5,"pad":"{padding}"}}"#
            )
            .into_bytes(),
            |tpl| tpl.render(ts, seq),
        );

        if let Err(e) = client
            .publish(&args.topic, QoS::AtLeastOnce, false, payload_vec.as_slice())
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
