//! Content-based router plugin for WAFER pipeline.
//!
//! Implements the `router-node` world: routes messages to output ports based
//! on a configurable JSON field value lookup.
//!
//! Config (JSON in NodeConfig.config):
//!   { "route_field": "type", "routes": {"alert": "alert-port", "telemetry": "log-port"}, "default_port": "log-port" }

wit_bindgen::generate!({
    path: "../../wit/router",
    world: "router-node",
    generate_all,
});

use exports::pipeline::routing::router::ProcessError;
use wafer_plugin::{bad_input, define_state, set_state, with_state};

struct RouterConfig {
    route_field: String,
    routes: Vec<(String, String)>,
    default_port: String,
    all_ports: Vec<String>,
}

define_state!(RouterConfig);

struct ContentRouter;

impl exports::pipeline::node::lifecycle::Guest for ContentRouter {
    fn validate(config: exports::pipeline::node::lifecycle::NodeConfig) -> Option<String> {
        match parse_router_config(&config.config) {
            Ok(_) => None,
            Err(e) => Some(e),
        }
    }

    fn init(
        config: exports::pipeline::node::lifecycle::NodeConfig,
    ) -> Result<(), exports::pipeline::node::lifecycle::ProcessError> {
        let cfg = parse_router_config(&config.config)
            .map_err(exports::pipeline::node::lifecycle::ProcessError::BadInput)?;
        set_state!(cfg);
        Ok(())
    }

    fn close() {}
}

impl exports::pipeline::routing::router::Guest for ContentRouter {
    fn output_ports() -> Vec<String> {
        with_state!(cfg => {
            cfg.all_ports.clone()
        })
    }

    fn route(
        input: exports::pipeline::routing::router::Message,
    ) -> Result<Vec<String>, exports::pipeline::routing::router::ProcessError> {
        // Read payload to extract the route field value
        let bytes = input.payload.read_all();

        let text = String::from_utf8(bytes)
            .map_err(|_| bad_input!("payload is not valid UTF-8"))?;

        with_state!(cfg => {
            let value = extract_string_value(&text, &cfg.route_field);

            let port = match value {
                Some(ref v) => {
                    cfg.routes
                        .iter()
                        .find(|(k, _)| k == v)
                        .map(|(_, p)| p.as_str())
                        .unwrap_or(&cfg.default_port)
                }
                None => &cfg.default_port,
            };

            Ok(vec![port.to_string()])
        })
    }
}

// ---------------------------------------------------------------------------
// Config parsing (manual — no serde)
// ---------------------------------------------------------------------------

fn parse_router_config(json: &str) -> Result<RouterConfig, String> {
    let route_field = extract_string_value(json, "route_field")
        .ok_or_else(|| "missing 'route_field' in config".to_string())?;
    let default_port = extract_string_value(json, "default_port")
        .ok_or_else(|| "missing 'default_port' in config".to_string())?;

    let routes = extract_routes_object(json)
        .ok_or_else(|| "missing or invalid 'routes' in config".to_string())?;

    // Collect all unique port names
    let mut all_ports: Vec<String> = routes.iter().map(|(_, v)| v.clone()).collect();
    if !all_ports.contains(&default_port) {
        all_ports.push(default_port.clone());
    }
    all_ports.sort();
    all_ports.dedup();

    Ok(RouterConfig {
        route_field,
        routes,
        default_port,
        all_ports,
    })
}

// ---------------------------------------------------------------------------
// Manual JSON field extraction
// ---------------------------------------------------------------------------

/// Extract a string value for a given key from a JSON object.
fn extract_string_value(json: &str, key: &str) -> Option<String> {
    let mut needle = String::with_capacity(key.len() + 2);
    needle.push('"');
    needle.push_str(key);
    needle.push('"');

    let key_pos = json.find(&needle)?;
    let after_key = &json[key_pos + needle.len()..];

    // Skip whitespace and colon
    let colon_pos = after_key.find(':')?;
    let after_colon = &after_key[colon_pos + 1..];
    let trimmed = after_colon.trim_start();

    if !trimmed.starts_with('"') {
        return None;
    }

    let value_start = &trimmed[1..];
    let end_quote = find_unescaped_quote(value_start)?;
    Some(value_start[..end_quote].to_string())
}

/// Extract the "routes" object as a Vec of (key, value) string pairs.
/// Expects: "routes": {"key1": "val1", "key2": "val2"}
fn extract_routes_object(json: &str) -> Option<Vec<(String, String)>> {
    let needle = "\"routes\"";
    let key_pos = json.find(needle)?;
    let after_key = &json[key_pos + needle.len()..];

    // Find the colon
    let colon_pos = after_key.find(':')?;
    let after_colon = &after_key[colon_pos + 1..];
    let trimmed = after_colon.trim_start();

    // Must start with {
    if !trimmed.starts_with('{') {
        return None;
    }

    // Find matching closing brace
    let obj_content = &trimmed[1..];
    let close_brace = find_matching_brace(obj_content)?;
    let inner = &obj_content[..close_brace];

    // Parse key-value pairs from the inner content
    let mut routes = Vec::new();
    let mut remaining = inner;

    loop {
        remaining = remaining.trim_start();
        if remaining.is_empty() {
            break;
        }

        // Find key
        if !remaining.starts_with('"') {
            break;
        }
        let key_start = &remaining[1..];
        let key_end = find_unescaped_quote(key_start)?;
        let k = key_start[..key_end].to_string();
        remaining = &key_start[key_end + 1..];

        // Skip colon
        let colon = remaining.find(':')?;
        remaining = &remaining[colon + 1..];
        remaining = remaining.trim_start();

        // Find value
        if !remaining.starts_with('"') {
            return None;
        }
        let val_start = &remaining[1..];
        let val_end = find_unescaped_quote(val_start)?;
        let v = val_start[..val_end].to_string();
        remaining = &val_start[val_end + 1..];

        routes.push((k, v));

        // Skip comma if present
        remaining = remaining.trim_start();
        if remaining.starts_with(',') {
            remaining = &remaining[1..];
        }
    }

    Some(routes)
}

/// Find position of closing brace, accounting for nested braces.
fn find_matching_brace(s: &str) -> Option<usize> {
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
            b'{' if !in_string => {
                depth += 1;
                i += 1;
            }
            b'}' if !in_string => {
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

export!(ContentRouter);
