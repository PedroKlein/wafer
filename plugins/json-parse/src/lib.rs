//! JSON parse transform plugin for WAFER pipeline.
//!
//! This plugin parses JSON input and pretty-prints it with 2-space indentation.
//! Invalid JSON or empty payloads result in a ProcessError.

wit_bindgen::generate!({
    path: "../../wit",
    world: "transform-node",
});

use pipeline::transform::types::{Envelope, Payload, ProcessError, ProcessResult};
use serde_json::Value;

struct JsonParse;

impl exports::pipeline::transform::lifecycle::Guest for JsonParse {
    /// Validate configuration - JSON parse accepts any config (no configuration needed)
    fn validate(_config: exports::pipeline::transform::lifecycle::NodeConfig) -> Option<String> {
        None
    }

    /// Initialize the node - JSON parse needs no initialization
    fn init(_config: exports::pipeline::transform::lifecycle::NodeConfig) -> Result<(), String> {
        Ok(())
    }

    /// Graceful shutdown - JSON parse has nothing to clean up
    fn close() {
        // No resources to release
    }
}

impl exports::pipeline::transform::transform::Guest for JsonParse {
    /// Process a message - parse JSON and pretty-print with 2-space indent
    fn process(input: Envelope) -> ProcessResult {
        let bytes = match &input.payload {
            Payload::Raw(b) => b,
        };

        if bytes.is_empty() {
            return ProcessResult::Error(ProcessError {
                code: "PARSE_ERROR".to_string(),
                message: "Empty payload is not valid JSON".to_string(),
                retriable: false,
            });
        }

        match serde_json::from_slice::<Value>(bytes) {
            Ok(value) => {
                // Pretty print with 2-space indent
                match serde_json::to_vec_pretty(&value) {
                    Ok(pretty) => ProcessResult::Emit(Envelope {
                        payload: Payload::Raw(pretty),
                        ..input
                    }),
                    Err(e) => ProcessResult::Error(ProcessError {
                        code: "SERIALIZE_ERROR".to_string(),
                        message: e.to_string(),
                        retriable: false,
                    }),
                }
            }
            Err(e) => ProcessResult::Error(ProcessError {
                code: "PARSE_ERROR".to_string(),
                message: format!("Invalid JSON: {}", e),
                retriable: false,
            }),
        }
    }
}

export!(JsonParse);
