//! Runtime envelope type for pipeline messages.
//!
//! Uses `Arc<EnvelopeHeader>` for cheap fan-out clones and `Bytes` for
//! zero-copy payload sharing from MQTT/network sources.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

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
pub(crate) struct Lineage {
    pub parent_id: Option<Box<str>>,
    pub trace_id: Option<Box<str>>,
}

impl RuntimeEnvelope {
    /// Create a new envelope with a generated UUID and current timestamp.
    #[must_use]
    pub fn new(source: impl Into<Box<str>>, payload: Bytes) -> Self {
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or_else(
            |_| {
                tracing::warn!("system clock before UNIX epoch, using 0 as timestamp");
                0
            },
            |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX),
        );

        let header = EnvelopeHeader {
            id: Uuid::new_v4().to_string().into_boxed_str(),
            timestamp,
            source: source.into(),
            content_type: "application/octet-stream".into(),
            metadata: Vec::new(),
        };

        Self {
            header: Arc::new(header),
            payload,
            lineage: Lineage::default(),
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
        Arc::make_mut(&mut self.header)
            .metadata
            .push((key.into(), value.into()));
        self
    }

    /// Read payload as a UTF-8 string (lossy conversion).
    #[must_use]
    pub fn payload_as_string(&self) -> String {
        String::from_utf8_lossy(&self.payload).to_string()
    }
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
