//! Filter transform plugin for WAFER pipeline.
//!
//! This plugin drops messages that contain a specified pattern (case-sensitive substring match).
//! Messages that do NOT contain the pattern are passed through unchanged.
//!
//! Config format (TOML):
//! ```toml
//! pattern = "debug"
//! ```

use toml::Value;

wit_bindgen::generate!({
    path: "../../wit",
    world: "transform-node",
});

struct Filter;

/// Static storage for the filter pattern.
/// Safe to use static mut in WASM since it's single-threaded.
static mut PATTERN: Option<String> = None;

impl exports::pipeline::transform::lifecycle::Guest for Filter {
    /// Validate configuration - requires non-empty config with `pattern` key.
    fn validate(config: exports::pipeline::transform::lifecycle::NodeConfig) -> Option<String> {
        if config.config_bytes.is_empty() {
            return Some("config_bytes is empty - must contain pattern = \"...\"".to_string());
        }

        let config_str = match String::from_utf8(config.config_bytes) {
            Ok(s) => s,
            Err(_) => return Some("config_bytes is not valid UTF-8".to_string()),
        };

        let parsed: Value = match config_str.parse() {
            Ok(v) => v,
            Err(e) => return Some(format!("Invalid TOML: {}", e)),
        };

        if parsed.get("pattern").is_none() {
            return Some("Missing 'pattern' key in config".to_string());
        }

        // Verify pattern is a string
        if parsed.get("pattern").and_then(|v| v.as_str()).is_none() {
            return Some("'pattern' must be a string value".to_string());
        }

        None // validation passed
    }

    /// Initialize the node - parse config and store pattern.
    fn init(config: exports::pipeline::transform::lifecycle::NodeConfig) -> Result<(), String> {
        let config_str =
            String::from_utf8(config.config_bytes).map_err(|e| format!("UTF-8 error: {}", e))?;

        let parsed: Value = config_str
            .parse()
            .map_err(|e: toml::de::Error| format!("TOML parse error: {}", e))?;

        let pattern = parsed
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or("pattern must be a string")?
            .to_string();

        // SAFETY: WASM is single-threaded, no data races possible
        unsafe {
            PATTERN = Some(pattern);
        }

        Ok(())
    }

    /// Graceful shutdown - no resources to clean up.
    fn close() {
        // Clear the pattern on close
        unsafe {
            PATTERN = None;
        }
    }
}

impl exports::pipeline::transform::transform::Guest for Filter {
    fn process(
        input: pipeline::transform::types::Envelope,
    ) -> pipeline::transform::types::ProcessResult {
        // SAFETY: WASM is single-threaded, init() must be called before process()
        let pattern = unsafe { (*std::ptr::addr_of!(PATTERN)).as_ref() }
            .expect("init() must be called before process()");

        // Extract payload bytes
        let bytes = match &input.payload {
            pipeline::transform::types::Payload::Raw(b) => b.clone(),
        };

        // Try to convert to string for pattern matching
        let text = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => {
                // Non-UTF8 data: pass through (can't match pattern)
                return pipeline::transform::types::ProcessResult::Emit(input);
            }
        };

        // Case-sensitive substring match: if pattern found, drop the message
        if text.contains(pattern) {
            pipeline::transform::types::ProcessResult::Filter
        } else {
            pipeline::transform::types::ProcessResult::Emit(input)
        }
    }
}

export!(Filter);
