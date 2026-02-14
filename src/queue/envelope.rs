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
        Self {
            id: Uuid::new_v4().to_string(),
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
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
