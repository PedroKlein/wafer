//! Serializable envelope written to the dead-letter queue.

use crate::queue::{BenchStamps, RuntimeEnvelope};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    /// `BenchSource` stamps, so a dead-lettered message stays identifiable
    /// by sequence number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bench: Option<BenchStamps>,
}

impl From<RuntimeEnvelope> for SerializableEnvelope {
    fn from(env: RuntimeEnvelope) -> Self {
        Self {
            id: env.header.id.to_string(),
            timestamp: env.header.timestamp,
            source: env.header.source.to_string(),
            metadata: env
                .header
                .metadata
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            payload: env.payload.to_vec(),
            retry_count: env.retry_count,
            bench: env.header.bench,
        }
    }
}

impl From<SerializableEnvelope> for RuntimeEnvelope {
    fn from(env: SerializableEnvelope) -> Self {
        use crate::queue::envelope::{EnvelopeHeader, Lineage};
        use bytes::Bytes;
        use std::sync::Arc;

        let header = EnvelopeHeader {
            id: env.id.into_boxed_str(),
            timestamp: env.timestamp,
            source: env.source.into_boxed_str(),
            content_type: "application/octet-stream".into(),
            metadata: env
                .metadata
                .into_iter()
                .map(|(k, v)| (k.into_boxed_str(), v.into_boxed_str()))
                .collect(),
            bench: env.bench,
        };
        Self {
            header: Arc::new(header),
            payload: Bytes::from(env.payload),
            lineage: Lineage::default(),
            retry_count: env.retry_count,
        }
    }
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

    #[test]
    fn dlq_json_roundtrip_preserves_bench_stamps() {
        use crate::queue::{BurstPhase, BurstStamps};

        let stamps = BenchStamps {
            sequence: 41,
            intended_ns: 1_000,
            emit_ns: 1_250,
            warmup: false,
            measurement_start_seq: 10,
            sequence_end: None,
            burst: Some(BurstStamps {
                phase: BurstPhase::Burst,
                measurement_start_unix_ns: Some(900),
            }),
        };
        let envelope = RuntimeEnvelope::from_string("src", "late").with_bench_stamps(stamps);
        let json = serde_json::to_string(&SerializableEnvelope::from(envelope)).unwrap();

        assert!(json.contains("\"sequence\":41"), "record must name the sequence: {json}");
        let decoded: SerializableEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(RuntimeEnvelope::from(decoded).header.bench, Some(stamps));

        let unstamped = SerializableEnvelope::from(RuntimeEnvelope::from_string("src", "x"));
        assert!(!serde_json::to_string(&unstamped).unwrap().contains("bench"));
    }
}
