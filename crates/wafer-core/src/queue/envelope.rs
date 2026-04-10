//! Runtime envelope type for pipeline messages.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Host-side message envelope, converted to/from WIT types at the WASM boundary.
#[derive(Debug, Clone)]
pub struct RuntimeEnvelope {
    pub id: String,
    pub timestamp: u64,
    pub source: String,
    pub metadata: HashMap<String, String>,
    pub payload: Vec<u8>,
}

impl RuntimeEnvelope {
    #[must_use]
    pub fn new(source: impl Into<String>, payload: Vec<u8>) -> Self {
        // Saturates at u64::MAX (~584 million years from epoch)
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or_else(
            |_| {
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

    #[must_use]
    pub fn from_string(source: impl Into<String>, data: impl Into<String>) -> Self {
        Self::new(source, data.into().into_bytes())
    }

    #[must_use]
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    pub fn payload_as_string(&self) -> String {
        String::from_utf8_lossy(&self.payload).to_string()
    }
}

impl Default for RuntimeEnvelope {
    fn default() -> Self {
        Self::new("unknown", Vec::new())
    }
}
