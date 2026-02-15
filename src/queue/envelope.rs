//! Runtime envelope type for pipeline messages.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Runtime representation of a pipeline message.
///
/// This is the host-side envelope that gets converted to/from
/// the WIT-generated types when crossing the WASM boundary.
#[derive(Debug, Clone)]
pub struct RuntimeEnvelope {
    /// Unique message identifier.
    pub id: String,
    /// Creation timestamp (Unix millis).
    pub timestamp: u64,
    /// Source node name.
    pub source: String,
    /// Message metadata.
    pub metadata: HashMap<String, String>,
    /// Raw payload bytes.
    pub payload: Vec<u8>,
}

impl RuntimeEnvelope {
    /// Create a new envelope with the given payload.
    pub fn new(source: impl Into<String>, payload: Vec<u8>) -> Self {
        // Timestamp in milliseconds since UNIX epoch
        // Saturates at u64::MAX for dates far in the future (~584 million years)
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or_else(
            |_| {
                // System clock is before UNIX epoch (misconfigured system)
                // This should be extremely rare but we handle it gracefully
                tracing::warn!("system clock before UNIX epoch, using 0 as timestamp");
                0
            },
            |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX),
        );

        Self {
            id: Uuid::new_v4().to_string(),
            timestamp,
            source: source.into(),
            metadata: HashMap::new(),
            payload,
        }
    }

    /// Create an envelope with a string payload.
    pub fn from_string(source: impl Into<String>, data: impl Into<String>) -> Self {
        Self::new(source, data.into().into_bytes())
    }

    /// Add metadata entry.
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Get payload as UTF-8 string (lossy).
    pub fn payload_as_string(&self) -> String {
        String::from_utf8_lossy(&self.payload).to_string()
    }
}

impl Default for RuntimeEnvelope {
    fn default() -> Self {
        Self::new("unknown", Vec::new())
    }
}
