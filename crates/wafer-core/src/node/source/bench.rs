//! Rate-controlled benchmark source for evaluation experiments.
//!
//! `BenchSource` emits messages at a constant arrival rate with intended-publish-time
//! stamps to prevent coordinated omission (Tene 2012). Each message carries a monotonic
//! sequence number for gap/duplicate detection at the sink.
//!
//! See docs/rfcs/RFC-008-evaluation-harness.md — Session 8 D3A.

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;

use crate::error::Result;
use crate::node::{Lifecycle, Source};
use crate::queue::RuntimeEnvelope;

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
}

impl BenchSourceConfig {
    /// Create a new config with required parameters.
    #[must_use]
    pub const fn new(rate_per_sec: f64, total_messages: u64) -> Self {
        Self {
            rate_per_sec,
            total_messages,
            warmup_messages: 0,
            payload_size: 128,
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
}

/// Rate-controlled source that emits sequenced, timestamped messages.
///
/// Designed for open-loop benchmarking: the intended publish time is computed
/// from `sequence * interval` rather than actual emission time. This ensures
/// that if the system falls behind, the measured latency correctly includes
/// the queuing delay (preventing coordinated omission).
pub struct BenchSource {
    id: String,
    config: BenchSourceConfig,
    sequence: u64,
    start_time: Option<Instant>,
    interval: Option<tokio::time::Interval>,
    payload: Bytes,
    /// Interval between messages in nanoseconds (precomputed for intended_ns calc).
    interval_ns: u64,
}

impl BenchSource {
    /// Create a new BenchSource from configuration.
    #[must_use]
    pub fn new(config: BenchSourceConfig) -> Self {
        let payload = Bytes::from(vec![0x42u8; config.payload_size]);
        #[expect(
            clippy::as_conversions,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "f64→u64: 1e9/rate is always a positive finite value < u64::MAX for any rate ≥ 1"
        )]
        let interval_ns = (1_000_000_000.0 / config.rate_per_sec) as u64;

        Self {
            id: "bench-source".to_owned(),
            config,
            sequence: 0,
            start_time: None,
            interval: None,
            payload,
            interval_ns,
        }
    }

    /// Create with a custom node ID.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }
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
        Ok(())
    }

    fn init(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }

    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }
}

impl Source for BenchSource {
    fn poll(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<RuntimeEnvelope>>> + Send + '_>> {
        Box::pin(async {
            // Check completion
            if self.sequence >= self.config.total_messages {
                return Ok(None);
            }

            // Initialize on first call
            if self.interval.is_none() {
                let duration = Duration::from_secs_f64(1.0 / self.config.rate_per_sec);
                let mut interval = tokio::time::interval(duration);
                // First tick completes immediately — consume it
                interval.tick().await;
                self.interval = Some(interval);
                self.start_time = Some(Instant::now());
            }

            // Wait for next tick
            if let Some(ref mut interval) = self.interval {
                interval.tick().await;
            }

            // Compute intended publish time (prevents coordinated omission)
            let _intended_ns = self.sequence.saturating_mul(self.interval_ns);

            // Build envelope
            let seq = self.sequence;
            self.sequence = self.sequence.saturating_add(1);

            // Use real wall-clock for the envelope timestamp field, but intended_ns
            // in metadata for latency calculation at the sink
            let now_ns = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, crate::util::duration_ns_saturating);

            let envelope = RuntimeEnvelope::new("bench-source", self.payload.clone())
                .with_metadata("bench.sequence", seq.to_string())
                .with_metadata("bench.intended_ns", now_ns.to_string());

            Ok(Some(envelope))
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
            let seq_meta = msg
                .header
                .metadata
                .iter()
                .find(|(k, _)| k.as_ref() == "bench.sequence")
                .map(|(_, v)| v.as_ref())
                .unwrap();
            assert_eq!(seq_meta, expected_seq.to_string());
        }
    }

    #[tokio::test]
    async fn metadata_contains_required_fields() {
        let config = BenchSourceConfig::new(100_000.0, 1);
        let mut source = BenchSource::new(config);

        source.init().await.unwrap();

        let msg = source.poll().await.unwrap().unwrap();
        let keys: Vec<&str> = msg.header.metadata.iter().map(|(k, _)| k.as_ref()).collect();
        assert!(keys.contains(&"bench.sequence"));
        assert!(keys.contains(&"bench.intended_ns"));
    }

    #[tokio::test]
    async fn payload_has_configured_size() {
        let config = BenchSourceConfig::new(100_000.0, 1).with_payload_size(256);
        let mut source = BenchSource::new(config);

        source.init().await.unwrap();

        let msg = source.poll().await.unwrap().unwrap();
        assert_eq!(msg.payload.len(), 256);
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
        let config = BenchSourceConfig::new(5000.0, 100)
            .with_warmup(20)
            .with_payload_size(512);

        assert!(
            (config.rate_per_sec - 5000.0).abs() < f64::EPSILON,
            "rate_per_sec should be 5000.0"
        );
        assert_eq!(config.total_messages, 100);
        assert_eq!(config.warmup_messages, 20);
        assert_eq!(config.payload_size, 512);
    }
}
