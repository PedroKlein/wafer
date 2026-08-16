//! Uppercase transform plugin for WAFER pipeline.
//!
//! Converts payload bytes to ASCII uppercase.
//! Empty payloads pass through unchanged (zero-length output).
//! Non-UTF8 payloads return a BadInput error.

wit_bindgen::generate!({
    path: "../../wit",
    world: "transform-node",
    generate_all,
});

use exports::wafer::pipeline::transform::{OutputMessage, ProcessError};
use wafer_plugin::{bad_input, output_from, payload_as_str};

struct Uppercase;

impl exports::wafer::pipeline::lifecycle::Guest for Uppercase {
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

impl exports::wafer::pipeline::transform::Guest for Uppercase {
    fn process(
        input: exports::wafer::pipeline::transform::Message,
    ) -> Result<
        exports::wafer::pipeline::transform::OutputMessage,
        exports::wafer::pipeline::transform::ProcessError,
    > {
        // Empty payload passes through as empty output
        if input.payload.size() == 0 {
            return Ok(output_from!(&input, Vec::new()));
        }

        // payload_as_str! reads all bytes and attempts UTF-8 conversion;
        // returns Err(bad_input!("payload is not valid UTF-8")) on failure
        let text = payload_as_str!(&input)?;
        let uppercased = text.to_ascii_uppercase();
        Ok(output_from!(&input, uppercased.into_bytes()))
    }
}

export!(Uppercase);
