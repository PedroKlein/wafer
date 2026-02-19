//! Filter transform plugin for WAFER pipeline.
//!
//! Filters messages based on a pattern match with configurable mode:
//! - `mode = "keep"` (default): EMIT messages containing pattern, FILTER non-matching
//! - `mode = "drop"`: FILTER messages containing pattern, EMIT non-matching
//!
//! Config format (TOML):
//! ```toml
//! pattern = "KEEP"
//! mode = "keep"  # optional, defaults to "keep"
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

/// Static storage for the filter mode ("keep" or "drop").
/// - "keep": emit messages containing pattern, filter (drop) non-matching
/// - "drop": filter (drop) messages containing pattern, emit non-matching
static mut MODE: Option<String> = None;

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

        // Validate mode if provided (optional, defaults to "keep")
        if let Some(mode_value) = parsed.get("mode") {
            match mode_value.as_str() {
                Some("keep") | Some("drop") => {}
                Some(_) => return Some("'mode' must be \"keep\" or \"drop\"".to_string()),
                None => return Some("'mode' must be a string value".to_string()),
            }
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

        let mode = parsed
            .get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or("keep")
            .to_string();

        // SAFETY: WASM is single-threaded, no data races possible
        unsafe {
            PATTERN = Some(pattern);
            MODE = Some(mode);
        }

        Ok(())
    }

    /// Graceful shutdown - no resources to clean up.
    fn close() {
        unsafe {
            PATTERN = None;
            MODE = None;
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
        let mode = unsafe { (*std::ptr::addr_of!(MODE)).as_ref() }
            .expect("init() must be called before process()");

        let bytes = match &input.payload {
            pipeline::transform::types::Payload::Raw(b) => b.clone(),
        };

        let text = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => {
                return pipeline::transform::types::ProcessResult::Emit(input);
            }
        };

        let contains_pattern = text.contains(pattern);
        let should_emit = if mode == "keep" {
            contains_pattern
        } else {
            !contains_pattern
        };

        if should_emit {
            pipeline::transform::types::ProcessResult::Emit(input)
        } else {
            pipeline::transform::types::ProcessResult::Filter
        }
    }
}

export!(Filter);
