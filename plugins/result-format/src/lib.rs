//! Result format plugin for WAFER pipeline.
//!
//! Formats inference output (f32 bytes, little-endian) back to structured JSON
//! with configurable labels and confidence threshold.
//!
//! Config: { "labels": ["normal", "anomaly", "critical"], "threshold": 0.5 }
//! Output: { "prediction": "normal", "confidence": 0.92, "scores": [0.92, 0.05, 0.03], "above_threshold": true }

wit_bindgen::generate!({
    path: "../../wit/node",
    world: "transform-node",
    generate_all,
});

use exports::pipeline::node::transform::{OutputMessage, ProcessError};
use wafer_plugin::{bad_input, define_state, output_with_type, payload_bytes, set_state, with_state};

struct ResultFormatConfig {
    labels: Vec<String>,
    threshold: f64,
}

define_state!(ResultFormatConfig);

struct ResultFormat;

impl exports::pipeline::node::lifecycle::Guest for ResultFormat {
    fn validate(config: exports::pipeline::node::lifecycle::NodeConfig) -> Option<String> {
        match parse_result_config(&config.config) {
            Ok(_) => None,
            Err(e) => Some(e),
        }
    }

    fn init(
        config: exports::pipeline::node::lifecycle::NodeConfig,
    ) -> Result<(), exports::pipeline::node::lifecycle::ProcessError> {
        let cfg = parse_result_config(&config.config)
            .map_err(exports::pipeline::node::lifecycle::ProcessError::BadInput)?;
        set_state!(cfg);
        Ok(())
    }

    fn close() {}
}

impl exports::pipeline::node::transform::Guest for ResultFormat {
    fn process(
        input: exports::pipeline::node::transform::Message,
    ) -> Result<
        exports::pipeline::node::transform::OutputMessage,
        exports::pipeline::node::transform::ProcessError,
    > {
        let bytes = payload_bytes!(&input);

        if bytes.is_empty() {
            return Err(bad_input!("empty tensor"));
        }

        if bytes.len() % 4 != 0 {
            return Err(bad_input!("payload not aligned to f32"));
        }

        // Parse bytes as f32 array (little-endian)
        let num_values = bytes.len() / 4;
        let mut scores: Vec<f32> = Vec::with_capacity(num_values);
        for i in 0..num_values {
            let start = i * 4;
            let chunk: [u8; 4] = [bytes[start], bytes[start + 1], bytes[start + 2], bytes[start + 3]];
            scores.push(f32::from_le_bytes(chunk));
        }

        with_state!(cfg => {
            // Find max confidence (argmax)
            let (max_idx, max_val) = scores
                .iter()
                .enumerate()
                .fold((0usize, f32::NEG_INFINITY), |(best_i, best_v), (i, &v)| {
                    if v > best_v { (i, v) } else { (best_i, best_v) }
                });

            // Map index to label (use index string if out of range)
            let prediction = if max_idx < cfg.labels.len() {
                cfg.labels[max_idx].as_str()
            } else {
                "unknown"
            };

            let above_threshold = (max_val as f64) >= cfg.threshold;

            // Build JSON output manually
            let json_output = build_json_output(prediction, max_val, &scores, above_threshold);

            Ok(output_with_type!(
                &input,
                json_output.into_bytes(),
                "application/json"
            ))
        })
    }
}

// ---------------------------------------------------------------------------
// JSON output construction
// ---------------------------------------------------------------------------

fn build_json_output(prediction: &str, confidence: f32, scores: &[f32], above_threshold: bool) -> String {
    let mut out = String::with_capacity(128);
    out.push_str("{\"prediction\": \"");
    out.push_str(prediction);
    out.push_str("\", \"confidence\": ");
    write_f32(&mut out, confidence);
    out.push_str(", \"scores\": [");

    for (i, &score) in scores.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_f32(&mut out, score);
    }

    out.push_str("], \"above_threshold\": ");
    if above_threshold {
        out.push_str("true");
    } else {
        out.push_str("false");
    }
    out.push('}');
    out
}

/// Write f32 with up to 4 decimal places, trimming trailing zeros.
fn write_f32(out: &mut String, value: f32) {
    // Format to 4 decimal places
    let mut buf = [0u8; 20];
    let s = format_f32_fixed(value, &mut buf);
    out.push_str(&s);
}

/// Simple f32 formatter with 4 decimal places, trailing-zero trimmed.
fn format_f32_fixed(value: f32, _buf: &mut [u8; 20]) -> String {
    // Use integer arithmetic to avoid alloc-heavy format! in simple cases
    let negative = value < 0.0;
    let abs_val = if negative { -value } else { value };

    let integer_part = abs_val as u64;
    let frac = ((abs_val - integer_part as f32) * 10000.0 + 0.5) as u64;

    let mut result = String::with_capacity(12);
    if negative {
        result.push('-');
    }

    // Integer part
    push_u64(&mut result, integer_part);

    // Fractional part (up to 4 digits, trim trailing zeros)
    if frac == 0 {
        result.push_str(".0");
    } else {
        result.push('.');
        // Pad to 4 digits
        let frac_str = format!("{frac:04}");
        let trimmed = frac_str.trim_end_matches('0');
        result.push_str(trimmed);
    }

    result
}

fn push_u64(out: &mut String, mut n: u64) {
    if n == 0 {
        out.push('0');
        return;
    }

    let mut digits = [0u8; 20];
    let mut i = 0;
    while n > 0 {
        digits[i] = (n % 10) as u8 + b'0';
        n /= 10;
        i += 1;
    }
    // Reverse
    for j in (0..i).rev() {
        out.push(digits[j] as char);
    }
}

// ---------------------------------------------------------------------------
// Config parsing (manual — no serde)
// ---------------------------------------------------------------------------

fn parse_result_config(json: &str) -> Result<ResultFormatConfig, String> {
    let labels = extract_string_array(json, "labels")
        .ok_or_else(|| "missing or invalid 'labels' array in config".to_string())?;

    if labels.is_empty() {
        return Err("'labels' array must not be empty".to_string());
    }

    let threshold = extract_number(json, "threshold").unwrap_or(0.5);

    Ok(ResultFormatConfig { labels, threshold })
}

fn extract_number(json: &str, key: &str) -> Option<f64> {
    let mut needle = String::with_capacity(key.len() + 2);
    needle.push('"');
    needle.push_str(key);
    needle.push('"');

    let key_pos = json.find(&needle)?;
    let after_key = &json[key_pos + needle.len()..];
    let colon_pos = after_key.find(':')?;
    let after_colon = &after_key[colon_pos + 1..];
    let trimmed = after_colon.trim_start();

    let end = trimmed
        .find(|c: char| c == ',' || c == '}' || c == ']' || c == '\n' || c == '\r')
        .unwrap_or(trimmed.len());
    let num_str = trimmed[..end].trim();
    num_str.parse::<f64>().ok()
}

fn extract_string_array(json: &str, key: &str) -> Option<Vec<String>> {
    let mut needle = String::with_capacity(key.len() + 2);
    needle.push('"');
    needle.push_str(key);
    needle.push('"');

    let key_pos = json.find(&needle)?;
    let after_key = &json[key_pos + needle.len()..];
    let colon_pos = after_key.find(':')?;
    let after_colon = &after_key[colon_pos + 1..];
    let trimmed = after_colon.trim_start();

    if !trimmed.starts_with('[') {
        return None;
    }

    let bracket_end = find_matching_bracket(&trimmed[1..])?;
    let inner = &trimmed[1..bracket_end + 1];

    let mut result = Vec::new();
    let mut remaining = inner;

    loop {
        remaining = remaining.trim_start();
        if remaining.is_empty() {
            break;
        }

        if !remaining.starts_with('"') {
            break;
        }

        let value_start = &remaining[1..];
        let end_quote = find_unescaped_quote(value_start)?;
        result.push(value_start[..end_quote].to_string());
        remaining = &value_start[end_quote + 1..];

        remaining = remaining.trim_start();
        if remaining.starts_with(',') {
            remaining = &remaining[1..];
        }
    }

    Some(result)
}

fn find_matching_bracket(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth: u32 = 0;
    let mut i = 0;
    let mut in_string = false;

    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_string => i += 2,
            b'"' => {
                in_string = !in_string;
                i += 1;
            }
            b'[' if !in_string => {
                depth += 1;
                i += 1;
            }
            b']' if !in_string => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

fn find_unescaped_quote(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
        } else if bytes[i] == b'"' {
            return Some(i);
        } else {
            i += 1;
        }
    }
    None
}

export!(ResultFormat);
