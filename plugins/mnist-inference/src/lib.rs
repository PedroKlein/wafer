//! MNIST inference plugin using wasi-nn
//!
//! Input: 3136 bytes (784 F32 values, little-endian) from tensor-prep
//! Output: 40 bytes (10 F32 logits, little-endian) for result-format
//!
//! ## Configuration
//!
//! The plugin accepts an optional `execution_target` parameter in the node config:
//!
//! ```toml
//! [nodes.config]
//! plugin_path = "..."
//! execution_target = "auto"  # "auto" (default), "cpu", "gpu", or "tpu"
//! ```
//!
//! - `"auto"`: Tries GPU first, falls back to CPU (logs which was selected)
//! - `"cpu"`: Force CPU execution
//! - `"gpu"`, `"tpu"`: Request GPU/TPU execution. Note: The ONNX backend may
//!   silently fall back to CPU if the hardware is unavailable. Check runtime
//!   logs for the actual execution provider used.
//!
//! TODO: Node configuration should be more generic. In the future, plugins should
//! bundle their configuration schema (e.g., JSON Schema or WIT-defined types) so
//! the host can validate configuration before passing to the plugin, provide
//! configuration discovery/documentation, and enable tooling to generate
//! configuration UI/docs.

wit_bindgen::generate!({
    path: "../../wit",
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

/// Wrapper for GraphExecutionContext to allow static storage.
///
/// # Safety
/// This uses UnsafeCell because WASM execution is single-threaded.
/// The static mut pattern is safe in WASM since there are no concurrent
/// threads that could cause data races. The UnsafeCell allows interior
/// mutability for the static variable.
struct ContextHolder(UnsafeCell<Option<GraphExecutionContext>>);

// SAFETY: WASM is single-threaded, so Sync is safe to implement
unsafe impl Sync for ContextHolder {}

static EXECUTION_CONTEXT: ContextHolder = ContextHolder(UnsafeCell::new(None));

/// Configured execution target for inference
#[derive(Default, Clone, Copy)]
enum ExecutionTargetConfig {
    /// Auto-detect: Try TPU -> GPU -> CPU with fallback
    #[default]
    Auto,
    /// Force CPU execution
    Cpu,
    /// Force GPU execution (fails if unavailable)
    Gpu,
    /// Force TPU execution (fails if unavailable)
    Tpu,
}

/// Parse the execution_target from node configuration.
///
/// Returns `ExecutionTargetConfig::Auto` if:
/// - config_bytes is empty
/// - config_bytes is not valid UTF-8
/// - config_bytes is not valid TOML
/// - execution_target key is missing
fn parse_execution_target(
    config: &exports::pipeline::transform::lifecycle::NodeConfig,
) -> ExecutionTargetConfig {
    if config.config_bytes.is_empty() {
        return ExecutionTargetConfig::Auto;
    }

    let Ok(config_str) = String::from_utf8(config.config_bytes.clone()) else {
        eprintln!("[mnist-inference] Config bytes not valid UTF-8, defaulting to auto");
        return ExecutionTargetConfig::Auto;
    };

    let Ok(parsed) = config_str.parse::<toml::Value>() else {
        eprintln!("[mnist-inference] Config not valid TOML, defaulting to auto");
        return ExecutionTargetConfig::Auto;
    };

    match parsed.get("execution_target").and_then(|v| v.as_str()) {
        Some("cpu") => ExecutionTargetConfig::Cpu,
        Some("gpu") => ExecutionTargetConfig::Gpu,
        Some("tpu") => ExecutionTargetConfig::Tpu,
        Some("auto") | None => ExecutionTargetConfig::Auto,
        Some(other) => {
            eprintln!(
                "[mnist-inference] Unknown execution_target '{}', defaulting to auto",
                other
            );
            ExecutionTargetConfig::Auto
        }
    }
}

/// Attempt to load the model and create an execution context with a specific target.
fn try_load_with_target(target: ExecutionTarget) -> Result<GraphExecutionContext, String> {
    let graph = graph::load(&[MODEL_BYTES.to_vec()], GraphEncoding::Onnx, target)
        .map_err(|e| format_nn_error("Failed to load model", &e))?;

    graph
        .init_execution_context()
        .map_err(|e| format_nn_error("Failed to create execution context", &e))
}

struct MnistInference;

impl exports::pipeline::transform::lifecycle::Guest for MnistInference {
    fn validate(config: exports::pipeline::transform::lifecycle::NodeConfig) -> Option<String> {
        // Validate execution_target if provided
        if !config.config_bytes.is_empty() {
            if let Ok(config_str) = String::from_utf8(config.config_bytes.clone()) {
                if let Ok(parsed) = config_str.parse::<toml::Value>() {
                    if let Some(target) = parsed.get("execution_target") {
                        match target.as_str() {
                            Some("auto" | "cpu" | "gpu" | "tpu") => {}
                            Some(other) => {
                                return Some(format!(
                                    "Invalid execution_target '{}': must be 'auto', 'cpu', 'gpu', or 'tpu'",
                                    other
                                ));
                            }
                            None => {
                                return Some("execution_target must be a string".to_string());
                            }
                        }
                    }
                }
            }
        }
        None
    }

    fn init(config: exports::pipeline::transform::lifecycle::NodeConfig) -> Result<(), String> {
        let target_config = parse_execution_target(&config);

        // Device selection logic - runs ONCE during init
        //
        // NOTE: The wasmtime-wasi-nn ONNX backend silently falls back to CPU when
        // the requested execution target (GPU/TPU) is not available. This means
        // we cannot reliably detect if GPU/TPU is actually being used vs falling
        // back to CPU. For "auto" mode, we try GPU first (most likely to be useful),
        // then CPU. The underlying runtime will log if a fallback occurs.
        let (context, device_name) = match target_config {
            ExecutionTargetConfig::Auto => {
                // Auto mode: Try GPU first, fall back to CPU
                // The ONNX runtime will internally fall back to CPU if GPU isn't available,
                // and will log a warning when it does so.
                eprintln!(
                    "[mnist-inference] Auto-detecting execution target (trying GPU, then CPU)..."
                );

                // Try GPU first - if CUDA/CoreML is available, it will be used
                // If not, the ONNX runtime will silently fall back to CPU
                match try_load_with_target(ExecutionTarget::Gpu) {
                    Ok(ctx) => {
                        eprintln!("[mnist-inference] Requested GPU execution (check runtime logs for actual provider)");
                        (ctx, "GPU (may fall back to CPU)")
                    }
                    Err(_) => {
                        eprintln!("[mnist-inference] GPU load failed, using CPU explicitly");
                        (try_load_with_target(ExecutionTarget::Cpu)?, "CPU")
                    }
                }
            }
            ExecutionTargetConfig::Cpu => {
                eprintln!("[mnist-inference] CPU execution target requested");
                (try_load_with_target(ExecutionTarget::Cpu)?, "CPU")
            }
            ExecutionTargetConfig::Gpu => {
                eprintln!("[mnist-inference] GPU execution target requested");
                // NOTE: The ONNX backend silently falls back to CPU if GPU is unavailable.
                // We cannot detect this at the wasi-nn level, so we warn the user to check
                // runtime logs for the actual execution provider.
                (
                    try_load_with_target(ExecutionTarget::Gpu).map_err(|e| {
                        format!("GPU execution requested but failed to load: {}", e)
                    })?,
                    "GPU (check runtime logs - may fall back to CPU)",
                )
            }
            ExecutionTargetConfig::Tpu => {
                eprintln!("[mnist-inference] TPU execution target requested");
                // NOTE: TPU is not yet supported by the ONNX backend and will fall back to CPU.
                // We cannot detect this at the wasi-nn level.
                (
                    try_load_with_target(ExecutionTarget::Tpu).map_err(|e| {
                        format!("TPU execution requested but failed to load: {}", e)
                    })?,
                    "TPU (check runtime logs - may fall back to CPU)",
                )
            }
        };

        // Log the final selection
        eprintln!(
            "[mnist-inference] Initialized with {} execution provider",
            device_name
        );

        // Store context for use by process()
        // SAFETY: WASM is single-threaded, no data races possible.
        // This is the standard pattern for static mutable state in WASM plugins.
        unsafe {
            *EXECUTION_CONTEXT.0.get() = Some(context);
        }

        Ok(())
    }

    fn close() {
        // SAFETY: WASM is single-threaded, no data races possible
        unsafe {
            *EXECUTION_CONTEXT.0.get() = None;
        }
    }
}

impl exports::pipeline::transform::transform::Guest for MnistInference {
    fn process(
        input: pipeline::transform::types::Envelope,
    ) -> pipeline::transform::types::ProcessResult {
        use pipeline::transform::types::{Envelope, Payload, ProcessError, ProcessResult};

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

        // SAFETY: WASM is single-threaded, init() must be called before process()
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
