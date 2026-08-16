//! Pass-through transform plugin for WAFER pipeline.
//!
//! Implements the `transform-node` world: reads the full payload from the
//! host-managed buffer and returns an identical output-message. Serves as
//! the minimal transform for integration testing the Wasm plugin boundary.

wit_bindgen::generate!({
    path: "../../wit",
    world: "transform-node",
    generate_all,
});

struct PassThrough;

impl exports::wafer::pipeline::lifecycle::Guest for PassThrough {
    fn validate(_config: exports::wafer::pipeline::lifecycle::NodeConfig) -> Option<String> {
        None
    }

    fn init(
        _config: exports::wafer::pipeline::lifecycle::NodeConfig,
    ) -> Result<(), exports::wafer::pipeline::lifecycle::ProcessError> {
        Ok(())
    }

    fn close() {}
}

impl exports::wafer::pipeline::transform::Guest for PassThrough {
    fn process(
        input: exports::wafer::pipeline::transform::Message,
    ) -> Result<
        exports::wafer::pipeline::transform::OutputMessage,
        exports::wafer::pipeline::transform::ProcessError,
    > {
        // Read entire payload from host buffer (the only copy needed for pass-through)
        let payload = input.payload.read_all();

        Ok(exports::wafer::pipeline::transform::OutputMessage {
            id: input.id,
            timestamp: input.timestamp,
            source: input.source,
            content_type: input.content_type,
            metadata: input.metadata,
            payload,
        })
    }
}

export!(PassThrough);
