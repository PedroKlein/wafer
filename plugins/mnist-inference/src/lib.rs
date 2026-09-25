wit_bindgen::generate!({
    path: "../../wit",
    world: "inference-node",
    generate_all,
});

use exports::wafer::pipeline::transform::{OutputMessage, ProcessError};
use serde::Deserialize;
use wafer::pipeline::types::LogLevel;
use wafer_plugin::{bad_input, define_state, payload_bytes, set_state, with_state};
use wasi::nn::graph::{self, ExecutionTarget, GraphEncoding};
use wasi::nn::inference::GraphExecutionContext;
use wasi::nn::tensor::{Tensor, TensorType};

const MODEL_BYTES: &[u8] = include_bytes!("../../../models/mnist-8.onnx");
const PIXEL_COUNT: usize = 28 * 28;
const TENSOR_SIZE: usize = PIXEL_COUNT * size_of::<f32>();
const LOGIT_COUNT: usize = 10;
const LOGIT_SIZE: usize = LOGIT_COUNT * size_of::<f32>();
const INPUT_TENSOR_NAME: &str = "Input3";
const OUTPUT_TENSOR_NAME: &str = "Plus214_Output_0";

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct InferenceConfig {
    execution_target: Target,
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self { execution_target: Target::Cpu }
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Target {
    Cpu,
    Gpu,
    Tpu,
}

impl Target {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Gpu => "gpu",
            Self::Tpu => "tpu",
        }
    }
}

impl From<Target> for ExecutionTarget {
    fn from(target: Target) -> Self {
        match target {
            Target::Cpu => Self::Cpu,
            Target::Gpu => Self::Gpu,
            Target::Tpu => Self::Tpu,
        }
    }
}

struct InferenceState {
    context: GraphExecutionContext,
    requested_target: &'static str,
    plugin_version: String,
}

define_state!(InferenceState);

struct MnistInference;

impl exports::wafer::pipeline::lifecycle::Guest for MnistInference {
    fn validate(config: exports::wafer::pipeline::lifecycle::NodeConfig) -> Option<String> {
        wafer_plugin::parse_config::<InferenceConfig>(&config.config).err()
    }

    fn init(
        config: exports::wafer::pipeline::lifecycle::NodeConfig,
    ) -> Result<(), exports::wafer::pipeline::lifecycle::ProcessError> {
        let inference_config = wafer_plugin::parse_config::<InferenceConfig>(&config.config)
            .map_err(ProcessError::BadInput)?;
        let requested_target = inference_config.execution_target.as_str();
        wafer::pipeline::logging::log(
            LogLevel::Info,
            &format!("requested execution target: {requested_target}"),
        );
        let graph = graph::load(
            &[MODEL_BYTES.to_vec()],
            GraphEncoding::Onnx,
            inference_config.execution_target.into(),
        )
        .map_err(|error| ProcessError::DependencyFailed(format_nn_error("model load", &error)))?;
        let context = graph.init_execution_context().map_err(|error| {
            ProcessError::DependencyFailed(format_nn_error("execution context", &error))
        })?;
        set_state!(InferenceState {
            context,
            requested_target,
            plugin_version: config.plugin_version,
        });
        Ok(())
    }

    fn close() {
        __WAFER_STATE.with(|cell| *cell.borrow_mut() = None);
    }
}

impl exports::wafer::pipeline::transform::Guest for MnistInference {
    fn process(
        input: exports::wafer::pipeline::transform::Message,
    ) -> Result<OutputMessage, ProcessError> {
        let bytes = payload_bytes!(&input);
        let tensor_data = prepare_tensor(&bytes)?;
        let input_tensor = Tensor::new(&[1, 1, 28, 28], TensorType::Fp32, &tensor_data);

        with_state!(state => {
            let outputs = state
                .context
                .compute(vec![(INPUT_TENSOR_NAME.to_string(), input_tensor)])
                .map_err(|error| {
                    ProcessError::ProcessingFailed(format_nn_error("inference", &error))
                })?;
            let output = outputs
                .iter()
                .find(|(name, _)| name == OUTPUT_TENSOR_NAME)
                .or_else(|| outputs.first())
                .ok_or_else(|| ProcessError::ProcessingFailed("inference returned no output".into()))?;
            let logits = output.1.data();
            validate_logits(&logits)?;
            let mut metadata = input.metadata.clone();
            metadata.retain(|(key, _)| {
                key != "inference.execution_target.requested" && key != "plugin.version"
            });
            metadata.push((
                "inference.execution_target.requested".into(),
                state.requested_target.into(),
            ));
            metadata.push(("plugin.version".into(), state.plugin_version.clone()));
            Ok(OutputMessage {
                id: input.id.clone(),
                timestamp: input.timestamp,
                source: input.source.clone(),
                content_type: "application/vnd.wafer.mnist-logits".into(),
                metadata,
                payload: logits,
            })
        })
    }
}

fn prepare_tensor(bytes: &[u8]) -> Result<Vec<u8>, ProcessError> {
    match bytes.len() {
        PIXEL_COUNT => {
            Ok(bytes.iter().flat_map(|pixel| (f32::from(*pixel) / 255.0).to_le_bytes()).collect())
        }
        TENSOR_SIZE => Ok(bytes.to_vec()),
        size => Err(bad_input!(format!(
            "expected {PIXEL_COUNT} grayscale bytes or {TENSOR_SIZE} f32 tensor bytes, got {size}"
        ))),
    }
}

fn validate_logits(bytes: &[u8]) -> Result<(), ProcessError> {
    if bytes.len() != LOGIT_SIZE {
        return Err(ProcessError::ProcessingFailed(format!(
            "expected {LOGIT_COUNT} f32 logits, got {} bytes",
            bytes.len()
        )));
    }
    if bytes
        .as_chunks::<{ size_of::<f32>() }>()
        .0
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .any(|logit| !logit.is_finite())
    {
        return Err(ProcessError::ProcessingFailed("inference returned non-finite logits".into()));
    }
    Ok(())
}

fn format_nn_error(context: &str, error: &wasi::nn::errors::Error) -> String {
    format!("{context}: {:?}: {}", error.code(), error.data())
}

export!(MnistInference);
