//! Tensor prep plugin for WAFER pipeline.
//!
//! Normalizes JSON sensor values to f32 tensor bytes (little-endian).
//! Configurable field extraction and optional normalization (divide by 100.0).
//!
//! Config: { "fields": ["temperature", "humidity", "pressure"], "normalize": true }

wit_bindgen::generate!({
    path: "../../wit/node",
    world: "transform-node",
    generate_all,
});

use exports::pipeline::node::transform::{OutputMessage, ProcessError};
use wafer_plugin::{bad_input, define_state, output_with_type, payload_as_str, set_state, with_state};

struct TensorPrepConfig {
    fields: Vec<String>,
    normalize: bool,
}

define_state!(TensorPrepConfig);

struct TensorPrep;

impl exports::pipeline::node::lifecycle::Guest for TensorPrep {
    fn validate(config: exports::pipeline::node::lifecycle::NodeConfig) -> Option<String> {
        match parse_tensor_config(&config.config) {
            Ok(_) => None,
            Err(e) => Some(e),
        }
    }

    fn init(
        config: exports::pipeline::node::lifecycle::NodeConfig,
    ) -> Result<(), exports::pipeline::node::lifecycle::ProcessError> {
        let cfg = parse_tensor_config(&config.config)
            .map_err(exports::pipeline::node::lifecycle::ProcessError::BadInput)?;
        set_state!(cfg);
        Ok(())
    }

    fn close() {}
}

impl exports::pipeline::node::transform::Guest for TensorPrep {
    fn process(
        input: exports::pipeline::node::transform::Message,
    ) -> Result<
        exports::pipeline::node::transform::OutputMessage,
        exports::pipeline::node::transform::ProcessError,
    > {
        let text = payload_as_str!(&input)?;

        with_state!(cfg => {
            let mut tensor_bytes = Vec::with_capacity(cfg.fields.len() * 4);

            for field in &cfg.fields {
                let value = match extract_number(&text, field) {
                    Some(v) => v,
                    // Missing field: fill with zero for ML padding
                    None => 0.0,
                };

                // Verify the value is numeric (not NaN/Inf from malformed input)
                if !value.is_finite() {
                    return Err(bad_input!("field not numeric"));
                }

                let normalized = if cfg.normalize {
                    (value / 100.0) as f32
                } else {
                    value as f32
                };

                tensor_bytes.extend_from_slice(&normalized.to_le_bytes());
            }

            Ok(output_with_type!(
                &input,
                tensor_bytes,
                "application/octet-stream"
            ))
        })
    }
}

// ---------------------------------------------------------------------------
// Config parsing (manual — no serde)
// ---------------------------------------------------------------------------

fn parse_tensor_config(json: &str) -> Result<TensorPrepConfig, String> {
    let fields = extract_string_array(json, "fields")
        .ok_or_else(|| "missing or invalid 'fields' array in config".to_string())?;

    if fields.is_empty() {
        return Err("'fields' array must not be empty".to_string());
    }

    let normalize = extract_bool(json, "normalize").unwrap_or(false);

    Ok(TensorPrepConfig { fields, normalize })
}

// ---------------------------------------------------------------------------
// Manual JSON extraction helpers
// ---------------------------------------------------------------------------

/// Extract a numeric value for a given key.
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

/// Extract a boolean value for a given key.
fn extract_bool(json: &str, key: &str) -> Option<bool> {
    let mut needle = String::with_capacity(key.len() + 2);
    needle.push('"');
    needle.push_str(key);
    needle.push('"');

    let key_pos = json.find(&needle)?;
    let after_key = &json[key_pos + needle.len()..];
    let colon_pos = after_key.find(':')?;
    let after_colon = &after_key[colon_pos + 1..];
    let trimmed = after_colon.trim_start();

    if trimmed.starts_with("true") {
        Some(true)
    } else if trimmed.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

/// Extract an array of strings: "key": ["a", "b", "c"]
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

export!(TensorPrep);
