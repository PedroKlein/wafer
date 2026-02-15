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
    fn init(_config: exports::pipeline::transform::lifecycle::NodeConfig) -> Result<(), String> {
        Ok(())
    }
}

impl exports::pipeline::transform::transform::Guest for PassThrough {
    fn process(
        input: pipeline::transform::types::Envelope,
    ) -> pipeline::transform::types::ProcessResult {
        pipeline::transform::types::ProcessResult::Emit(input)
    }
}

export!(PassThrough);
