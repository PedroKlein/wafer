//! Attack scenario S4: Memory exhaustion.
//! Expected runtime behavior: Trap (memory limit exceeded via StoreLimits).

wit_bindgen::generate!({
    path: "../../../wit/node",
    world: "transform-node",
    generate_all,
});

struct AttackPlugin;

impl exports::pipeline::node::lifecycle::Guest for AttackPlugin {
    fn validate(_config: exports::pipeline::node::lifecycle::NodeConfig) -> Option<String> { None }
    fn init(_config: exports::pipeline::node::lifecycle::NodeConfig) -> Result<(), exports::pipeline::node::lifecycle::ProcessError> { Ok(()) }
    fn close() {}
}

impl exports::pipeline::node::transform::Guest for AttackPlugin {
    fn process(
        _input: exports::pipeline::node::transform::Message,
    ) -> Result<exports::pipeline::node::transform::OutputMessage, exports::pipeline::node::transform::ProcessError> {
        // Allocate 1MB chunks in a loop until the memory limit is exceeded
        let mut hoard: Vec<Vec<u8>> = Vec::new();
        loop {
            hoard.push(vec![0u8; 1_048_576]);
        }
    }
}

export!(AttackPlugin);
