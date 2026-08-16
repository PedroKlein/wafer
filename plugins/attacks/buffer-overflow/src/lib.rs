//! Attack scenario S1: Buffer overflow attempt.
//! Expected runtime behavior: Trap (out-of-bounds memory access).

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
