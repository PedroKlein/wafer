//! MNIST inference plugin using wasi-nn
//!
//! Input: 3136 bytes (784 F32 values, little-endian) from tensor-prep
//! Output: 40 bytes (10 F32 logits, little-endian) for result-format

wit_bindgen::generate!({
    path: "wit",
    world: "inference-node",
    generate_all,
});

use core::cell::UnsafeCell;
use wasi::nn::{
    errors::Error as NnError,
    graph::{self, ExecutionTarget, GraphEncoding},
    inference::GraphExecutionContext,
    tensor::{Tensor, TensorType},
};

static MODEL_BYTES: &[u8] = include_bytes!("../../../models/mnist-8.onnx");

const INPUT_SIZE: usize = 784 * 4;
const OUTPUT_SIZE: usize = 10 * 4;

// MNIST-8 model tensor names (from ONNX model definition)
const INPUT_TENSOR_NAME: &str = "Input3";
const OUTPUT_TENSOR_NAME: &str = "Plus214_Output_0";

struct ContextHolder(UnsafeCell<Option<GraphExecutionContext>>);
unsafe impl Sync for ContextHolder {}
static EXECUTION_CONTEXT: ContextHolder = ContextHolder(UnsafeCell::new(None));

struct MnistInference;

impl exports::pipeline::inference::lifecycle::Guest for MnistInference {
    fn validate(_config: exports::pipeline::inference::lifecycle::NodeConfig) -> Option<String> {
        None
    }

    fn init(_config: exports::pipeline::inference::lifecycle::NodeConfig) -> Result<(), String> {
        let graph = graph::load(
            &[MODEL_BYTES.to_vec()],
            GraphEncoding::Onnx,
            ExecutionTarget::Cpu,
        )
        .map_err(|e| format_nn_error("Failed to load model", &e))?;

        let context = graph
            .init_execution_context()
            .map_err(|e| format_nn_error("Failed to create execution context", &e))?;

        unsafe {
            *EXECUTION_CONTEXT.0.get() = Some(context);
        }

        Ok(())
    }

    fn close() {
        unsafe {
            *EXECUTION_CONTEXT.0.get() = None;
        }
    }
}

impl exports::pipeline::inference::transform::Guest for MnistInference {
    fn process(
        input: pipeline::inference::types::Envelope,
    ) -> pipeline::inference::types::ProcessResult {
        use pipeline::inference::types::{Envelope, Payload, ProcessError, ProcessResult};

        let Payload::Raw(bytes) = input.payload;

        if bytes.len() != INPUT_SIZE {
            return ProcessResult::Error(ProcessError {
                code: "INVALID_INPUT_SIZE".to_string(),
                message: format!(
                    "Expected {} bytes (784 F32 values), got {} bytes",
                    INPUT_SIZE,
                    bytes.len()
                ),
                retriable: false,
            });
        }

        let context = unsafe {
            match (*EXECUTION_CONTEXT.0.get()).as_ref() {
                Some(ctx) => ctx,
                None => {
                    return ProcessResult::Error(ProcessError {
                        code: "NOT_INITIALIZED".to_string(),
                        message: "Inference context not initialized. Call init() first."
                            .to_string(),
                        retriable: false,
                    });
                }
            }
        };

        let input_tensor = Tensor::new(&[1, 1, 28, 28], TensorType::Fp32, &bytes);
        let inputs = vec![(INPUT_TENSOR_NAME.to_string(), input_tensor)];

        let outputs = match context.compute(inputs) {
            Ok(out) => out,
            Err(e) => {
                return ProcessResult::Error(ProcessError {
                    code: "INFERENCE_ERROR".to_string(),
                    message: format_nn_error("Inference failed", &e),
                    retriable: true,
                });
            }
        };

        let output_data = match outputs.iter().find(|(name, _)| name == OUTPUT_TENSOR_NAME) {
            Some((_, tensor)) => tensor.data(),
            None => {
                if let Some((_, tensor)) = outputs.first() {
                    tensor.data()
                } else {
                    return ProcessResult::Error(ProcessError {
                        code: "NO_OUTPUT".to_string(),
                        message: "No output tensor returned from inference".to_string(),
                        retriable: false,
                    });
                }
            }
        };

        if output_data.len() != OUTPUT_SIZE {
            return ProcessResult::Error(ProcessError {
                code: "INVALID_OUTPUT_SIZE".to_string(),
                message: format!(
                    "Expected {} bytes (10 F32 values), got {} bytes",
                    OUTPUT_SIZE,
                    output_data.len()
                ),
                retriable: false,
            });
        }

        ProcessResult::Emit(Envelope {
            payload: Payload::Raw(output_data),
            ..input
        })
    }
}

fn format_nn_error(context: &str, error: &NnError) -> String {
    format!("{}: {:?} - {}", context, error.code(), error.data())
}

export!(MnistInference);
