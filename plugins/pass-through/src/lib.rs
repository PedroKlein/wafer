//! Pass-through transform plugin for WAFER pipeline.
//!
//! This plugin implements a no-op transform that passes envelopes through unchanged.
//! It serves as the simplest possible transform implementation and can be used
//! for testing the WASM plugin infrastructure.

wit_bindgen::generate!({
    inline: r#"
        package pipeline:transform@0.1.0;

        interface types {
            type message-id = string;
            type timestamp = u64;
            
            record metadata-entry {
                key: string,
                value: string,
            }
            
            variant payload {
                raw(list<u8>),
            }
            
            record envelope {
                id: message-id,
                timestamp: timestamp,
                source: string,
                metadata: list<metadata-entry>,
                payload: payload,
            }
            
            variant process-result {
                emit(envelope),
                filter,
                error(process-error),
            }
            
            record process-error {
                code: u32,
                message: string,
                retriable: bool,
            }
        }

        interface lifecycle {
            use types.{metadata-entry};
            
            record node-config {
                name: string,
                config: list<metadata-entry>,
            }
            
            init: func(config: node-config) -> result<_, string>;
        }

        interface transform {
            use types.{envelope, process-result};
            
            process: func(input: envelope) -> process-result;
        }

        world transform-node {
            import types;
            
            export lifecycle;
            export transform;
        }
    "#,
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
