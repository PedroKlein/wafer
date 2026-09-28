//! Attack scenario S1: Buffer overflow attempt.
//! Writes one byte just past the end of linear memory.
//! Expected runtime behavior: out-of-bounds memory access trap.

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
        let end = core::arch::wasm32::memory_size(0) * 65_536;
        unsafe { core::ptr::write_volatile(core::ptr::without_provenance_mut::<u8>(end), 0xFF) };
        Err(exports::wafer::pipeline::transform::ProcessError::ProcessingFailed(format!(
            "NOT CONTAINED: wrote past the end of linear memory at {end:#x}"
        )))
    }
}

export!(AttackPlugin);
