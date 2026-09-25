wit_bindgen::generate!({
    path: "../../../../../wit",
    world: "inference-node",
    generate_all,
});

struct InferenceTest;

impl exports::wafer::pipeline::lifecycle::Guest for InferenceTest {
    fn validate(_config: exports::wafer::pipeline::lifecycle::NodeConfig) -> Option<String> {
        None
    }

    fn init(
        _config: exports::wafer::pipeline::lifecycle::NodeConfig,
    ) -> Result<(), exports::wafer::pipeline::lifecycle::ProcessError> {
        Ok(())
    }

    fn close() {}
}

impl exports::wafer::pipeline::transform::Guest for InferenceTest {
    fn process(
        input: exports::wafer::pipeline::transform::Message,
    ) -> Result<
        exports::wafer::pipeline::transform::OutputMessage,
        exports::wafer::pipeline::transform::ProcessError,
    > {
        let _ = wasi::nn::graph::load(
            &[],
            wasi::nn::graph::GraphEncoding::Onnx,
            wasi::nn::graph::ExecutionTarget::Cpu,
        );
        Ok(exports::wafer::pipeline::transform::OutputMessage {
            id: input.id,
            timestamp: input.timestamp,
            source: input.source,
            content_type: input.content_type,
            metadata: input.metadata,
            payload: input.payload.read_all(),
        })
    }
}

export!(InferenceTest);
