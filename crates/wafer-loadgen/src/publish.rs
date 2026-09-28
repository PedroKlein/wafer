//! MQTT publisher — refactored (P0.3) to drive from a `LoadShape` scheduler
//! rather than rebuilding `tokio::time::interval` per tick.
//!
//! # Load shapes
//! - `steady`   — constant rate.
//! - `burst`    — 2× base rate for `on_secs` every `cycle_secs`.
//! - `ramp`     — stepwise increasing rate up to `max_rate`.
//! - `hotswap-trigger` — steady rate; a companion task POSTs to the
//!   runtime's hot-swap API at `swap_at_secs`. E-Swap experiments consume this.
//!
//! The scheduling model is open-loop (Tene 2012): message N's target publish
//! offset is `sum(1/rate(t_k)) for k in 0..N`, independent of whether previous
//! publishes were on time. Each payload's `ts` is that target time, not the
//! time it was sent, so when the publisher falls behind (a full client queue,
//! a slow broker) the delay counts as latency and the overdue messages go out
//! back to back. How late each message actually left is reported separately
//! as `source_lag_ns` in the publisher summary.
//!
//! Target times are paced by a dedicated OS thread: the Tokio timer rounds
//! every deadline up to the next millisecond, which would add up to 1 ms of
//! lag to every message and release high rates in per-millisecond bursts.
//!
//! # Compat
//! Publisher CLI flags from the pre-refactor binary are preserved. The
//! `--profile` string still accepts `steady`, `burst`, `ramp`; P0.3 adds
//! `hotswap-trigger` and the `--hotswap-*` flags to configure it.

use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hdrhistogram::Histogram;

use clap::Args;
use rumqttc::{AsyncClient, MqttOptions, QoS};
use serde::{Deserialize, Serialize};
use tokio::time::Instant;
use tracing::{error, info, warn};

use crate::payload::PayloadTemplate;
use crate::profile::{LoadShape, Scheduler};
use crate::recorder::{PublisherTimingReceipt, write_atomic};
use crate::stop_signal::StopSignal;

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

    /// Target base rate (msg/s). For burst / ramp / hotswap-trigger profiles
    /// this is the *base* rate — variations layer on top.
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
    #[arg(long)]
    pub payload_template: Option<PayloadTemplate>,

    /// Load profile: `steady`, `burst`, `ramp`, `hotswap-trigger`.
    #[arg(long, default_value = "steady")]
    pub profile: String,

    /// Client ID used for the MQTT session.
    #[arg(long, default_value = "wafer-loadgen-pub")]
    pub client_id: String,

    /// Path to a TOML profile file that pre-fills flags. Explicit CLI values
    /// override profile values when they differ from the compile-time default.
    #[arg(long)]
    pub profile_file: Option<PathBuf>,

    /// Print the resolved configuration and exit without contacting the broker.
    #[arg(long, default_value_t = false)]
    pub dry_run: bool,

    // --- Burst profile knobs ---
    /// Multiplier applied to `rate` during the burst window.
    #[arg(long, default_value_t = 2)]
    pub burst_multiplier: u32,

    /// Burst window duration (seconds).
    #[arg(long, default_value_t = 10)]
    pub burst_on_secs: u64,

    /// Full cycle duration (burst + steady).
    #[arg(long, default_value_t = 60)]
    pub burst_cycle_secs: u64,

    // --- Ramp profile knobs ---
    /// Ramp start rate (msg/s).
    #[arg(long, default_value_t = 100)]
    pub ramp_start_rate: u32,

    /// Ramp per-step increment (msg/s).
    #[arg(long, default_value_t = 100)]
    pub ramp_step_rate: u32,

    /// Ramp step interval (seconds).
    #[arg(long, default_value_t = 10)]
    pub ramp_step_interval_secs: u64,

    /// Ramp ceiling (msg/s).
    #[arg(long, default_value_t = 10_000)]
    pub ramp_max_rate: u32,

    // --- Hotswap-trigger knobs ---
    /// Node ID to swap on when profile = hotswap-trigger.
    #[arg(long)]
    pub hotswap_target_node: Option<String>,

    /// New Wasm plugin path. Must be a filesystem path the wafer-runtime
    /// process can read.
    #[arg(long)]
    pub hotswap_wasm_path: Option<PathBuf>,

    /// Seconds into the run at which to POST the hot-swap. Fractional OK.
    #[arg(long, default_value_t = 30.0)]
    pub hotswap_swap_at_secs: f64,

    /// Base URL of the wafer-runtime HTTP API (no trailing slash). The POST
    /// target is `{api_url}/api/v1/nodes/{target_node}/hot-swap`.
    #[arg(long, default_value = "http://localhost:9090")]
    pub hotswap_api_url: String,

    /// Write every offered sequence and its payload `ts` (scheduled time) as CSV.
    /// Intended for diagnostics; omit during canonical measurements.
    #[arg(long)]
    pub trace_file: Option<PathBuf>,

    /// Write bounded publisher counters as JSON.
    #[arg(long)]
    pub summary_file: Option<PathBuf>,

    /// First sequence number emitted by this publisher process.
    #[arg(long, default_value_t = 0)]
    pub sequence_start: u64,

    /// Reject messages when the MQTT client queue is full instead of waiting.
    #[arg(long, default_value_t = false)]
    pub drop_when_full: bool,

    /// Write the hot-swap HTTP response as a canonical timeline artifact.
    #[arg(long)]
    pub hotswap_result_path: Option<PathBuf>,

    /// Atomically publish the measured boundary used for cross-process event alignment.
    #[arg(long)]
    pub timing_receipt: Option<PathBuf>,
}

// -----------------------------------------------------------------------------
// Profile file (TOML)
// -----------------------------------------------------------------------------
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ProfileFile {
    #[serde(default)]
    pub loadgen: LoadgenProfile,
    #[serde(default)]
    pub hotswap: HotswapProfile,
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
    pub burst_multiplier: Option<u32>,
    pub burst_on_secs: Option<u64>,
    pub burst_cycle_secs: Option<u64>,
    pub ramp_start_rate: Option<u32>,
    pub ramp_step_rate: Option<u32>,
    pub ramp_step_interval_secs: Option<u64>,
    pub ramp_max_rate: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct HotswapProfile {
    /// Seconds after publish start when the hot-swap POST fires.
    pub trigger_after_secs: Option<f64>,
    /// New plugin identifier — this field previously held a plugin *name*
    /// (e.g. `pass-through-v2`); we also accept a filesystem path.
    pub new_plugin: Option<String>,
    /// Explicit `.wasm` path (preferred over `new_plugin` for P0.3).
    pub new_plugin_path: Option<String>,
    /// Runtime node to target.
    pub target_node: Option<String>,
    /// Base URL of the runtime API.
    pub api_url: Option<String>,
}

impl PublishArgs {
    /// Fold a profile-file TOML into `self`. CLI-explicit values (i.e. those
    /// that differ from the compile-time default) beat profile values.
    ///
    /// # Errors
    /// TOML parse or unknown `payload_template` returns an error.
    pub fn apply_profile_file(&mut self) -> anyhow::Result<()> {
        let Some(path) = self.profile_file.clone() else {
            return Ok(());
        };
        let text = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("read profile {}: {e}", path.display()))?;
        let cfg: ProfileFile = toml::from_str(&text)
            .map_err(|e| anyhow::anyhow!("parse profile {}: {e}", path.display()))?;
        self.apply_loadgen_profile(cfg.loadgen)?;
        self.apply_hotswap_profile(cfg.hotswap);
        Ok(())
    }

    fn apply_loadgen_profile(&mut self, profile: LoadgenProfile) -> anyhow::Result<()> {
        if self.broker_host == "localhost"
            && let Some(value) = profile.broker_host
        {
            self.broker_host = value;
        }
        if self.broker_port == 1883
            && let Some(value) = profile.broker_port
        {
            self.broker_port = value;
        }
        if self.topic == "wafer/bench/input"
            && let Some(value) = profile.topic
        {
            self.topic = value;
        }
        if self.rate == 1000
            && let Some(value) = profile.rate
        {
            self.rate = value;
        }
        if self.duration_secs == 60
            && let Some(value) = profile.duration_secs
        {
            self.duration_secs = value;
        }
        if self.payload_size == 128
            && let Some(value) = profile.payload_size
        {
            self.payload_size = value;
        }
        if self.profile == "steady"
            && let Some(value) = profile.profile
        {
            self.profile = value;
        }
        if self.client_id == "wafer-loadgen-pub"
            && let Some(value) = profile.client_id
        {
            self.client_id = value;
        }
        if self.payload_template.is_none()
            && let Some(name) = profile.payload_template
        {
            self.payload_template =
                Some(name.parse::<PayloadTemplate>().map_err(|e| anyhow::anyhow!("{e}"))?);
        }
        if self.burst_multiplier == 2
            && let Some(value) = profile.burst_multiplier
        {
            self.burst_multiplier = value;
        }
        if self.burst_on_secs == 10
            && let Some(value) = profile.burst_on_secs
        {
            self.burst_on_secs = value;
        }
        if self.burst_cycle_secs == 60
            && let Some(value) = profile.burst_cycle_secs
        {
            self.burst_cycle_secs = value;
        }
        if self.ramp_start_rate == 100
            && let Some(value) = profile.ramp_start_rate
        {
            self.ramp_start_rate = value;
        }
        if self.ramp_step_rate == 100
            && let Some(value) = profile.ramp_step_rate
        {
            self.ramp_step_rate = value;
        }
        if self.ramp_step_interval_secs == 10
            && let Some(value) = profile.ramp_step_interval_secs
        {
            self.ramp_step_interval_secs = value;
        }
        if self.ramp_max_rate == 10_000
            && let Some(value) = profile.ramp_max_rate
        {
            self.ramp_max_rate = value;
        }
        Ok(())
    }

    fn apply_hotswap_profile(&mut self, profile: HotswapProfile) {
        if self.hotswap_target_node.is_none() {
            self.hotswap_target_node = profile.target_node;
        }
        if self.hotswap_wasm_path.is_none() {
            self.hotswap_wasm_path =
                profile.new_plugin_path.or(profile.new_plugin).map(PathBuf::from);
        }
        if (self.hotswap_swap_at_secs - 30.0).abs() < f64::EPSILON
            && let Some(value) = profile.trigger_after_secs
        {
            self.hotswap_swap_at_secs = value;
        }
        if self.hotswap_api_url == "http://localhost:9090"
            && let Some(value) = profile.api_url
        {
            self.hotswap_api_url = value;
        }
    }

    /// Build the `LoadShape` from the resolved args. Fails if the profile
    /// name is unknown or hotswap-trigger is missing required fields.
    ///
    /// # Errors
    /// Unknown `profile` or missing hotswap fields.
    pub fn resolve_shape(&self) -> anyhow::Result<LoadShape> {
        if self.rate == 0 {
            anyhow::bail!("--rate must be positive");
        }
        match self.profile.as_str() {
            "steady" => Ok(LoadShape::Steady { rate: self.rate.max(1) }),
            "burst" => Ok(LoadShape::Burst {
                base_rate: self.rate.max(1),
                multiplier: self.burst_multiplier.max(1),
                on_secs: self.burst_on_secs,
                cycle_secs: self.burst_cycle_secs.max(1),
            }),
            "ramp" => Ok(LoadShape::Ramp {
                start_rate: self.ramp_start_rate.max(1),
                step_rate: self.ramp_step_rate,
                step_interval_secs: self.ramp_step_interval_secs.max(1),
                max_rate: self.ramp_max_rate.max(self.ramp_start_rate),
            }),
            "hotswap-trigger" => {
                let target_node = self.hotswap_target_node.clone().ok_or_else(|| {
                    anyhow::anyhow!("--hotswap-target-node required for hotswap-trigger profile")
                })?;
                let wasm_path = self.hotswap_wasm_path.clone().ok_or_else(|| {
                    anyhow::anyhow!("--hotswap-wasm-path required for hotswap-trigger profile")
                })?;
                if !(self.hotswap_swap_at_secs.is_finite() && self.hotswap_swap_at_secs >= 0.0) {
                    anyhow::bail!(
                        "--hotswap-swap-at-secs must be a finite, non-negative number of seconds"
                    );
                }
                // A swap scheduled at or after the end of the run never fires.
                let duration = Duration::from_secs(self.duration_secs).as_secs_f64();
                if self.hotswap_swap_at_secs >= duration {
                    anyhow::bail!(
                        "--hotswap-swap-at-secs ({}) must be less than --duration-secs ({})",
                        self.hotswap_swap_at_secs,
                        self.duration_secs
                    );
                }
                Ok(LoadShape::HotswapTrigger {
                    base_rate: self.rate.max(1),
                    swap_at_secs: self.hotswap_swap_at_secs,
                    target_node,
                    wasm_path,
                    api_url: self.hotswap_api_url.clone(),
                })
            }
            other => Err(anyhow::anyhow!(
                "unknown profile `{other}`; expected: steady, burst, ramp, hotswap-trigger"
            )),
        }
    }

    /// Human-readable summary for `--dry-run`. Renders template fingerprint
    /// and expanded profile knobs so an operator can eyeball reproducibility.
    #[must_use]
    pub fn dry_run_report(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let _r = writeln!(out, "wafer-loadgen publish (dry-run)");
        let _r = writeln!(out, "  broker         = {}:{}", self.broker_host, self.broker_port);
        let _r = writeln!(out, "  topic          = {}", self.topic);
        let _r = writeln!(out, "  base_rate      = {} msg/s", self.rate);
        let _r = writeln!(out, "  duration_secs  = {}", self.duration_secs);
        let _r = writeln!(out, "  profile        = {}", self.profile);
        let _r = writeln!(out, "  sequence_start = {}", self.sequence_start);
        let _r = writeln!(out, "  drop_when_full = {}", self.drop_when_full);
        match self.profile.as_str() {
            "burst" => {
                let _r = writeln!(
                    out,
                    "    burst_multiplier={} burst_on_secs={} burst_cycle_secs={}",
                    self.burst_multiplier, self.burst_on_secs, self.burst_cycle_secs,
                );
            }
            "ramp" => {
                let _r = writeln!(
                    out,
                    "    ramp_start_rate={} ramp_step_rate={} ramp_step_interval_secs={} ramp_max_rate={}",
                    self.ramp_start_rate,
                    self.ramp_step_rate,
                    self.ramp_step_interval_secs,
                    self.ramp_max_rate,
                );
            }
            "hotswap-trigger" => {
                let _r = writeln!(
                    out,
                    "    target_node={:?} wasm_path={:?} swap_at_secs={} api_url={}",
                    self.hotswap_target_node,
                    self.hotswap_wasm_path,
                    self.hotswap_swap_at_secs,
                    self.hotswap_api_url,
                );
            }
            _ => {}
        }
        if let Some(tpl) = self.payload_template {
            let bytes = tpl.render(crate::payload::CANONICAL_TS_NS, crate::payload::CANONICAL_SEQ);
            let _r = writeln!(
                out,
                "  payload        = template `{}` ({} bytes at canonical ts/seq)",
                tpl.name(),
                bytes.len()
            );
            let _r = writeln!(out, "  fingerprint    = sha256:{}", tpl.fingerprint_hex());
        } else {
            let _r =
                writeln!(out, "  payload        = size {} (legacy 'x' filler)", self.payload_size);
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

fn write_timing_receipt(path: &std::path::Path, measurement_started_ns: u64) -> anyhow::Result<()> {
    const EVENT_OFFSET_NS: u64 = 60_000_000_000;
    let receipt = PublisherTimingReceipt {
        schema_version: 1,
        measurement_clock: "monotonic".into(),
        alignment_clock: "unix-epoch".into(),
        measurement_started_unix_epoch_ns: measurement_started_ns,
        event_unix_epoch_ns: measurement_started_ns.saturating_add(EVENT_OFFSET_NS),
        event_offset_ns: EVENT_OFFSET_NS,
    };
    write_atomic(path, format!("{}\n", serde_json::to_string_pretty(&receipt)?).as_bytes())?;
    Ok(())
}

/// Spawn the hot-swap trigger task if the shape asks for it.
///
/// Returns the [`JoinHandle`] so the caller can `.abort()` on shutdown. The task
/// builds its HTTP client, sleeps `swap_at_secs` from `start`, then POSTs the
/// wasm path to `{api_url}/api/v1/nodes/{target_node}/hot-swap`.
///
/// When `result_path` is set the artifact is written whether the request
/// succeeded or not (a failed request records `error` and, if a response
/// arrived, its `http_status`). The task returns `Err` on a transport error
/// or a non-2xx response so the publisher exits non-zero: an E-Swap run in
/// which no swap happened must not look like a valid run.
///
/// [`JoinHandle`]: tokio::task::JoinHandle
fn spawn_hotswap_trigger(
    shape: &LoadShape,
    start: Instant,
    result_path: Option<PathBuf>,
) -> Option<tokio::task::JoinHandle<anyhow::Result<()>>> {
    let LoadShape::HotswapTrigger { swap_at_secs, target_node, wasm_path, api_url, .. } = shape
    else {
        return None;
    };
    let swap_at = *swap_at_secs;
    let target_node = target_node.clone();
    let wasm_path = wasm_path.clone();
    let api_url = api_url.clone();
    Some(tokio::spawn(async move {
        // Built before the wait so its construction cost does not delay the POST.
        let client = reqwest::Client::builder().timeout(Duration::from_secs(30)).build()?;
        let target = Duration::try_from_secs_f64(swap_at)
            .ok()
            .and_then(|offset| start.checked_add(offset))
            .unwrap_or(start);
        tokio::time::sleep_until(target).await;
        let url = format!("{api_url}/api/v1/nodes/{target_node}/hot-swap");
        let body = serde_json::json!({ "wasm_path": wasm_path });
        info!(
            "Firing hot-swap POST to {url} (target elapsed = {swap_at:.3}s; actual delta = {:.3}s)",
            start.elapsed().as_secs_f64()
        );
        let request_started_ns = now_ns();
        let request_started_monotonic = Instant::now();
        let outcome = post_hotswap(&client, &url, &body).await;
        let request_duration_ns =
            u64::try_from(request_started_monotonic.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let request_finished_ns = now_ns();
        let (http_status, response_body, failure) = match outcome {
            Ok((status, response_body)) => {
                info!("hot-swap POST {} => HTTP {}", url, status);
                let failure = (!status.is_success())
                    .then(|| format!("hot-swap POST returned HTTP {status}: {response_body}"));
                (Some(status.as_u16()), Some(response_body), failure)
            }
            Err(e) => (None, None, Some(format!("hot-swap POST to {url} failed: {e:#}"))),
        };
        if let Some(path) = result_path {
            let body = response_body.as_ref().map(|text| {
                serde_json::from_str(text)
                    .unwrap_or_else(|_| serde_json::Value::String(text.clone()))
            });
            let artifact = serde_json::json!({
                "requests": [{
                    "event_index": 0,
                    "plugin": wasm_path.file_name().and_then(|name| name.to_str()),
                    "request_started_ns": request_started_ns,
                    "request_finished_ns": request_finished_ns,
                    "request_timestamp_clock": "unix-epoch",
                    "request_duration_ns": request_duration_ns,
                    "request_duration_clock": "monotonic",
                    "http_status": http_status,
                    "body": body,
                    "error": failure,
                }]
            });
            tokio::fs::write(path, format!("{}\n", serde_json::to_string_pretty(&artifact)?))
                .await?;
        }
        failure.map_or(Ok(()), |message| Err(anyhow::anyhow!(message)))
    }))
}

async fn post_hotswap(
    client: &reqwest::Client,
    url: &str,
    body: &serde_json::Value,
) -> anyhow::Result<(reqwest::StatusCode, String)> {
    let response = client.post(url).json(body).send().await?;
    let status = response.status();
    Ok((status, response.text().await?))
}

async fn enqueue_publish(
    client: &AsyncClient,
    topic: &str,
    payload: Vec<u8>,
    drop_when_full: bool,
) -> Result<(), rumqttc::ClientError> {
    if drop_when_full {
        client.try_publish(topic, QoS::AtLeastOnce, false, payload)
    } else {
        client.publish(topic, QoS::AtLeastOnce, false, payload).await
    }
}

/// Drive the publisher until `--duration-secs * --rate` messages have been sent.
///
/// # Errors
/// - MQTT connection setup failure.
/// - The hot-swap trigger failed (client build, transport error, non-2xx
///   response, or task panic). The summary file is still written first.
///
/// Note: MQTT publishes queued to rumqttc's internal channel succeed even if
/// the broker is unreachable; the eventloop task logs the connection failure
/// separately. This is intentional — the eval scripts fail on broker
/// unavailability at a higher layer via the subscriber's `sequence.csv`.
#[expect(
    clippy::too_many_lines,
    reason = "publisher orchestration keeps MQTT setup, shape resolution, publish loop, and hotswap trigger cleanup in one place; splitting hurts readability more than it helps"
)]
pub async fn run_publisher(mut args: PublishArgs) -> anyhow::Result<PublisherReport> {
    args.apply_profile_file()?;

    if args.dry_run {
        info!(target: "wafer_loadgen::publish::dry_run", "{}", args.dry_run_report());
        return Ok(PublisherReport {
            schema_version: 1,
            published: 0,
            errors: 0,
            intended: 0,
            rejected: 0,
            enqueued: 0,
            measurement_duration_ns: 0,
            deadline_misses: 0,
            elapsed_ms: 0,
            actual_rate: 0.0,
            hotswap_triggered_at_secs: None,
            exit_reason: "dry-run".into(),
            source_lag_ns: LagSummary::default(),
        });
    }

    let shape = args.resolve_shape()?;
    let mut stop_signal = StopSignal::install()?;

    info!(
        broker = %args.broker_host,
        port = args.broker_port,
        topic = %args.topic,
        base_rate = shape.base_rate(),
        duration = args.duration_secs,
        profile = %args.profile,
        "Starting WAFER load generator (publisher)"
    );

    let mut opts = MqttOptions::new(&args.client_id, &args.broker_host, args.broker_port);
    opts.set_keep_alive(Duration::from_secs(30));
    opts.set_clean_session(true);
    let (client, mut eventloop) = AsyncClient::new(opts, 1024);
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

    // Brief delay for the MQTT session to establish before we start hammering.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let start = Instant::now();
    let measurement_started_ns = now_ns();
    if let Some(path) = &args.timing_receipt {
        write_timing_receipt(path, measurement_started_ns)?;
    }
    let hotswap_target = match &shape {
        LoadShape::HotswapTrigger { swap_at_secs, .. }
            if *swap_at_secs < Duration::from_secs(args.duration_secs).as_secs_f64() =>
        {
            Some(*swap_at_secs)
        }
        _ => None,
    };
    let hotswap_task = hotswap_target
        .and_then(|_| spawn_hotswap_trigger(&shape, start, args.hotswap_result_path.clone()));

    let padding: String = "x".repeat(args.payload_size.saturating_sub(80));
    let payload_template = args.payload_template;
    let mut trace =
        args.trace_file.as_ref().map(std::fs::File::create).transpose()?.map(BufWriter::new);
    if let Some(trace) = &mut trace {
        writeln!(trace, "seq,ts_ns")?;
    }
    let mut due =
        pace(Scheduler::new(shape), start.into_std(), Duration::from_secs(args.duration_secs))?;
    let mut source_lag = lag_histogram();
    let mut seq = args.sequence_start;
    let mut offered: u64 = 0;
    let mut errors: u64 = 0;
    let mut deadline_misses: u64 = 0;
    let mut exit_reason = "duration";

    loop {
        let (offset, next_offset) = tokio::select! {
            biased;
            reason = stop_signal.recv() => {
                info!(signal = reason, "Stop signal received; writing summary");
                exit_reason = reason;
                break;
            }
            tick = due.recv() => match tick {
                Some(tick) => tick,
                None => break,
            },
        };

        let ts = measurement_started_ns.saturating_add(duration_ns(offset));
        let payload_vec: Vec<u8> = payload_template.map_or_else(
            || format!(
                r#"{{"ts":{ts},"seq":{seq},"device_id":"bench","temperature":72.5,"pad":"{padding}"}}"#
            )
            .into_bytes(),
            |tpl| tpl.render(ts, seq),
        );
        let enqueued = tokio::select! {
            biased;
            reason = stop_signal.recv() => {
                info!(signal = reason, "Stop signal received; writing summary");
                exit_reason = reason;
                break;
            }
            result = enqueue_publish(&client, &args.topic, payload_vec, args.drop_when_full) => result,
        };
        if enqueued.is_ok() {
            record_lag(&mut source_lag, duration_ns(start.elapsed().saturating_sub(offset)));
        }
        if let Some(trace) = &mut trace {
            writeln!(trace, "{seq},{ts}")?;
        }
        if let Err(error) = enqueued {
            if !args.drop_when_full {
                warn!("Publish error (seq={seq}): {error}");
            }
            errors = errors.saturating_add(1);
        }
        if start.elapsed() >= next_offset {
            deadline_misses = deadline_misses.saturating_add(1);
        }

        seq = seq.saturating_add(1);
        offered = offered.saturating_add(1);
    }
    drop(due);

    if let Some(trace) = &mut trace {
        trace.flush()?;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let _disc = client.disconnect().await;
    eventloop_task.abort();
    // A failed swap fails the run, but only after the summary is written so
    // the evidence of what was offered survives.
    let hotswap_failure = match hotswap_task {
        Some(t) if exit_reason != "duration" => {
            t.abort();
            None
        }
        Some(t) => match t.await {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(e),
            Err(join) => Some(anyhow::anyhow!("hot-swap trigger task panicked: {join}")),
        },
        None => None,
    };
    if let Some(e) = &hotswap_failure {
        error!("hot-swap trigger failed: {e:#}");
    }

    let elapsed = start.elapsed();
    let actual_rate = if elapsed.as_secs_f64() > 0.0 {
        #[expect(
            clippy::cast_precision_loss,
            clippy::as_conversions,
            reason = "offered count is bounded by rate times duration and fits in f64 mantissa for any realistic run"
        )]
        let rate = offered as f64 / elapsed.as_secs_f64();
        rate
    } else {
        0.0
    };

    info!(
        total = offered,
        errors = errors,
        elapsed_ms = elapsed.as_millis(),
        actual_rate = format!("{actual_rate:.1}"),
        "Load generation complete"
    );

    let report = PublisherReport {
        schema_version: 1,
        published: offered,
        errors,
        intended: offered,
        rejected: errors,
        enqueued: offered.saturating_sub(errors),
        measurement_duration_ns: args.duration_secs.saturating_mul(1_000_000_000),
        deadline_misses,
        elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        actual_rate,
        hotswap_triggered_at_secs: hotswap_target.filter(|_| hotswap_failure.is_none()),
        exit_reason: exit_reason.to_owned(),
        source_lag_ns: LagSummary::from(&source_lag),
    };
    if let Some(path) = args.summary_file {
        write_atomic(&path, format!("{}\n", serde_json::to_string_pretty(&report)?).as_bytes())?;
    }
    if let Some(e) = hotswap_failure {
        return Err(e.context("hot-swap trigger failed; the run did not swap"));
    }
    Ok(report)
}

/// Post-run summary returned by `run_publisher`.
#[derive(Debug, Clone, Serialize)]
pub struct PublisherReport {
    pub schema_version: u8,
    pub published: u64,
    pub errors: u64,
    pub intended: u64,
    pub rejected: u64,
    pub enqueued: u64,
    pub measurement_duration_ns: u64,
    pub deadline_misses: u64,
    pub elapsed_ms: u64,
    pub actual_rate: f64,
    /// If profile = hotswap-trigger, the offset (secs) at which the swap was
    /// triggered. `None` for other profiles and when the swap request failed.
    pub hotswap_triggered_at_secs: Option<f64>,
    /// `duration` when the schedule ran to its end, or the signal that stopped it.
    pub exit_reason: String,
    /// How late each message was handed to the MQTT client relative to its
    /// scheduled time. Latency is measured from the scheduled time, so a run
    /// whose lag approaches its latency is limited by the publisher.
    pub source_lag_ns: LagSummary,
}

/// Percentiles of a nanosecond histogram. All zero when nothing was recorded.
#[derive(Debug, Clone, Default, Serialize)]
pub struct LagSummary {
    pub count: u64,
    pub p50: u64,
    pub p99: u64,
    pub p999: u64,
    pub max: u64,
}

impl From<&Histogram<u64>> for LagSummary {
    fn from(histogram: &Histogram<u64>) -> Self {
        Self {
            count: histogram.len(),
            p50: histogram.value_at_quantile(0.50),
            p99: histogram.value_at_quantile(0.99),
            p999: histogram.value_at_quantile(0.999),
            max: histogram.max(),
        }
    }
}

/// How far the pacer may run ahead of a publisher blocked on the client queue.
/// Any value works: target times come from the schedule, not from when the
/// publisher picks the tick up.
const PACER_AHEAD: usize = 64;

/// Sends `(offset, next_offset)` for every scheduled publish before `duration`,
/// each at its offset from `origin`, then closes the channel.
fn pace(
    mut scheduler: Scheduler,
    origin: std::time::Instant,
    duration: Duration,
) -> std::io::Result<tokio::sync::mpsc::Receiver<(Duration, Duration)>> {
    let (tx, rx) = tokio::sync::mpsc::channel(PACER_AHEAD);
    std::thread::Builder::new().name("loadgen-pacer".to_owned()).spawn(move || {
        loop {
            let offset = scheduler.next_offset();
            if offset >= duration {
                return;
            }
            if let Some(wait) = origin
                .checked_add(offset)
                .and_then(|due| due.checked_duration_since(std::time::Instant::now()))
            {
                std::thread::sleep(wait);
            }
            if tx.blocking_send((offset, scheduler.peek())).is_err() {
                return;
            }
        }
    })?;
    Ok(rx)
}

#[expect(
    clippy::expect_used,
    reason = "constant bounds (1 ns to 1 h, 3 significant digits) are valid"
)]
fn lag_histogram() -> Histogram<u64> {
    Histogram::new_with_bounds(1, 3_600_000_000_000, 3).expect("valid histogram bounds")
}

#[expect(
    clippy::let_underscore_must_use,
    reason = "a lag above the 1 h upper bound is not a plausible run and is dropped"
)]
fn record_lag(histogram: &mut Histogram<u64>, lag_ns: u64) {
    let _ = histogram.record(lag_ns.max(1));
}

fn duration_ns(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use rumqttc::{AsyncClient, MqttOptions};

    use super::{PublishArgs, enqueue_publish, pace};
    use crate::profile::{LoadShape, Scheduler};

    #[tokio::test]
    async fn pacer_keeps_scheduled_offsets_when_the_publisher_falls_behind() {
        let origin = std::time::Instant::now();
        let mut due = pace(
            Scheduler::new(LoadShape::Steady { rate: 100 }),
            origin,
            Duration::from_millis(50),
        )
        .unwrap();

        let (first, next) = due.recv().await.unwrap();
        assert_eq!((first, next), (Duration::ZERO, Duration::from_millis(10)));
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut late = Vec::new();
        while let Some((offset, _)) = due.recv().await {
            late.push(offset);
        }

        assert_eq!(late, [10, 20, 30, 40].map(Duration::from_millis));
    }

    fn hotswap_args(api_url: String, result_path: std::path::PathBuf) -> PublishArgs {
        PublishArgs {
            broker_host: "127.0.0.1".into(),
            broker_port: 1,
            topic: "test/topic".into(),
            rate: 10,
            duration_secs: 1,
            payload_size: 128,
            payload_template: None,
            profile: "hotswap-trigger".into(),
            client_id: "hotswap-test".into(),
            profile_file: None,
            dry_run: false,
            burst_multiplier: 2,
            burst_on_secs: 1,
            burst_cycle_secs: 2,
            ramp_start_rate: 1,
            ramp_step_rate: 1,
            ramp_step_interval_secs: 1,
            ramp_max_rate: 2,
            hotswap_target_node: Some("t1".into()),
            hotswap_wasm_path: Some("plugins/v2.wasm".into()),
            hotswap_swap_at_secs: 0.1,
            hotswap_api_url: api_url,
            trace_file: None,
            summary_file: None,
            sequence_start: 0,
            drop_when_full: true,
            hotswap_result_path: Some(result_path),
            timing_receipt: None,
        }
    }

    #[tokio::test]
    async fn failed_hotswap_request_fails_the_run_and_records_the_error() {
        // Reserve a port, then close it so the POST is refused.
        let closed = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let api_url = format!("http://{}", closed.local_addr().expect("addr"));
        drop(closed);
        let dir = tempfile::tempdir().expect("tempdir");
        let result_path = dir.path().join("swap_timeline.json");

        let err = super::run_publisher(hotswap_args(api_url, result_path.clone()))
            .await
            .expect_err("a swap that never happened must fail the publisher");

        assert!(format!("{err:#}").contains("hot-swap"), "{err:#}");
        let artifact: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&result_path).expect("artifact written"))
                .expect("parse artifact");
        let request = &artifact["requests"][0];
        assert!(request["http_status"].is_null());
        assert!(request["error"].as_str().is_some_and(|e| e.contains("failed")), "{artifact}");
    }

    #[test]
    fn rejects_negative_or_non_finite_swap_offsets() {
        // hotswap_args runs for 1 s, so 1.0 and later would never fire.
        for bad in [-1.0, f64::NAN, f64::INFINITY, 1.0, 45.0] {
            let mut args = hotswap_args("http://localhost".into(), "unused.json".into());
            args.hotswap_swap_at_secs = bad;
            assert!(args.resolve_shape().is_err(), "{bad} must be rejected");
        }
    }

    #[test]
    fn rejects_zero_rate() {
        let args = PublishArgs {
            broker_host: "localhost".into(),
            broker_port: 1883,
            topic: "test/topic".into(),
            rate: 0,
            duration_secs: 1,
            payload_size: 128,
            payload_template: None,
            profile: "steady".into(),
            client_id: "test".into(),
            profile_file: None,
            dry_run: false,
            burst_multiplier: 2,
            burst_on_secs: 1,
            burst_cycle_secs: 2,
            ramp_start_rate: 1,
            ramp_step_rate: 1,
            ramp_step_interval_secs: 1,
            ramp_max_rate: 2,
            hotswap_target_node: None,
            hotswap_wasm_path: None,
            hotswap_swap_at_secs: 1.0,
            hotswap_api_url: "http://localhost".into(),
            trace_file: None,
            summary_file: None,
            sequence_start: 0,
            drop_when_full: false,
            hotswap_result_path: None,
            timing_receipt: None,
        };
        assert!(args.resolve_shape().is_err(), "zero is not a positive scout rate");
    }

    #[tokio::test]
    async fn drop_when_full_never_waits_for_publish_queue_capacity() {
        let options = MqttOptions::new("queue-test", "127.0.0.1", 1883);
        let (client, _eventloop) = AsyncClient::new(options, 1);

        enqueue_publish(&client, "test/topic", vec![1], true)
            .await
            .expect("first publish should fill the queue");
        let result = tokio::time::timeout(
            Duration::from_millis(50),
            enqueue_publish(&client, "test/topic", vec![2], true),
        )
        .await
        .expect("open-loop publish must not wait for queue capacity");

        assert!(result.is_err(), "full queue must reject the offered message");
    }
}
