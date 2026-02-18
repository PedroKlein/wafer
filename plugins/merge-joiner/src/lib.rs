//! Merge joiner plugin for WAFER pipeline.
//!
//! This plugin implements a stateless joiner that accepts messages from multiple
//! input ports and passes them through unchanged. It serves as the simplest
//! possible joiner implementation for merging multiple data streams.

wit_bindgen::generate!({
    path: "../../wit",
    world: "joiner-node",
});

struct MergeJoiner;

impl exports::pipeline::transform::lifecycle::Guest for MergeJoiner {
    /// Validate configuration - merge joiner accepts any config
    fn validate(_config: exports::pipeline::transform::lifecycle::NodeConfig) -> Option<String> {
        // No validation errors - merge joiner accepts any config
        None
    }

    /// Initialize the node - merge joiner needs no initialization
    fn init(_config: exports::pipeline::transform::lifecycle::NodeConfig) -> Result<(), String> {
        Ok(())
    }

    /// Graceful shutdown - merge joiner has nothing to clean up
    fn close() {
        // No resources to release
    }
}

impl exports::pipeline::transform::joiner::Guest for MergeJoiner {
    /// Return the list of input ports this joiner accepts
    fn input_ports() -> Vec<String> {
        vec!["input-a".to_string(), "input-b".to_string()]
    }

    /// Process a message from any input port - stateless passthrough
    fn process(
        _port: String,
        input: pipeline::transform::types::Envelope,
    ) -> pipeline::transform::types::ProcessResult {
        // Stateless passthrough - just emit the message unchanged
        pipeline::transform::types::ProcessResult::Emit(input)
    }
}

export!(MergeJoiner);
