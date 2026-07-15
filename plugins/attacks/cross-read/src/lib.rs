//! Attack scenario S2: Cross-read attempt (host memory access).
//! Expected runtime behavior: Trap (out-of-bounds memory access) — linear memory is isolated.

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
        // Attempt to read from a fabricated host memory address
        let host_addr: *const u8 = 0xDEAD_BEEF_usize as *const u8;
        let _stolen_byte = unsafe { core::ptr::read(host_addr) };
        unreachable!()
    }
}

export!(AttackPlugin);
