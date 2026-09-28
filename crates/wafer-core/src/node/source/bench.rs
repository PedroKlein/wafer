//! Rate-controlled benchmark source for evaluation experiments.
//!
//! `BenchSource` emits messages on an open-loop schedule fixed when the first
//! message is requested. Each message carries its scheduled time
//! (`intended_ns`) and the time it actually left the source (`emit_ns`), so
//! a stalled pipeline shows up as latency instead of silently delaying the
//! schedule (coordinated omission, Tene 2012). Each message also carries a
//! monotonic sequence number for gap/duplicate detection at the sink. These
//! travel as host-only [`BenchStamps`], so Wasm guests never see or copy them.
//!
//! See docs/rfcs/RFC-008-evaluation-harness.md — Session 8 D3A.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::{Duration, Instant};

use bytes::Bytes;

use crate::error::Result;
use crate::node::{Lifecycle, Source};
use crate::queue::{BenchStamps, BurstPhase, BurstStamps, RuntimeEnvelope};
use crate::util::monotonic_unix_ns;

/// Configuration for a benchmark source node.
#[derive(Debug, Clone)]
pub struct BenchSourceConfig {
    /// Messages per second emission rate.
    pub rate_per_sec: f64,
    /// Total messages to emit (including warmup).
    pub total_messages: u64,
    /// Messages to emit before measurement begins (sink-side warmup).
    pub warmup_messages: u64,
    /// Payload size in bytes (filled with 0x42 pattern).
    pub payload_size: usize,
    burst: Option<BenchBurstSchedule>,
    evidence_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy)]
pub struct BenchBurstSchedule {
    rate_per_sec: f64,
    start_secs: u64,
    end_secs: u64,
}

impl BenchBurstSchedule {
    #[must_use]
    pub const fn new(rate_per_sec: f64, start_secs: u64, end_secs: u64) -> Self {
        Self { rate_per_sec, start_secs, end_secs }
    }
}

impl BenchSourceConfig {
    fn burst_phase_counts(&self) -> Option<[u64; 3]> {
        let burst = self.burst?;
        let before = messages_for(self.rate_per_sec, burst.start_secs)?;
        let during =
            messages_for(burst.rate_per_sec, burst.end_secs.saturating_sub(burst.start_secs))?;
        let measured = self.total_messages.checked_sub(self.warmup_messages)?;
        let after = measured.checked_sub(before.saturating_add(during))?;
        Some([before, during, after])
    }

    fn measurement_offset_ns(&self, sequence: u64) -> Option<u64> {
        let burst = self.burst?;
        let measurement_index = sequence.checked_sub(self.warmup_messages)?;
        let [before, during, _] = self.burst_phase_counts()?;
        let base_interval = interval_ns(self.rate_per_sec)?;
        let burst_interval = interval_ns(burst.rate_per_sec)?;
        if measurement_index < before {
            Some(measurement_index.saturating_mul(base_interval))
        } else if measurement_index < before.saturating_add(during) {
            Some(burst.start_secs.saturating_mul(1_000_000_000).saturating_add(
                measurement_index.saturating_sub(before).saturating_mul(burst_interval),
            ))
        } else {
            Some(
                burst.end_secs.saturating_mul(1_000_000_000).saturating_add(
                    measurement_index
                        .saturating_sub(before.saturating_add(during))
                        .saturating_mul(base_interval),
                ),
            )
        }
    }

    fn schedule_offset_ns(&self, sequence: u64) -> u64 {
        let base_interval = interval_ns(self.rate_per_sec).unwrap_or(0);
        self.measurement_offset_ns(sequence).map_or_else(
            || sequence.saturating_mul(base_interval),
            |offset| self.warmup_messages.saturating_mul(base_interval).saturating_add(offset),
        )
    }

    fn burst_phase(&self, sequence: u64) -> Option<BurstPhase> {
        let measurement_index = sequence.checked_sub(self.warmup_messages)?;
        let [before, during, _] = self.burst_phase_counts()?;
        Some(if measurement_index < before {
            BurstPhase::Before
        } else if measurement_index < before.saturating_add(during) {
            BurstPhase::Burst
        } else {
            BurstPhase::After
        })
    }

    /// Create a new config with required parameters.
    #[must_use]
    pub const fn new(rate_per_sec: f64, total_messages: u64) -> Self {
        Self {
            rate_per_sec,
            total_messages,
            warmup_messages: 0,
            payload_size: 128,
            burst: None,
            evidence_dir: None,
        }
    }

    /// Set the number of warmup messages.
    #[must_use]
    pub const fn with_warmup(mut self, warmup_messages: u64) -> Self {
        self.warmup_messages = warmup_messages;
        self
    }

    /// Set the payload size in bytes.
    #[must_use]
    pub const fn with_payload_size(mut self, size: usize) -> Self {
        self.payload_size = size;
        self
    }

    #[must_use]
    pub const fn with_burst(mut self, burst: BenchBurstSchedule) -> Self {
        self.burst = Some(burst);
        self
    }

    #[must_use]
    pub fn with_evidence_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.evidence_dir = dir;
        self
    }
}

/// Rate-controlled source that emits sequenced, timestamped messages.
///
/// Open-loop: message `n` is due at `origin + offset(n)`, where the origin is
/// taken once, on the first poll. A message that leaves late still carries its
/// due time, so time the source spent blocked on a full queue counts as
/// latency.
///
/// Due times are paced by a dedicated OS thread rather than the Tokio timer,
/// which rounds every deadline up to the next millisecond and would add up to
/// 1 ms of source-side lag to each message.
pub struct BenchSource {
    id: String,
    config: BenchSourceConfig,
    sequence: u64,
    pacer: Option<Pacer>,
    measurement_start_unix_ns: Option<u64>,
    measurement_completed_offset_ns: Option<u64>,
    payload: Bytes,
    emitted_phase_counts: [u64; 3],
}

impl BenchSource {
    /// Create a new BenchSource from configuration.
    #[must_use]
    pub fn new(config: BenchSourceConfig) -> Self {
        let payload = Bytes::from(vec![0x42u8; config.payload_size]);
        Self {
            id: "bench-source".to_owned(),
            config,
            sequence: 0,
            pacer: None,
            measurement_start_unix_ns: None,
            measurement_completed_offset_ns: None,
            payload,
            emitted_phase_counts: [0; 3],
        }
    }

    /// Create with a custom node ID.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    fn start_pacer(&mut self) -> Result<()> {
        let pacer = Pacer::start(self.config.clone())?;
        let measurement_start_unix_ns = pacer
            .origin_unix_ns
            .saturating_add(self.config.schedule_offset_ns(self.config.warmup_messages));
        self.pacer = Some(pacer);
        if self.config.burst.is_some() {
            self.measurement_start_unix_ns = Some(measurement_start_unix_ns);
            self.write_burst_timing(measurement_start_unix_ns)?;
        }
        Ok(())
    }

    async fn next_due(&mut self) -> Result<(u64, u64)> {
        if self.pacer.is_none() {
            self.start_pacer()?;
        }
        let stopped = || std::io::Error::other("bench source pacer stopped early");
        let pacer = self.pacer.as_mut().ok_or_else(stopped)?;
        let (sequence, offset_ns) = pacer.due.recv().await.ok_or_else(stopped)?;
        Ok((sequence, pacer.origin_unix_ns.saturating_add(offset_ns)))
    }

    fn write_burst_timing(&self, measurement_start_ns: u64) -> std::io::Result<()> {
        let Some(dir) = &self.config.evidence_dir else { return Ok(()) };
        let Some(burst) = self.config.burst else { return Ok(()) };
        std::fs::create_dir_all(dir)?;
        let value = serde_json::json!({
            "schema_version": 1,
            "timestamp_clock": "unix-epoch",
            "timestamp_clock_purpose": "cross-process-alignment",
            "scheduling_clock": "monotonic",
            "measurement_start_ns": measurement_start_ns,
            "burst_start_ns": measurement_start_ns.saturating_add(burst.start_secs.saturating_mul(1_000_000_000)),
            "scheduled_swap_ns": measurement_start_ns.saturating_add(60_000_000_000),
            "burst_end_ns": measurement_start_ns.saturating_add(burst.end_secs.saturating_mul(1_000_000_000)),
            "scheduled_measurement_end_ns": measurement_start_ns.saturating_add(120_000_000_000),
        });
        write_json_atomic(&dir.join("burst-source-timing.json"), &value)
    }

    fn write_burst_summary(&self) -> std::io::Result<()> {
        let Some(dir) = &self.config.evidence_dir else { return Ok(()) };
        let Some(burst) = self.config.burst else { return Ok(()) };
        let Some(intended) = self.config.burst_phase_counts() else { return Ok(()) };
        let measurement_start_ns = self.measurement_start_unix_ns.unwrap_or(0);
        let value = serde_json::json!({
            "schema_version": 1,
            "measurement_start_ns": measurement_start_ns,
            "measurement_end_ns": monotonic_unix_ns(),
            "source_completion_offset_ns": self.measurement_completed_offset_ns.unwrap_or(u64::MAX),
            "rates_msg_s": [self.config.rate_per_sec, burst.rate_per_sec, self.config.rate_per_sec],
            "phase_offsets_ns": [0, burst.start_secs.saturating_mul(1_000_000_000), burst.end_secs.saturating_mul(1_000_000_000), 120_000_000_000_u64],
            "warmup_messages": self.config.warmup_messages,
            "intended_phase_messages": intended,
            "emitted_phase_messages": self.emitted_phase_counts,
            "intended_measurement_messages": intended.iter().sum::<u64>(),
            "emitted_measurement_messages": self.emitted_phase_counts.iter().sum::<u64>(),
            "total_emitted_messages": self.sequence,
        });
        write_json_atomic(&dir.join("burst-source-summary.json"), &value)
    }
}

/// How far the pacer may run ahead of a source that is blocked downstream.
/// Any value works: due times come from the sequence number, not from when
/// the source picks the tick up.
const PACER_AHEAD: usize = 64;

struct Pacer {
    origin: Instant,
    origin_unix_ns: u64,
    /// `(sequence, offset_ns)` of each message once it is due.
    due: tokio::sync::mpsc::Receiver<(u64, u64)>,
}

impl Pacer {
    fn start(config: BenchSourceConfig) -> std::io::Result<Self> {
        let (tx, due) = tokio::sync::mpsc::channel(PACER_AHEAD);
        let origin = Instant::now();
        let origin_unix_ns = monotonic_unix_ns();
        std::thread::Builder::new().name("bench-pacer".to_owned()).spawn(move || {
            for sequence in 0..config.total_messages {
                let offset_ns = config.schedule_offset_ns(sequence);
                let due_at = origin.checked_add(Duration::from_nanos(offset_ns)).unwrap_or(origin);
                if let Some(wait) = due_at.checked_duration_since(Instant::now()) {
                    std::thread::sleep(wait);
                }
                if tx.blocking_send((sequence, offset_ns)).is_err() {
                    return;
                }
            }
        })?;
        Ok(Self { origin, origin_unix_ns, due })
    }
}

fn interval_ns(rate_per_sec: f64) -> Option<u64> {
    if !rate_per_sec.is_finite() || rate_per_sec <= 0.0 {
        return None;
    }
    #[expect(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "validated positive finite benchmark rates produce bounded nanosecond intervals"
    )]
    let interval = (1_000_000_000.0 / rate_per_sec) as u64;
    (interval > 0).then_some(interval)
}

fn messages_for(rate_per_sec: f64, seconds: u64) -> Option<u64> {
    #[expect(
        clippy::as_conversions,
        clippy::cast_precision_loss,
        reason = "benchmark phase durations are small and exactly representable in f64"
    )]
    let messages = rate_per_sec * seconds as f64;
    if !messages.is_finite() || messages < 0.0 || messages.fract() != 0.0 {
        return None;
    }
    #[expect(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "validated integral benchmark populations are bounded by configured u64 totals"
    )]
    Some(messages as u64)
}

fn write_json_atomic(path: &Path, value: &serde_json::Value) -> std::io::Result<()> {
    crate::util::write_atomic(
        path,
        format!("{}\n", serde_json::to_string_pretty(value)?).as_bytes(),
    )
}

impl From<BenchSourceConfig> for BenchSource {
    fn from(config: BenchSourceConfig) -> Self {
        Self::new(config)
    }
}

impl Lifecycle for BenchSource {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &'static str {
        "bench-source"
    }

    fn validate(&self) -> Result<()> {
        if interval_ns(self.config.rate_per_sec).is_none() {
            return Err(crate::error::ConfigError::Message(
                "bench source rate must be positive and finite".to_owned(),
            )
            .into());
        }
        if let Some(burst) = self.config.burst
            && (interval_ns(burst.rate_per_sec).is_none()
                || burst.start_secs >= burst.end_secs
                || self.config.burst_phase_counts().is_none())
        {
            return Err(crate::error::ConfigError::Message(
                "bench source burst schedule is invalid for the configured population".to_owned(),
            )
            .into());
        }
        Ok(())
    }

    fn init(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }

    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async {
            self.write_burst_summary()?;
            Ok(())
        })
    }
}

impl Source for BenchSource {
    fn poll(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<RuntimeEnvelope>>> + Send + '_>> {
        Box::pin(async {
            // Check completion
            if self.sequence >= self.config.total_messages {
                if self.measurement_completed_offset_ns.is_none()
                    && self.config.burst.is_some()
                    && let Some(pacer) = &self.pacer
                {
                    self.measurement_completed_offset_ns = Some(
                        crate::util::duration_ns_saturating(pacer.origin.elapsed()).saturating_sub(
                            self.config.schedule_offset_ns(self.config.warmup_messages),
                        ),
                    );
                }
                return Ok(None);
            }

            let (seq, intended_ns) = self.next_due().await?;
            let burst_phase = self.config.burst_phase(seq);
            self.sequence = seq.saturating_add(1);
            if let Some(count) =
                burst_phase.and_then(|phase| self.emitted_phase_counts.get_mut(phase.index()))
            {
                *count = count.saturating_add(1);
            }

            let burst = burst_phase.map(|phase| BurstStamps {
                phase,
                measurement_start_unix_ns: self.measurement_start_unix_ns,
            });
            let envelope = RuntimeEnvelope::new(self.id.clone(), self.payload.clone());
            Ok(Some(envelope.with_bench_stamps(BenchStamps {
                sequence: seq,
                intended_ns,
                emit_ns: monotonic_unix_ns(),
                warmup: seq < self.config.warmup_messages,
                measurement_start_seq: self.config.warmup_messages,
                sequence_end: Some(self.config.total_messages),
                burst,
            })))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn emits_correct_count_then_none() {
        let config = BenchSourceConfig::new(100_000.0, 5);
        let mut source = BenchSource::new(config);

        source.init().await.unwrap();

        for _ in 0..5 {
            let msg = source.poll().await.unwrap();
            assert!(msg.is_some());
        }

        // After total_messages, returns None
        let eof = source.poll().await.unwrap();
        assert!(eof.is_none());
    }

    #[tokio::test]
    async fn sequence_numbers_are_monotonic() {
        let config = BenchSourceConfig::new(100_000.0, 10);
        let mut source = BenchSource::new(config);

        source.init().await.unwrap();

        for expected_seq in 0..10u64 {
            let msg = source.poll().await.unwrap().unwrap();
            assert_eq!(msg.header.bench.unwrap().sequence, expected_seq);
        }
    }

    #[tokio::test]
    async fn stamps_stay_out_of_guest_visible_metadata() {
        let config = BenchSourceConfig::new(100_000.0, 1);
        let mut source = BenchSource::new(config);

        source.init().await.unwrap();

        let msg = source.poll().await.unwrap().unwrap();
        assert!(msg.header.metadata.is_empty(), "metadata = {:?}", msg.header.metadata);
        let stamps = msg.header.bench.unwrap();
        assert!(stamps.emit_ns >= stamps.intended_ns);
        assert_eq!(stamps.sequence_end, Some(1));
    }

    #[tokio::test]
    async fn metadata_marks_the_exact_warmup_population() {
        let config = BenchSourceConfig::new(100_000.0, 3).with_warmup(2);
        let mut source = BenchSource::new(config);
        source.init().await.unwrap();

        for expected in [true, true, false] {
            let message = source.poll().await.unwrap().unwrap();
            assert_eq!(message.header.bench.unwrap().warmup, expected);
        }
    }

    #[tokio::test]
    async fn metadata_carries_measurement_start_sequence() {
        let config = BenchSourceConfig::new(100_000.0, 3).with_warmup(2);
        let mut source = BenchSource::new(config);
        source.init().await.unwrap();

        for _ in 0..3 {
            let message = source.poll().await.unwrap().unwrap();
            assert_eq!(message.header.bench.unwrap().measurement_start_seq, 2);
        }
    }

    #[tokio::test]
    async fn burst_metadata_carries_source_measurement_origin() {
        let config = BenchSourceConfig::new(1_000.0, 4_001)
            .with_warmup(1)
            .with_burst(BenchBurstSchedule::new(2_000.0, 1, 2));
        let mut source = BenchSource::new(config);
        source.init().await.unwrap();

        let _warmup = source.poll().await.unwrap().unwrap();
        let measured = source.poll().await.unwrap().unwrap();
        let burst = measured.header.bench.unwrap().burst.unwrap();
        assert_eq!(burst.measurement_start_unix_ns, source.measurement_start_unix_ns);
    }

    #[tokio::test]
    async fn emitted_envelope_uses_configured_source_id() {
        let config = BenchSourceConfig::new(100_000.0, 1);
        let mut source = BenchSource::new(config).with_id("source-a");
        source.init().await.unwrap();

        let message = source.poll().await.unwrap().unwrap();
        assert_eq!(&*message.header.source, "source-a");
    }

    #[tokio::test]
    async fn candidate_payload_grid_has_exact_size_and_content_hash() {
        use sha2::{Digest as _, Sha256};

        let payloads = [
            (120, "c2444823dde4c40542129b562b092b693b36c59c909106d291b18a650769b418"),
            (1_024, "9b6ce55f379e9771551de6939556a7e6b949814ae27c2f5cfd5dbeb378ce7c2a"),
            (8_192, "766c00ba277e84ef9550596c7eda86bad4d66a3fee2255924e6388ee9c272792"),
            (10_240, "6dff39006bfd7895ec3ef56f233bcf5977a4cb6bd10e6aeb44d2169a773a886d"),
            (16_384, "db03474b1b90657f9fe742b4eed775e8b9000196bf262d1bd8521f8f7f3edd3f"),
            (32_768, "314a5163f130c25e1f962e1b0316d356d0702438df90c32c2d2dd16c84e551a8"),
            (65_536, "fee47b1f0d7685a226fd5f2b9dd8f525038bbb05fe9d89a5d75c249edac868e3"),
            (102_400, "7dc809b57100c529a1c31c9a89b97f4f2f682a327f23d66afa2f5b3922e3ca4e"),
            (131_072, "97eb39e6f0fb754d60677c47fc58038027025b6351fdc5887e54aea232a5e07b"),
            (262_144, "4b0d375a615c0382b4f958b48e43e7f356b4fcac76e20423294adc07b8d4976e"),
        ];

        for (size, expected_sha256) in payloads {
            let config = BenchSourceConfig::new(100_000.0, 1).with_payload_size(size);
            let mut source = BenchSource::new(config);
            source.init().await.unwrap();
            let message = source.poll().await.unwrap().unwrap();

            assert_eq!(message.payload.len(), size);
            assert_eq!(hex::encode(Sha256::digest(&message.payload)), expected_sha256);
        }
    }

    #[tokio::test]
    async fn lifecycle_methods_work() {
        let config = BenchSourceConfig::new(1000.0, 10);
        let mut source = BenchSource::new(config);

        assert_eq!(source.id(), "bench-source");
        assert_eq!(source.node_type(), "bench-source");
        source.validate().unwrap();
        source.init().await.unwrap();
        source.close().await.unwrap();
    }

    #[tokio::test]
    async fn custom_id() {
        let config = BenchSourceConfig::new(1000.0, 1);
        let source = BenchSource::new(config).with_id("my-source");
        assert_eq!(source.id(), "my-source");
    }

    #[tokio::test]
    async fn config_builder_pattern() {
        let config = BenchSourceConfig::new(5000.0, 100).with_warmup(20).with_payload_size(512);

        assert!(
            (config.rate_per_sec - 5000.0).abs() < f64::EPSILON,
            "rate_per_sec should be 5000.0"
        );
        assert_eq!(config.total_messages, 100);
        assert_eq!(config.warmup_messages, 20);
        assert_eq!(config.payload_size, 512);
    }

    #[tokio::test]
    async fn late_poll_keeps_the_scheduled_time_and_reports_the_lag() {
        let mut source = BenchSource::new(BenchSourceConfig::new(10.0, 3));
        source.init().await.unwrap();

        let first = source.poll().await.unwrap().unwrap();
        tokio::time::sleep(Duration::from_millis(350)).await;
        let second = source.poll().await.unwrap().unwrap();
        let third = source.poll().await.unwrap().unwrap();

        let [first, second, third] = [first, second, third].map(|m| m.header.bench.unwrap());
        assert_eq!(second.intended_ns, first.intended_ns + 100_000_000);
        assert_eq!(third.intended_ns, first.intended_ns + 200_000_000);
        let lag = second.emit_ns - second.intended_ns;
        assert!(lag >= 200_000_000, "second message left {lag} ns after its slot");
    }

    #[test]
    fn burst_schedule_emits_exact_phase_counts_and_offsets() {
        let config = BenchSourceConfig::new(1_000.0, 160_000)
            .with_warmup(30_000)
            .with_burst(BenchBurstSchedule::new(2_000.0, 55, 65));
        let source = BenchSource::new(config);

        assert_eq!(source.config.burst_phase_counts(), Some([55_000, 20_000, 55_000]));
        assert_eq!(source.config.measurement_offset_ns(30_000), Some(0));
        assert_eq!(source.config.measurement_offset_ns(84_999), Some(54_999_000_000));
        assert_eq!(source.config.measurement_offset_ns(85_000), Some(55_000_000_000));
        assert_eq!(source.config.measurement_offset_ns(104_999), Some(64_999_500_000));
        assert_eq!(source.config.measurement_offset_ns(105_000), Some(65_000_000_000));
        assert_eq!(source.config.measurement_offset_ns(159_999), Some(119_999_000_000));
    }
}
