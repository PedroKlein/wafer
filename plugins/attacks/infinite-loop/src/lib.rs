//! Attack scenario S3: Infinite loop (CPU exhaustion).
//! Expected runtime behavior: Epoch interrupt → Trap (interrupted/timed-out).

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
        loop {}
    }
}

export!(AttackPlugin);
