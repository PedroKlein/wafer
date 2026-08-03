//! Dead Letter Queue (DLQ) types and utilities.

use crate::queue::RuntimeEnvelope;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Reason why a message was routed to the Dead Letter Queue.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DlqReason {
    QueueFull,
    ProcessError { code: String, message: String },
    SinkError { message: String },
}

impl DlqReason {
    pub fn process_error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::ProcessError { code: code.into(), message: message.into() }
    }

    pub fn sink_error(message: impl Into<String>) -> Self {
        Self::SinkError { message: message.into() }
    }
}

/// Serializable representation of a `RuntimeEnvelope`.
///
/// Payload is encoded as base64 since it may contain binary data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableEnvelope {
    pub id: String,
    pub timestamp: u64,
    pub source: String,
    pub metadata: HashMap<String, String>,
    #[serde(with = "base64_serde")]
    pub payload: Vec<u8>,
    /// Retry attempts burned before this envelope hit the DLQ.
    /// Preserved across DLQ round-trips so re-injected poison messages
    /// cannot regain a fresh retry budget. `#[serde(default)]` keeps
    /// pre-existing DLQ records readable.
    #[serde(default)]
    pub retry_count: u32,
}

impl From<RuntimeEnvelope> for SerializableEnvelope {
    fn from(env: RuntimeEnvelope) -> Self {
        Self {
            id: env.header.id.to_string(),
            timestamp: env.header.timestamp,
            source: env.header.source.to_string(),
            metadata: env.header.metadata.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            payload: env.payload.to_vec(),
            retry_count: env.retry_count,
        }
    }
}

impl From<SerializableEnvelope> for RuntimeEnvelope {
    fn from(env: SerializableEnvelope) -> Self {
        use std::sync::Arc;
        use bytes::Bytes;
        use crate::queue::envelope::{EnvelopeHeader, Lineage};

        let header = EnvelopeHeader {
            id: env.id.into_boxed_str(),
            timestamp: env.timestamp,
            source: env.source.into_boxed_str(),
            content_type: "application/octet-stream".into(),
            metadata: env.metadata.into_iter().map(|(k, v)| (k.into_boxed_str(), v.into_boxed_str())).collect(),
        };
        Self {
            header: Arc::new(header),
            payload: Bytes::from(env.payload),
            lineage: Lineage::default(),
            retry_count: env.retry_count,
        }
    }
}

/// Dead Letter Queue envelope wrapping a failed message with error context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlqEnvelope {
    pub original: SerializableEnvelope,
    pub failed_edge: String,
    pub reason: DlqReason,
    pub failed_at: DateTime<Utc>,
}

impl DlqEnvelope {
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

    /// Serialize this envelope to JSON bytes for routing to the DLQ sink.
    pub fn to_json_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }
}

/// Wrap an envelope in DLQ metadata for dead-letter routing.
///
/// # Panics
///
/// Panics if JSON serialization of the DLQ envelope fails (unreachable: all fields are serde-infallible).
#[expect(clippy::expect_used, reason = "DlqEnvelope fields are all serde-infallible types (String, u64, HashMap, Vec<u8>)")]
pub fn wrap_for_dlq(
    envelope: RuntimeEnvelope,
    failed_edge: impl Into<String>,
    reason: DlqReason,
) -> RuntimeEnvelope {
    let dlq_envelope = DlqEnvelope::new(envelope, failed_edge, reason);
    // SAFETY: DlqEnvelope contains only String, u64, HashMap<String,String>, Vec<u8> -- all infallible to serialize
    let payload = dlq_envelope.to_json_bytes().expect("infallible serialization");

    RuntimeEnvelope::new("dlq", bytes::Bytes::from(payload)).with_metadata("content_type", "application/json")
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
    use crate::queue::RuntimeEnvelope;

    /// Regression: DLQ round-trip must preserve `retry_count` so a
    /// re-injected poison message cannot silently reset its exhausted
    /// budget.
    #[test]
    fn dlq_roundtrip_preserves_retry_count() {
        let mut envelope = RuntimeEnvelope::from_string("src", "poison");
        envelope.retry_count = 7;

        let serial: SerializableEnvelope = envelope.into();
        assert_eq!(serial.retry_count, 7, "SerializableEnvelope must carry retry_count");

        let round_trip: RuntimeEnvelope = serial.into();
        assert_eq!(
            round_trip.retry_count, 7,
            "RuntimeEnvelope reconstructed from SerializableEnvelope must preserve retry_count"
        );
    }

    /// Regression: JSON serialisation carries `retry_count`; legacy
    /// records without the field decode as 0 (backward compat).
    #[test]
    fn dlq_json_roundtrip_preserves_retry_count() {
        let mut envelope = RuntimeEnvelope::from_string("src", "poison");
        envelope.retry_count = 3;
        let serial: SerializableEnvelope = envelope.into();

        let json = serde_json::to_string(&serial).unwrap();
        assert!(
            json.contains("\"retry_count\":3"),
            "serialized JSON must expose retry_count; got: {json}"
        );

        let decoded: SerializableEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.retry_count, 3);

        // Legacy DLQ records (pre-field) decode as retry_count = 0.
        let legacy_json = json.replace(",\"retry_count\":3", "");
        let legacy: SerializableEnvelope = serde_json::from_str(&legacy_json).unwrap();
        assert_eq!(
            legacy.retry_count, 0,
            "legacy DLQ records (no retry_count field) must default to 0"
        );
    }
}
