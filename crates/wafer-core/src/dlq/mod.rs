//! Dead Letter Queue (DLQ) types and utilities.
//!
//! This module provides the envelope types used to wrap failed messages
//! before routing them to the Dead Letter Queue sink.

use crate::queue::RuntimeEnvelope;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Reason why a message was routed to the Dead Letter Queue.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DlqReason {
    /// Queue was full when message arrived (overflow with `dead-letter` policy).
    QueueFull,
    /// Transform/router/joiner processing returned an error.
    ProcessError {
        /// Error code (e.g., "transform_failed", "route_error").
        code: String,
        /// Human-readable error message.
        message: String,
    },
    /// Sink failed to collect the message.
    SinkError {
        /// Human-readable error message.
        message: String,
    },
}

impl DlqReason {
    /// Create a `ProcessError` reason.
    pub fn process_error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::ProcessError {
            code: code.into(),
            message: message.into(),
        }
    }

    /// Create a `SinkError` reason.
    pub fn sink_error(message: impl Into<String>) -> Self {
        Self::SinkError {
            message: message.into(),
        }
    }
}

/// Serializable representation of a `RuntimeEnvelope`.
///
/// Used within `DlqEnvelope` to preserve the original message.
/// The payload is encoded as base64 since it may contain binary data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableEnvelope {
    /// Unique message identifier.
    pub id: String,
    /// Creation timestamp (Unix millis).
    pub timestamp: u64,
    /// Source node name.
    pub source: String,
    /// Message metadata.
    pub metadata: HashMap<String, String>,
    /// Raw payload bytes encoded as base64.
    #[serde(with = "base64_serde")]
    pub payload: Vec<u8>,
}

impl From<RuntimeEnvelope> for SerializableEnvelope {
    fn from(env: RuntimeEnvelope) -> Self {
        Self {
            id: env.id,
            timestamp: env.timestamp,
            source: env.source,
            metadata: env.metadata,
            payload: env.payload,
        }
    }
}

impl From<SerializableEnvelope> for RuntimeEnvelope {
    fn from(env: SerializableEnvelope) -> Self {
        Self {
            id: env.id,
            timestamp: env.timestamp,
            source: env.source,
            metadata: env.metadata,
            payload: env.payload,
        }
    }
}

/// Dead Letter Queue envelope wrapping a failed message.
///
/// This envelope captures the original message along with error context
/// for debugging and potential replay scenarios.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlqEnvelope {
    /// Original message that failed.
    pub original: SerializableEnvelope,
    /// Edge identifier where the failure occurred (e.g., "source:default->transform:default").
    pub failed_edge: String,
    /// Reason for the failure.
    pub reason: DlqReason,
    /// When the failure occurred (UTC timestamp).
    pub failed_at: DateTime<Utc>,
}

impl DlqEnvelope {
    /// Create a new DLQ envelope.
    pub fn new(
        original: RuntimeEnvelope,
        failed_edge: impl Into<String>,
        reason: DlqReason,
    ) -> Self {
        Self {
            original: original.into(),
            failed_edge: failed_edge.into(),
            reason,
            failed_at: Utc::now(),
        }
    }

    /// Extract the original envelope for potential replay.
    pub fn into_original(self) -> RuntimeEnvelope {
        self.original.into()
    }

    /// Serialize this envelope to JSON bytes.
    ///
    /// This is used when wrapping the DLQ envelope as payload in a `RuntimeEnvelope`
    /// for routing to the DLQ sink.
    pub fn to_json_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }
}

/// Wrap a failed message for routing to the Dead Letter Queue.
///
/// This creates a new `RuntimeEnvelope` with the `DlqEnvelope` serialized as JSON payload.
/// The returned envelope can be sent through standard queue channels to the DLQ sink.
///
/// # Arguments
///
/// * `envelope` - The original message that failed.
/// * `failed_edge` - Identifier for the edge where failure occurred.
/// * `reason` - Why the message failed.
///
/// # Returns
///
/// A new `RuntimeEnvelope` with source "dlq" and the serialized `DlqEnvelope` as payload.
///
/// # Panics
///
/// Panics if `DlqEnvelope` serialization fails, which should never happen
/// for valid envelope data.
pub fn wrap_for_dlq(
    envelope: RuntimeEnvelope,
    failed_edge: impl Into<String>,
    reason: DlqReason,
) -> RuntimeEnvelope {
    let dlq_envelope = DlqEnvelope::new(envelope, failed_edge, reason);
    let payload = dlq_envelope
        .to_json_bytes()
        .expect("DlqEnvelope serialization should not fail");

    RuntimeEnvelope::new("dlq", payload).with_metadata("content_type", "application/json")
}

/// Custom serde module for base64 encoding/decoding of binary data.
mod base64_serde {
    use serde::{Deserialize, Deserializer, Serializer};

    const ENGINE: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use base64::Engine;
        serializer.serialize_str(&ENGINE.encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        use base64::Engine;
        let s = String::deserialize(deserializer)?;
        ENGINE.decode(s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dlq_reason_queue_full_serializes() {
        let reason = DlqReason::QueueFull;
        let json = serde_json::to_string(&reason).unwrap();
        assert_eq!(json, r#"{"type":"queue_full"}"#);

        let parsed: DlqReason = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, reason);
    }

    #[test]
    fn dlq_reason_process_error_serializes() {
        let reason = DlqReason::process_error("transform_failed", "invalid JSON");
        let json = serde_json::to_string(&reason).unwrap();
        assert!(json.contains(r#""type":"process_error""#));
        assert!(json.contains(r#""code":"transform_failed""#));
        assert!(json.contains(r#""message":"invalid JSON""#));

        let parsed: DlqReason = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, reason);
    }

    #[test]
    fn dlq_reason_sink_error_serializes() {
        let reason = DlqReason::sink_error("connection refused");
        let json = serde_json::to_string(&reason).unwrap();
        assert!(json.contains(r#""type":"sink_error""#));
        assert!(json.contains(r#""message":"connection refused""#));

        let parsed: DlqReason = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, reason);
    }

    #[test]
    fn serializable_envelope_roundtrip() {
        let original = RuntimeEnvelope::new("test-source", b"hello world".to_vec())
            .with_metadata("key", "value");

        let serializable: SerializableEnvelope = original.clone().into();
        let json = serde_json::to_string(&serializable).unwrap();

        // Check base64 encoding of payload
        assert!(json.contains(r#""payload":"aGVsbG8gd29ybGQ=""#)); // "hello world" in base64

        let parsed: SerializableEnvelope = serde_json::from_str(&json).unwrap();
        let restored: RuntimeEnvelope = parsed.into();

        assert_eq!(restored.id, original.id);
        assert_eq!(restored.timestamp, original.timestamp);
        assert_eq!(restored.source, original.source);
        assert_eq!(restored.metadata, original.metadata);
        assert_eq!(restored.payload, original.payload);
    }

    #[test]
    fn dlq_envelope_serialization_roundtrip() {
        let original = RuntimeEnvelope::new("test-source", b"test payload".to_vec());
        let dlq = DlqEnvelope::new(original.clone(), "src->transform", DlqReason::QueueFull);

        let json = serde_json::to_string(&dlq).unwrap();
        let parsed: DlqEnvelope = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.failed_edge, "src->transform");
        assert_eq!(parsed.reason, DlqReason::QueueFull);
        assert_eq!(parsed.original.id, original.id);
        assert_eq!(parsed.original.payload, original.payload);
    }

    #[test]
    fn wrap_for_dlq_creates_valid_envelope() {
        let original = RuntimeEnvelope::new("test-source", b"test".to_vec());
        let original_id = original.id.clone();

        let wrapped = wrap_for_dlq(original, "edge-1", DlqReason::QueueFull);

        assert_eq!(wrapped.source, "dlq");
        assert_eq!(
            wrapped.metadata.get("content_type"),
            Some(&"application/json".to_string())
        );

        // Parse the payload back
        let dlq_envelope: DlqEnvelope = serde_json::from_slice(&wrapped.payload).unwrap();
        assert_eq!(dlq_envelope.original.id, original_id);
        assert_eq!(dlq_envelope.reason, DlqReason::QueueFull);
    }

    #[test]
    fn dlq_envelope_into_original_extracts_message() {
        let original = RuntimeEnvelope::new("test-source", b"test".to_vec());
        let original_id = original.id.clone();
        let original_payload = original.payload.clone();

        let dlq = DlqEnvelope::new(original, "edge-1", DlqReason::QueueFull);
        let restored = dlq.into_original();

        assert_eq!(restored.id, original_id);
        assert_eq!(restored.payload, original_payload);
    }
}
