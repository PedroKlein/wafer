//! Pass-through transform plugin for WAFER pipeline.
//!
//! Implements the `transform-node` world: reads the full payload from the
//! host-managed buffer and returns an identical output-message. Serves as
//! the minimal transform for integration testing the Wasm plugin boundary.

wit_bindgen::generate!({
    path: "../../wit/node",
    world: "transform-node",
    generate_all,
});

struct PassThrough;

impl exports::pipeline::node::lifecycle::Guest for PassThrough {
    fn validate(_config: exports::pipeline::node::lifecycle::NodeConfig) -> Option<String> {
        None
    }

    fn init(
        _config: exports::pipeline::node::lifecycle::NodeConfig,
    ) -> Result<(), exports::pipeline::node::lifecycle::ProcessError> {
        Ok(())
    }

    fn close() {}
}

impl exports::pipeline::node::transform::Guest for PassThrough {
    fn process(
        input: exports::pipeline::node::transform::Message,
    ) -> Result<
        exports::pipeline::node::transform::OutputMessage,
        exports::pipeline::node::transform::ProcessError,
    > {
        // Read entire payload from host buffer (the only copy needed for pass-through)
        let payload = input.payload.read_all();

        Ok(exports::pipeline::node::transform::OutputMessage {
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
