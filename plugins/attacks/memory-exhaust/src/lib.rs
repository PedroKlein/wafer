//! Attack scenario S4: Memory exhaustion.
//! Expected runtime behavior: Trap (memory limit exceeded via StoreLimits).

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
        // Allocate 1MB chunks in a loop until the memory limit is exceeded
        let mut hoard: Vec<Vec<u8>> = Vec::new();
        loop {
            hoard.push(vec![0u8; 1_048_576]);
        }
    }
}

export!(AttackPlugin);
