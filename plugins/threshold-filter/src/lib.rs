//! Threshold filter plugin for WAFER pipeline.
//!
//! Implements the `filter-node` world: evaluates whether a numeric field
//! in a JSON payload falls within a configured [min, max] range.
//!
//! Config (JSON in NodeConfig.config):
//!   { "field": "temperature", "min": 0.0, "max": 100.0 }

wit_bindgen::generate!({
    path: "../../wit/node",
    world: "filter-node",
    generate_all,
});

use exports::pipeline::node::filter::ProcessError;
use wafer_plugin::{bad_input, define_state, payload_as_str, set_state, with_state};

struct FilterConfig {
    field: String,
    min: f64,
    max: f64,
}

define_state!(FilterConfig);

struct ThresholdFilter;

impl exports::pipeline::node::lifecycle::Guest for ThresholdFilter {
    fn validate(config: exports::pipeline::node::lifecycle::NodeConfig) -> Option<String> {
        match parse_filter_config(&config.config) {
            Ok(_) => None,
            Err(e) => Some(e),
        }
    }

    fn init(
        config: exports::pipeline::node::lifecycle::NodeConfig,
    ) -> Result<(), exports::pipeline::node::lifecycle::ProcessError> {
        let cfg = parse_filter_config(&config.config)
            .map_err(exports::pipeline::node::lifecycle::ProcessError::BadInput)?;
        set_state!(cfg);
        Ok(())
    }

    fn close() {}
}

impl exports::pipeline::node::filter::Guest for ThresholdFilter {
    fn evaluate(
        input: exports::pipeline::node::filter::Message,
    ) -> Result<bool, exports::pipeline::node::filter::ProcessError> {
        let text = payload_as_str!(&input)?;

        with_state!(cfg => {
            let value = extract_number(&text, &cfg.field)
                .ok_or_else(|| bad_input!("field not found"))?;
            Ok(value >= cfg.min && value <= cfg.max)
        })
    }
}

// ---------------------------------------------------------------------------
// Config parsing (manual — no serde)
// ---------------------------------------------------------------------------

fn parse_filter_config(json: &str) -> Result<FilterConfig, String> {
    let field = extract_string_value(json, "field")
        .ok_or_else(|| "missing or invalid 'field' in config".to_string())?;
    let min = extract_number(json, "min")
        .ok_or_else(|| "missing or invalid 'min' in config".to_string())?;
    let max = extract_number(json, "max")
        .ok_or_else(|| "missing or invalid 'max' in config".to_string())?;

    if min > max {
        return Err("'min' must be <= 'max'".to_string());
    }

    Ok(FilterConfig { field, min, max })
}

// ---------------------------------------------------------------------------
// Manual JSON field extraction
// ---------------------------------------------------------------------------

/// Extract a string value for a given key from a flat JSON object.
/// Looks for `"key": "value"` pattern.
fn extract_string_value(json: &str, key: &str) -> Option<String> {
    // Build the pattern: "key"
    let mut needle = String::with_capacity(key.len() + 2);
    needle.push('"');
    needle.push_str(key);
    needle.push('"');

    let key_pos = json.find(&needle)?;
    let after_key = &json[key_pos + needle.len()..];

    // Skip whitespace and colon
    let after_colon = after_key.strip_prefix(|c: char| c.is_ascii_whitespace() || c == ':')?;
    let trimmed = after_colon.trim_start();

    // Must start with a quote
    if !trimmed.starts_with('"') {
        return None;
    }

    let value_start = &trimmed[1..];
    let end_quote = find_unescaped_quote(value_start)?;
    Some(value_start[..end_quote].to_string())
}

/// Extract a numeric value for a given key from a flat JSON object.
/// Looks for `"key": <number>` pattern.
fn extract_number(json: &str, key: &str) -> Option<f64> {
    let mut needle = String::with_capacity(key.len() + 2);
    needle.push('"');
    needle.push_str(key);
    needle.push('"');

    let key_pos = json.find(&needle)?;
    let after_key = &json[key_pos + needle.len()..];

    // Skip to colon
    let colon_pos = after_key.find(':')?;
    let after_colon = &after_key[colon_pos + 1..];
    let trimmed = after_colon.trim_start();

    // Parse the number (ends at comma, whitespace, or closing brace/bracket)
    let end = trimmed
        .find(|c: char| c == ',' || c == '}' || c == ']' || c == '\n' || c == '\r')
        .unwrap_or(trimmed.len());
    let num_str = trimmed[..end].trim();
    num_str.parse::<f64>().ok()
}

/// Find the position of the first unescaped double quote.
fn find_unescaped_quote(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2; // skip escaped char
        } else if bytes[i] == b'"' {
            return Some(i);
        } else {
            i += 1;
        }
    }
    None
}

export!(ThresholdFilter);
