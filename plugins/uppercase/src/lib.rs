//! Uppercase transform plugin for WAFER pipeline.
//!
//! This plugin converts payload bytes to ASCII uppercase.
//! Empty payloads pass through unchanged.
//! Non-UTF8 payloads return an error.

wit_bindgen::generate!({
    path: "../../wit",
    world: "transform-node",
});

struct Uppercase;

impl exports::pipeline::transform::lifecycle::Guest for Uppercase {
    /// Validate configuration - uppercase has no config, always valid
    fn validate(_config: exports::pipeline::transform::lifecycle::NodeConfig) -> Option<String> {
        None
    }

    /// Initialize the node - uppercase needs no initialization
    fn init(_config: exports::pipeline::transform::lifecycle::NodeConfig) -> Result<(), String> {
        Ok(())
    }

    /// Graceful shutdown - uppercase has nothing to clean up
    fn close() {
        // No resources to release
    }
}

impl exports::pipeline::transform::transform::Guest for Uppercase {
    /// Process a message - convert payload to ASCII uppercase
    fn process(
        input: pipeline::transform::types::Envelope,
    ) -> pipeline::transform::types::ProcessResult {
        use pipeline::transform::types::{Envelope, Payload, ProcessError, ProcessResult};

        let Payload::Raw(bytes) = input.payload;

        // Empty payload passes through
        if bytes.is_empty() {
            return ProcessResult::Emit(Envelope {
                payload: Payload::Raw(bytes),
                ..input
            });
        }

        match String::from_utf8(bytes) {
            Ok(text) => {
                let uppercased = text.to_ascii_uppercase();
                ProcessResult::Emit(Envelope {
                    payload: Payload::Raw(uppercased.into_bytes()),
                    ..input
                })
            }
            Err(_) => ProcessResult::Error(ProcessError {
                code: "INVALID_UTF8".to_string(),
                message: "Payload is not valid UTF-8".to_string(),
                retriable: false,
            }),
        }
    }
}

export!(Uppercase);
