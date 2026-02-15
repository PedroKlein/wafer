//! Result format plugin for WAFER inference output parsing.
//!
//! This plugin converts 40 bytes (10 F32 little-endian logits) to JSON output.
//! Applies softmax to convert logits to probabilities, finds argmax for predicted digit.
//!
//! Output format: {"digit": N, "confidence": 0.XX, "all_scores": [...]}

wit_bindgen::generate!({
    path: "wit",
    world: "transform-node",
});

struct ResultFormat;

/// Apply numerically stable softmax to logits
fn softmax(logits: &[f32]) -> Vec<f32> {
    let max = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = logits.iter().map(|&x| (x - max).exp()).collect();
    let sum: f32 = exps.iter().sum();
    exps.iter().map(|&e| e / sum).collect()
}

/// Find index of maximum value (argmax)
fn argmax(values: &[f32]) -> usize {
    values
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(idx, _)| idx)
        .unwrap_or(0)
}

/// Parse 40 bytes as 10 little-endian F32 values
fn parse_logits(bytes: &[u8]) -> Option<Vec<f32>> {
    if bytes.len() != 40 {
        return None;
    }

    let mut logits = Vec::with_capacity(10);
    for i in 0..10 {
        let start = i * 4;
        let bytes_slice: [u8; 4] = bytes[start..start + 4].try_into().ok()?;
        logits.push(f32::from_le_bytes(bytes_slice));
    }
    Some(logits)
}

/// Format f32 with 4 decimal places, removing trailing zeros
fn format_f32(value: f32) -> String {
    let formatted = format!("{:.4}", value);
    // Trim trailing zeros after decimal point, but keep at least one digit after decimal
    let trimmed = formatted.trim_end_matches('0');
    if trimmed.ends_with('.') {
        format!("{}0", trimmed)
    } else {
        trimmed.to_string()
    }
}

fn format_json(digit: usize, confidence: f32, all_scores: &[f32]) -> String {
    let scores_str: Vec<String> = all_scores.iter().map(|&s| format_f32(s)).collect();
    format!(
        r#"{{"digit": {}, "confidence": {}, "all_scores": [{}]}}"#,
        digit,
        format_f32(confidence),
        scores_str.join(", ")
    )
}

impl exports::pipeline::transform::lifecycle::Guest for ResultFormat {
    /// Validate configuration - result-format has no config, always valid
    fn validate(_config: exports::pipeline::transform::lifecycle::NodeConfig) -> Option<String> {
        None
    }

    /// Initialize the node - result-format needs no initialization
    fn init(_config: exports::pipeline::transform::lifecycle::NodeConfig) -> Result<(), String> {
        Ok(())
    }

    /// Graceful shutdown - result-format has nothing to clean up
    fn close() {
        // No resources to release
    }
}

impl exports::pipeline::transform::transform::Guest for ResultFormat {
    /// Process a message - convert 40 bytes of logits to JSON output
    fn process(
        input: pipeline::transform::types::Envelope,
    ) -> pipeline::transform::types::ProcessResult {
        use pipeline::transform::types::{Envelope, Payload, ProcessError, ProcessResult};

        let Payload::Raw(bytes) = input.payload;

        // Validate input is exactly 40 bytes
        if bytes.len() != 40 {
            return ProcessResult::Error(ProcessError {
                code: "INVALID_INPUT_SIZE".to_string(),
                message: format!(
                    "Expected exactly 40 bytes (10 F32 logits), got {} bytes",
                    bytes.len()
                ),
                retriable: false,
            });
        }

        // Parse bytes as F32 logits
        let logits = match parse_logits(&bytes) {
            Some(l) => l,
            None => {
                return ProcessResult::Error(ProcessError {
                    code: "PARSE_ERROR".to_string(),
                    message: "Failed to parse bytes as F32 logits".to_string(),
                    retriable: false,
                });
            }
        };

        // Apply softmax to get probabilities
        let probabilities = softmax(&logits);

        // Find predicted digit (argmax)
        let digit = argmax(&probabilities);
        let confidence = probabilities[digit];

        // Format as JSON
        let json_output = format_json(digit, confidence, &probabilities);

        ProcessResult::Emit(Envelope {
            payload: Payload::Raw(json_output.into_bytes()),
            ..input
        })
    }
}

export!(ResultFormat);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_softmax_basic() {
        let logits = vec![1.0, 2.0, 3.0];
        let probs = softmax(&logits);

        // Sum should be approximately 1.0
        let sum: f32 = probs.iter().sum();
        assert!(
            (sum - 1.0).abs() < 1e-6,
            "Softmax sum should be 1.0, got {}",
            sum
        );

        // Higher logit should have higher probability
        assert!(probs[2] > probs[1], "Higher logit should have higher prob");
        assert!(probs[1] > probs[0], "Higher logit should have higher prob");
    }

    #[test]
    fn test_softmax_numerical_stability() {
        // Large values that would overflow without numerical stability
        let logits = vec![1000.0, 1001.0, 1002.0];
        let probs = softmax(&logits);

        let sum: f32 = probs.iter().sum();
        assert!(
            (sum - 1.0).abs() < 1e-6,
            "Softmax should handle large values"
        );
        assert!(
            probs.iter().all(|&p| p.is_finite()),
            "All probs should be finite"
        );
    }

    #[test]
    fn test_softmax_equal_logits() {
        let logits = vec![1.0, 1.0, 1.0, 1.0, 1.0];
        let probs = softmax(&logits);

        // All probabilities should be equal (1/5 = 0.2)
        for prob in &probs {
            assert!(
                (prob - 0.2).abs() < 1e-6,
                "Equal logits should give equal probs"
            );
        }
    }

    #[test]
    fn test_argmax() {
        assert_eq!(argmax(&[0.1, 0.2, 0.7]), 2);
        assert_eq!(argmax(&[0.9, 0.05, 0.05]), 0);
        assert_eq!(argmax(&[0.1, 0.8, 0.1]), 1);
    }

    #[test]
    fn test_parse_logits_valid() {
        // Create 40 bytes representing 10 F32 values
        let mut bytes = Vec::with_capacity(40);
        for i in 0..10 {
            let value = i as f32;
            bytes.extend_from_slice(&value.to_le_bytes());
        }

        let logits = parse_logits(&bytes).unwrap();
        assert_eq!(logits.len(), 10);
        for (i, &logit) in logits.iter().enumerate() {
            assert_eq!(logit, i as f32);
        }
    }

    #[test]
    fn test_parse_logits_invalid_size() {
        assert!(parse_logits(&[0u8; 39]).is_none(), "39 bytes should fail");
        assert!(parse_logits(&[0u8; 41]).is_none(), "41 bytes should fail");
        assert!(parse_logits(&[]).is_none(), "Empty should fail");
    }

    #[test]
    fn test_format_f32() {
        assert_eq!(format_f32(0.9823), "0.9823");
        assert_eq!(format_f32(0.1), "0.1");
        assert_eq!(format_f32(1.0), "1.0");
        assert_eq!(format_f32(0.0), "0.0");
        assert_eq!(format_f32(0.12345), "0.1235"); // Rounds to 4 places
    }

    #[test]
    fn test_format_json() {
        let json = format_json(
            7,
            0.9823,
            &[
                0.001, 0.002, 0.003, 0.001, 0.002, 0.001, 0.002, 0.9823, 0.003, 0.002,
            ],
        );

        assert!(json.contains("\"digit\": 7"));
        assert!(json.contains("\"confidence\": 0.9823"));
        assert!(json.contains("\"all_scores\":"));
        // Should have 10 values
        assert!(json.contains("[0.001"));
        assert!(json.contains("0.002]"));
    }

    #[test]
    fn test_end_to_end() {
        // Create logits where digit 7 has highest value
        let mut logits_f32 = vec![0.0f32; 10];
        logits_f32[7] = 10.0; // High logit for digit 7

        // Convert to bytes
        let mut bytes = Vec::with_capacity(40);
        for logit in &logits_f32 {
            bytes.extend_from_slice(&logit.to_le_bytes());
        }

        // Parse and process
        let parsed = parse_logits(&bytes).unwrap();
        let probs = softmax(&parsed);
        let digit = argmax(&probs);

        assert_eq!(digit, 7, "Should predict digit 7");
        assert!(probs[7] > 0.99, "Digit 7 should have very high confidence");
    }
}
