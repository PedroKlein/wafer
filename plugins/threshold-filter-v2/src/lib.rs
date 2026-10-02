//! Threshold filter v2, the replacement plugin of the E-Swap-3 hot-swap arm.
//!
//! Takes the same config as `threshold-filter` and raises the lower bound to
//! at least [`MIN_FLOOR`]: a message passes when
//! `max(min, MIN_FLOOR) <= value <= max`. A hot-swap keeps the node's config,
//! so the new bound is part of the plugin. The eKuiper replacement rule makes
//! the same change in SQL (`temperature >= 60`).
//!
//! Config (JSON in NodeConfig.config):
//!   { "field": "temperature", "min": 0.0, "max": 100.0 }

wit_bindgen::generate!({
    path: "../../wit",
    world: "filter-node",
    generate_all,
});

use exports::wafer::pipeline::filter::ProcessError;
use wafer_plugin::{bad_input, define_state, payload_as_str, set_state, with_state};

struct FilterConfig {
    field: String,
    min: f64,
    max: f64,
}

const MIN_FLOOR: f64 = 60.0;

define_state!(FilterConfig);

struct ThresholdFilter;

impl exports::wafer::pipeline::lifecycle::Guest for ThresholdFilter {
    fn validate(config: exports::wafer::pipeline::lifecycle::NodeConfig) -> Option<String> {
        parse_filter_config(&config.config).err()
    }

    fn init(
        config: exports::wafer::pipeline::lifecycle::NodeConfig,
    ) -> Result<(), exports::wafer::pipeline::lifecycle::ProcessError> {
        let cfg = parse_filter_config(&config.config)
            .map_err(exports::wafer::pipeline::lifecycle::ProcessError::BadInput)?;
        set_state!(cfg);
        Ok(())
    }

    fn close() {}
}

impl exports::wafer::pipeline::filter::Guest for ThresholdFilter {
    fn evaluate(
        input: exports::wafer::pipeline::filter::Message,
    ) -> Result<bool, exports::wafer::pipeline::filter::ProcessError> {
        let text = payload_as_str!(&input)?;

        with_state!(cfg => {
            let value = extract_number(&text, &cfg.field)
                .ok_or_else(|| bad_input!("field not found"))?;
            Ok(value >= cfg.min.max(MIN_FLOOR) && value <= cfg.max)
        })
    }
}

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

/// Extract a string value for a given key from a flat JSON object.
/// Looks for `"key": "value"` pattern.
fn extract_string_value(json: &str, key: &str) -> Option<String> {
    let mut needle = String::with_capacity(key.len() + 2);
    needle.push('"');
    needle.push_str(key);
    needle.push('"');

    let key_pos = json.find(&needle)?;
    let after_key = &json[key_pos + needle.len()..];

    let after_colon = after_key.strip_prefix(|c: char| c.is_ascii_whitespace() || c == ':')?;
    let trimmed = after_colon.trim_start();

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

    let colon_pos = after_key.find(':')?;
    let after_colon = &after_key[colon_pos + 1..];
    let trimmed = after_colon.trim_start();

    let end = trimmed.find([',', '}', ']', '\n', '\r']).unwrap_or(trimmed.len());
    let num_str = trimmed[..end].trim();
    num_str.parse::<f64>().ok()
}

/// Find the position of the first unescaped double quote.
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

export!(ThresholdFilter);
