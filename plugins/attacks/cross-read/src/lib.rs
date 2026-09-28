//! Attack scenario S2: Cross-read attempt (host memory access).
//! Expected runtime behavior: out-of-bounds memory access trap, since linear memory is isolated.

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
        let host_addr = core::ptr::without_provenance::<u8>(0xDEAD_BEEF);
        let stolen = unsafe { core::ptr::read_volatile(host_addr) };
        Err(exports::wafer::pipeline::transform::ProcessError::ProcessingFailed(format!(
            "NOT CONTAINED: read {stolen:#04x} from {host_addr:p}"
        )))
    }
}

export!(AttackPlugin);
