//! Attack scenario S1: Buffer overflow attempt.
//! Expected runtime behavior: Trap (out-of-bounds memory access).

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
        // Attempt to write past allocation using unsafe pointer arithmetic
        let v: Vec<u8> = Vec::with_capacity(16);
        let ptr = v.as_ptr() as *mut u8;
        unsafe {
            // Write far beyond the allocated capacity into unmapped linear memory
            core::ptr::write(ptr.add(1_000_000), 0xFF);
        }
        unreachable!()
    }
}

export!(AttackPlugin);
