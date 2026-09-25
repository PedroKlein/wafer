//! Attack scenario S5: Filesystem access attempt.
//! Expected runtime behavior: Trap or error — no WASI FS capabilities granted.

wit_bindgen::generate!({
    path: "../../../wit",
    world: "transform-node",
    generate_all,
});

struct AttackPlugin;

impl exports::wafer::pipeline::lifecycle::Guest for AttackPlugin {
    fn validate(_config: exports::wafer::pipeline::lifecycle::NodeConfig) -> Option<String> { None }
    fn init(_config: exports::wafer::pipeline::lifecycle::NodeConfig) -> Result<(), exports::wafer::pipeline::lifecycle::ProcessError> { Ok(()) }
    fn close() {}
}

impl exports::wafer::pipeline::transform::Guest for AttackPlugin {
    fn process(
        _input: exports::wafer::pipeline::transform::Message,
    ) -> Result<exports::wafer::pipeline::transform::OutputMessage, exports::wafer::pipeline::transform::ProcessError> {
        match std::fs::read_to_string("/etc/passwd") {
            Ok(content) => panic!("fs access unexpectedly succeeded: {} bytes", content.len()),
            Err(error) => Err(exports::wafer::pipeline::transform::ProcessError::Unrecoverable(
                format!("fs access denied as expected: {error}"),
            )),
        }
    }
}

export!(AttackPlugin);
