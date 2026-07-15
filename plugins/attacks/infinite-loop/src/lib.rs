//! Attack scenario S3: Infinite loop (CPU exhaustion).
//! Expected runtime behavior: Epoch interrupt → Trap (interrupted/timed-out).

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
        loop {}
    }
}

export!(AttackPlugin);
