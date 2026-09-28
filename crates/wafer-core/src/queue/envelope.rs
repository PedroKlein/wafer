//! Runtime envelope type for pipeline messages.
//!
//! Uses `Arc<EnvelopeHeader>` for cheap fan-out clones and `Bytes` for
//! payload sharing between host-side stages. Sharing stops at the Wasm
//! boundary: every Wasm hop copies the payload out of guest memory and
//! copies the header strings in both directions.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use uuid::Uuid;

/// Host-side message envelope flowing through pipeline queues.
///
/// Clone semantics: header is shared via Arc bump, payload via Bytes refcount,
/// only lineage is deep-copied (two small Option<Box<str>> fields).
#[derive(Debug, Clone)]
pub struct RuntimeEnvelope {
    pub header: Arc<EnvelopeHeader>,
    pub payload: Bytes,
    pub(crate) lineage: Lineage,
    /// P0.11 (A7 residual): number of retry attempts this envelope has
    /// survived. Zero on first ingest; incremented by `error_policy::try_retry`
    /// before pushing into the retry buffer. When it reaches
    /// `ResolvedRetryConfig.retries`, the envelope is sent to the DLQ with
    /// `DlqReason::RetriesExhausted { max_retries }` instead of being
    /// requeued.
    pub retry_count: u32,
}

/// Immutable identity fields shared across fan-out clones.
///
/// Box<str> saves 8 bytes per field vs String (no capacity field) — worthwhile
/// on the hot path where millions of envelopes are in flight.
#[derive(Debug, Clone)]
pub struct EnvelopeHeader {
    pub id: Box<str>,
    pub timestamp: u64,
    pub source: Box<str>,
    pub content_type: Box<str>,
    pub metadata: Vec<(Box<str>, Box<str>)>,
}

/// Internal lineage tracking for distributed tracing.
#[derive(Debug, Clone, Default)]
pub struct Lineage {
    pub parent_id: Option<Box<str>>,
    pub trace_id: Option<Box<str>>,
}

impl RuntimeEnvelope {
    /// Create a new envelope with a generated UUID and current timestamp.
    #[must_use]
    pub fn new(source: impl Into<Box<str>>, payload: Bytes) -> Self {
        let timestamp = unix_epoch_nanos(SystemTime::now());

        let header = EnvelopeHeader {
            id: Uuid::new_v4().to_string().into_boxed_str(),
            timestamp,
            source: source.into(),
            content_type: "application/octet-stream".into(),
            metadata: Vec::new(),
        };

        Self { header: Arc::new(header), payload, lineage: Lineage::default(), retry_count: 0 }
    }

    /// Create an envelope from fields supplied by a transform guest.
    pub(crate) fn from_output_fields(
        id: Box<str>,
        timestamp: u64,
        source: Box<str>,
        content_type: Box<str>,
        metadata: Vec<(Box<str>, Box<str>)>,
        payload: Bytes,
    ) -> Self {
        Self {
            header: Arc::new(EnvelopeHeader { id, timestamp, source, content_type, metadata }),
            payload,
            lineage: Lineage::default(),
            retry_count: 0,
        }
    }

    /// Create an envelope from a UTF-8 string payload.
    #[must_use]
    pub fn from_string(source: impl Into<Box<str>>, data: impl Into<String>) -> Self {
        Self::new(source, Bytes::from(data.into()))
    }

    /// Add a metadata key-value pair (builder pattern).
    #[must_use]
    pub fn with_metadata(mut self, key: impl Into<Box<str>>, value: impl Into<Box<str>>) -> Self {
        Arc::make_mut(&mut self.header).metadata.push((key.into(), value.into()));
        self
    }

    /// Ensure this envelope has a trace ID, assigning one if absent.
    pub fn ensure_trace_id(&mut self) {
        if self.lineage.trace_id.is_none() {
            self.lineage.trace_id = Some(Uuid::new_v4().to_string().into_boxed_str());
        }
    }

    /// Copy lineage from another envelope.
    pub fn inherit_lineage_from(&mut self, parent: &Self) {
        self.lineage = parent.lineage.clone();
    }

    /// Set this envelope's parent ID.
    pub fn set_parent_id(&mut self, parent_id: impl Into<Box<str>>) {
        self.lineage.parent_id = Some(parent_id.into());
    }

    /// Return the current trace ID, if assigned.
    #[must_use]
    pub fn trace_id(&self) -> Option<&str> {
        self.lineage.trace_id.as_deref()
    }

    /// Return the current parent ID, if assigned.
    #[must_use]
    pub fn parent_id(&self) -> Option<&str> {
        self.lineage.parent_id.as_deref()
    }

    /// Read payload as a UTF-8 string (lossy conversion).
    #[must_use]
    pub fn payload_as_string(&self) -> String {
        String::from_utf8_lossy(&self.payload).to_string()
    }
}

fn unix_epoch_nanos(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH).map_or_else(
        |_| {
            tracing::warn!("system clock before UNIX epoch, using 0 as timestamp");
            0
        },
        duration_to_unix_epoch_nanos,
    )
}

fn duration_to_unix_epoch_nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

impl Default for RuntimeEnvelope {
    fn default() -> Self {
        Self::new(Box::<str>::from("unknown"), Bytes::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clone_shares_header_via_arc() {
        let envelope = RuntimeEnvelope::from_string("test-source", "hello");
        let cloned = envelope.clone();
        assert!(Arc::ptr_eq(&envelope.header, &cloned.header));
    }

    #[test]
    fn test_clone_shares_payload_bytes() {
        let envelope = RuntimeEnvelope::from_string("test-source", "hello");
        let cloned = envelope.clone();
        assert_eq!(envelope.payload.as_ptr(), cloned.payload.as_ptr());
    }

    #[test]
    fn test_clone_copies_lineage() {
        let mut envelope = RuntimeEnvelope::from_string("test-source", "hello");
        envelope.lineage.parent_id = Some("parent-123".into());
        let mut cloned = envelope.clone();
        cloned.lineage.parent_id = Some("different".into());
        assert_eq!(envelope.lineage.parent_id.as_deref(), Some("parent-123"));
    }

    #[test]
    fn test_new_creates_valid_envelope() {
        let envelope = RuntimeEnvelope::new(Box::<str>::from("sensor-a"), Bytes::from("data"));
        assert!(!envelope.header.id.is_empty());
        assert!(envelope.header.timestamp > 0);
        assert_eq!(&*envelope.header.source, "sensor-a");
        assert!(envelope.header.metadata.is_empty());
    }

    #[test]
    fn new_timestamp_is_unix_epoch_nanoseconds() {
        let envelope = RuntimeEnvelope::from_string("sensor-a", "data");
        assert!(
            envelope.header.timestamp > 1_000_000_000_000_000_000,
            "timestamp must be nanoseconds since the Unix epoch, not milliseconds: {}",
            envelope.header.timestamp
        );
    }

    #[test]
    fn unix_epoch_nanos_clamps_pre_epoch_and_saturates() {
        assert_eq!(unix_epoch_nanos(UNIX_EPOCH), 0);
        assert_eq!(
            unix_epoch_nanos(UNIX_EPOCH.checked_add(Duration::from_nanos(42)).expect("valid time")),
            42
        );
        assert_eq!(
            unix_epoch_nanos(UNIX_EPOCH.checked_sub(Duration::from_nanos(1)).expect("valid time")),
            0
        );
        assert_eq!(duration_to_unix_epoch_nanos(Duration::from_secs(u64::MAX)), u64::MAX);
    }

    #[test]
    fn test_from_string_creates_utf8_payload() {
        let envelope = RuntimeEnvelope::from_string("src", "hello world");
        assert_eq!(envelope.payload.as_ref(), b"hello world");
    }

    #[test]
    fn test_with_metadata_adds_entry() {
        let envelope = RuntimeEnvelope::from_string("src", "data")
            .with_metadata("key1", "value1")
            .with_metadata("key2", "value2");
        assert_eq!(envelope.header.metadata.len(), 2);
        assert_eq!(&*envelope.header.metadata[0].0, "key1");
        assert_eq!(&*envelope.header.metadata[0].1, "value1");
    }

    #[test]
    fn test_payload_as_string_works() {
        let envelope = RuntimeEnvelope::from_string("src", "round-trip test");
        assert_eq!(envelope.payload_as_string(), "round-trip test");
    }

    #[test]
    fn test_default_has_empty_lineage() {
        let envelope = RuntimeEnvelope::default();
        assert!(envelope.lineage.parent_id.is_none());
        assert!(envelope.lineage.trace_id.is_none());
    }
}
