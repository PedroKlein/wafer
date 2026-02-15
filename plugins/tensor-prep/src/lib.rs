//! Tensor prep: normalizes 28x28 grayscale (784 bytes) to F32 tensor (3136 bytes, little-endian)

wit_bindgen::generate!({
    path: "wit",
    world: "transform-node",
});

const INPUT_SIZE: usize = 28 * 28;
const OUTPUT_SIZE: usize = INPUT_SIZE * 4;

struct TensorPrep;

impl exports::pipeline::transform::lifecycle::Guest for TensorPrep {
    fn validate(_config: exports::pipeline::transform::lifecycle::NodeConfig) -> Option<String> {
        None
    }

    fn init(_config: exports::pipeline::transform::lifecycle::NodeConfig) -> Result<(), String> {
        Ok(())
    }

    fn close() {}
}

impl exports::pipeline::transform::transform::Guest for TensorPrep {
    fn process(
        input: pipeline::transform::types::Envelope,
    ) -> pipeline::transform::types::ProcessResult {
        use pipeline::transform::types::{Envelope, Payload, ProcessError, ProcessResult};

        let Payload::Raw(bytes) = input.payload;

        if bytes.len() != INPUT_SIZE {
            return ProcessResult::Error(ProcessError {
                code: "INVALID_INPUT_SIZE".to_string(),
                message: format!(
                    "Expected {} bytes (28x28 grayscale), got {} bytes",
                    INPUT_SIZE,
                    bytes.len()
                ),
                retriable: false,
            });
        }

        let tensor_bytes: Vec<u8> = bytes
            .iter()
            .flat_map(|&b| (b as f32 / 255.0).to_le_bytes())
            .collect();

        debug_assert_eq!(tensor_bytes.len(), OUTPUT_SIZE);

        ProcessResult::Emit(Envelope {
            payload: Payload::Raw(tensor_bytes),
            ..input
        })
    }
}

export!(TensorPrep);

#[cfg(test)]
mod tests {
    use super::*;

    fn normalize_to_tensor(bytes: &[u8]) -> Vec<u8> {
        bytes
            .iter()
            .flat_map(|&b| (b as f32 / 255.0).to_le_bytes())
            .collect()
    }

    #[test]
    fn test_784_zeros_to_3136_bytes_all_zero_f32() {
        let input = vec![0u8; INPUT_SIZE];
        let output = normalize_to_tensor(&input);

        assert_eq!(output.len(), OUTPUT_SIZE);
        for chunk in output.chunks(4) {
            let value = f32::from_le_bytes(chunk.try_into().unwrap());
            assert_eq!(value, 0.0);
        }
    }

    #[test]
    fn test_784_bytes_255_to_3136_bytes_all_one_f32() {
        let input = vec![255u8; INPUT_SIZE];
        let output = normalize_to_tensor(&input);

        assert_eq!(output.len(), OUTPUT_SIZE);
        for chunk in output.chunks(4) {
            let value = f32::from_le_bytes(chunk.try_into().unwrap());
            assert_eq!(value, 1.0);
        }
    }

    #[test]
    fn test_midpoint_127_normalizes_to_approximately_half() {
        let input = vec![127u8; INPUT_SIZE];
        let output = normalize_to_tensor(&input);

        assert_eq!(output.len(), OUTPUT_SIZE);
        for chunk in output.chunks(4) {
            let value = f32::from_le_bytes(chunk.try_into().unwrap());
            assert!((value - 127.0 / 255.0).abs() < 0.001);
        }
    }

    #[test]
    fn test_output_is_exactly_3136_bytes() {
        let input = vec![128u8; INPUT_SIZE];
        let output = normalize_to_tensor(&input);
        assert_eq!(output.len(), OUTPUT_SIZE);
        assert_eq!(output.len(), 3136);
    }
}
