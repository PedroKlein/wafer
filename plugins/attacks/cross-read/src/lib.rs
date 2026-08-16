//! Attack scenario S2: Cross-read attempt (host memory access).
//! Expected runtime behavior: Trap (out-of-bounds memory access) — linear memory is isolated.

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
        // Attempt to read from a fabricated host memory address
        let host_addr: *const u8 = 0xDEAD_BEEF_usize as *const u8;
        let _stolen_byte = unsafe { core::ptr::read(host_addr) };
        unreachable!()
    }
}

export!(AttackPlugin);
