//! Attack scenario S5: Filesystem access attempt.
//! Expected runtime behavior: Trap or error — no WASI FS capabilities granted.

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
        // Attempt to read /etc/passwd — should fail without FS capability grants
        let content = std::fs::read_to_string("/etc/passwd")
            .unwrap_or_else(|_| "access denied".to_string());
        // If we somehow got here, leak the content (should never happen)
        panic!("FS access succeeded unexpectedly: {} bytes", content.len());
    }
}

export!(AttackPlugin);
