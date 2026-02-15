//! Pass-through transform plugin for WAFER pipeline.
//!
//! This plugin implements a no-op transform that passes envelopes through unchanged.
//! It serves as the simplest possible transform implementation and can be used
//! for testing the WASM plugin infrastructure.

wit_bindgen::generate!({
    path: "../../wit",
    world: "transform-node",
});

struct PassThrough;

impl exports::pipeline::transform::lifecycle::Guest for PassThrough {
    /// Validate configuration - pass-through accepts any config
    fn validate(_config: exports::pipeline::transform::lifecycle::NodeConfig) -> Option<String> {
        // No validation errors - pass-through accepts any config
        None
    }

    /// Initialize the node - pass-through needs no initialization
    fn init(_config: exports::pipeline::transform::lifecycle::NodeConfig) -> Result<(), String> {
        Ok(())
    }

    /// Graceful shutdown - pass-through has nothing to clean up
    fn close() {
        // No resources to release
    }
}

impl exports::pipeline::transform::transform::Guest for PassThrough {
    /// Process a message - pass-through emits unchanged
    fn process(
        input: pipeline::transform::types::Envelope,
    ) -> pipeline::transform::types::ProcessResult {
        pipeline::transform::types::ProcessResult::Emit(input)
    }
}

export!(PassThrough);
