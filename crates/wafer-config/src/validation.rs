//! Semantic validation for pipeline configs.
//!
//! Validation is intentionally two-phase: serde handles structural checks (wrong
//! types, missing required fields) during deserialization; this module handles
//! semantic checks that require cross-field reasoning.
//!
//! ALL errors are accumulated before returning — the user sees the full picture
//! rather than fixing problems one at a time.

use wafer_types::config::{Config, NodeCategory, NodeDef, OverflowPolicy, SinkDef, SourceDef};

pub const UNSUPPORTED_ALLOW_INFERENCE_MESSAGE: &str =
    "allow_inference=true is supported only for Wasm Transform nodes";

use crate::error::ValidationError;

/// # Errors
///
/// Returns all validation failures found (accumulated, never fail-fast).
pub fn validate(config: &Config) -> Result<(), Vec<ValidationError>> {
    let mut errors: Vec<ValidationError> = Vec::new();

    check_edge_node_references(config, &mut errors);
    check_source_no_inbound(config, &mut errors);
    check_sink_no_outbound(config, &mut errors);
    check_filter_single_inbound(config, &mut errors);
    check_router_single_inbound(config, &mut errors);
    check_router_edges_have_port(config, &mut errors);
    check_port_only_on_router_edges(config, &mut errors);
    check_duplicate_edges(config, &mut errors);
    check_processing_nodes_have_input_and_output(config, &mut errors);
    check_stdin_singleton(config, &mut errors);
    check_stdout_singleton(config, &mut errors);
    check_dead_letter_required_for_overflow(config, &mut errors);
    check_queue_capacities(config, &mut errors);
    check_epoch_tick(config, &mut errors);
    check_outbound_http(config, &mut errors);
    check_unsupported_allow_inference(config, &mut errors);
    check_orphan_nodes(config, &mut errors);
    check_no_cycles(config, &mut errors);

    if errors.is_empty() { Ok(()) } else { Err(errors) }
}

// Individual validation checks

/// Every `from`/`to` in edges must refer to an existing node ID.
fn check_edge_node_references(config: &Config, errors: &mut Vec<ValidationError>) {
    for (i, edge) in config.edges.iter().enumerate() {
        if !config.nodes.contains_key(&edge.from) {
            errors.push(ValidationError::new(format!(
                "edge[{i}]: unknown source node '{}'",
                edge.from
            )));
        }
        if !config.nodes.contains_key(&edge.to) {
            errors.push(ValidationError::new(format!(
                "edge[{i}]: unknown destination node '{}'",
                edge.to
            )));
        }
    }
}

/// Source nodes must have zero inbound edges.
fn check_source_no_inbound(config: &Config, errors: &mut Vec<ValidationError>) {
    let inbound_nodes: std::collections::HashSet<&str> =
        config.edges.iter().map(|e| e.to.as_str()).collect();

    for (id, node) in &config.nodes {
        if node.category() == NodeCategory::Source && inbound_nodes.contains(id.as_str()) {
            errors.push(ValidationError::new(format!(
                "source node '{id}' must not have inbound edges"
            )));
        }
    }
}

/// Sink nodes must have zero outbound edges.
fn check_sink_no_outbound(config: &Config, errors: &mut Vec<ValidationError>) {
    let outbound_nodes: std::collections::HashSet<&str> =
        config.edges.iter().map(|e| e.from.as_str()).collect();

    for (id, node) in &config.nodes {
        if node.category() == NodeCategory::Sink && outbound_nodes.contains(id.as_str()) {
            errors.push(ValidationError::new(format!(
                "sink node '{id}' must not have outbound edges"
            )));
        }
    }
}

/// Filter nodes may only have a single inbound edge (borrow semantics require single-stream input).
fn check_filter_single_inbound(config: &Config, errors: &mut Vec<ValidationError>) {
    let mut inbound_count: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    for edge in &config.edges {
        let count = inbound_count.entry(edge.to.as_str()).or_default();
        *count = count.saturating_add(1);
    }

    for (id, node) in &config.nodes {
        if node.category() == NodeCategory::Filter {
            let count = inbound_count.get(id.as_str()).copied().unwrap_or(0);
            if count > 1 {
                errors.push(ValidationError::new(format!(
                    "filter node '{id}' must have exactly one inbound edge, found {count}"
                )));
            }
        }
    }
}

/// Router nodes may only have a single inbound edge.
fn check_router_single_inbound(config: &Config, errors: &mut Vec<ValidationError>) {
    let mut inbound_count: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    for edge in &config.edges {
        let count = inbound_count.entry(edge.to.as_str()).or_default();
        *count = count.saturating_add(1);
    }

    for (id, node) in &config.nodes {
        if node.category() == NodeCategory::Router {
            let count = inbound_count.get(id.as_str()).copied().unwrap_or(0);
            if count > 1 {
                errors.push(ValidationError::new(format!(
                    "router node '{id}' must have exactly one inbound edge, found {count}"
                )));
            }
        }
    }
}

/// Edges from router nodes must specify a `port` field.
fn check_router_edges_have_port(config: &Config, errors: &mut Vec<ValidationError>) {
    for (i, edge) in config.edges.iter().enumerate() {
        let Some(source_node) = config.nodes.get(&edge.from) else {
            continue; // already reported by check_edge_node_references
        };
        if source_node.category() == NodeCategory::Router && edge.port.is_none() {
            errors.push(ValidationError::new(format!(
                "edge[{i}] from router '{}' must specify a 'port' field",
                edge.from
            )));
        }
    }
}

/// Only router output is addressed by port; any other node sends to every outbound edge.
fn check_port_only_on_router_edges(config: &Config, errors: &mut Vec<ValidationError>) {
    for (i, edge) in config.edges.iter().enumerate() {
        let Some(source_node) = config.nodes.get(&edge.from) else {
            continue;
        };
        if edge.port.is_some() && source_node.category() != NodeCategory::Router {
            errors.push(ValidationError::new(format!(
                "edge[{i}]: 'port' is only allowed on edges from a router, but '{}' is a {}",
                edge.from,
                source_node.category()
            )));
        }
    }
}

/// A repeated edge becomes a second sender on the same queue and delivers every message twice.
fn check_duplicate_edges(config: &Config, errors: &mut Vec<ValidationError>) {
    let mut seen = std::collections::HashSet::new();
    for (i, edge) in config.edges.iter().enumerate() {
        if !seen.insert((edge.from.as_str(), edge.to.as_str(), edge.port.as_deref())) {
            errors.push(ValidationError::new(format!(
                "edge[{i}]: duplicate edge '{}' -> '{}'",
                edge.from, edge.to
            )));
        }
    }
}

/// A connected transform, filter or router needs an inbound edge to receive
/// messages and an outbound edge so its output is not discarded.
fn check_processing_nodes_have_input_and_output(
    config: &Config,
    errors: &mut Vec<ValidationError>,
) {
    let inbound: std::collections::HashSet<&str> =
        config.edges.iter().map(|e| e.to.as_str()).collect();
    let outbound: std::collections::HashSet<&str> =
        config.edges.iter().map(|e| e.from.as_str()).collect();

    for (id, node) in &config.nodes {
        if matches!(node.category(), NodeCategory::Source | NodeCategory::Sink) {
            continue;
        }
        let has_inbound = inbound.contains(id.as_str());
        let has_outbound = outbound.contains(id.as_str());
        if has_inbound && !has_outbound {
            errors.push(ValidationError::new(format!(
                "{} node '{id}' has no outbound edge, so its output would be discarded",
                node.category()
            )));
        }
        if has_outbound && !has_inbound {
            errors.push(ValidationError::new(format!(
                "{} node '{id}' has no inbound edge, so it would never receive a message",
                node.category()
            )));
        }
    }
}

/// At most one stdin source is allowed (process has only one stdin).
fn check_stdin_singleton(config: &Config, errors: &mut Vec<ValidationError>) {
    let stdin_count = config
        .nodes
        .values()
        .filter(|n| matches!(n, wafer_types::config::NodeDef::Source(SourceDef::Stdin(_))))
        .count();

    if stdin_count > 1 {
        errors.push(ValidationError::new(format!(
            "at most one stdin source is allowed, found {stdin_count}"
        )));
    }
}

/// At most one stdout sink is allowed (process has only one stdout).
fn check_stdout_singleton(config: &Config, errors: &mut Vec<ValidationError>) {
    let stdout_count = config
        .nodes
        .values()
        .filter(|n| matches!(n, wafer_types::config::NodeDef::Sink(SinkDef::Stdout(_))))
        .count();

    if stdout_count > 1 {
        errors.push(ValidationError::new(format!(
            "at most one stdout sink is allowed, found {stdout_count}"
        )));
    }
}

/// If any edge uses `overflow = "dead-letter"`, `[dead_letter]` must be configured.
fn check_dead_letter_required_for_overflow(config: &Config, errors: &mut Vec<ValidationError>) {
    let has_dead_letter_edge =
        config.edges.iter().any(|e| e.overflow == Some(OverflowPolicy::DeadLetter));

    if has_dead_letter_edge && config.dead_letter.is_none() {
        errors.push(ValidationError::new(
            "one or more edges use 'overflow = \"dead-letter\"' but no [dead_letter] sink is configured",
        ));
    }
}

fn check_queue_capacities(config: &Config, errors: &mut Vec<ValidationError>) {
    if config.engine.default_queue_capacity == 0 {
        errors
            .push(ValidationError::new("engine.default_queue_capacity must be greater than zero"));
    }
    if config.error_policy.retry_buffer_capacity == 0 {
        errors.push(ValidationError::new(
            "error_policy.retry_buffer_capacity must be greater than zero",
        ));
    }
    for (index, edge) in config.edges.iter().enumerate() {
        if edge.capacity == Some(0) {
            errors.push(ValidationError::new(format!(
                "edges[{index}].capacity must be greater than zero"
            )));
        }
    }
    for (node_id, node) in &config.nodes {
        let policy = match node {
            wafer_types::config::NodeDef::Transform(wasm)
            | wafer_types::config::NodeDef::Filter(wasm)
            | wafer_types::config::NodeDef::Router(wasm) => wasm.error_policy.as_ref(),
            wafer_types::config::NodeDef::Source(_) | wafer_types::config::NodeDef::Sink(_) => None,
        };
        if policy.is_some_and(|policy| policy.retry_buffer_capacity == 0) {
            errors.push(ValidationError::new(format!(
                "nodes.{node_id}.error_policy.retry_buffer_capacity must be greater than zero"
            )));
        }
    }
    let dead_letter_capacity = config.dead_letter.as_ref().map(|dead_letter| match dead_letter {
        wafer_types::config::DeadLetterConfig::Mqtt { queue_capacity, .. }
        | wafer_types::config::DeadLetterConfig::File { queue_capacity, .. } => *queue_capacity,
    });
    if dead_letter_capacity == Some(0) {
        errors.push(ValidationError::new("dead_letter.queue_capacity must be greater than zero"));
    }
}

/// A zero tick makes the epoch thread spin a full core and every epoch deadline expire at once.
fn check_epoch_tick(config: &Config, errors: &mut Vec<ValidationError>) {
    if config.engine.epoch_tick_ms == 0 {
        errors.push(ValidationError::new("engine.epoch_tick_ms must be greater than zero"));
    }
}

fn check_outbound_http(config: &Config, errors: &mut Vec<ValidationError>) {
    for (node_id, node) in &config.nodes {
        let Some(wasm) = (match node {
            NodeDef::Transform(wasm) | NodeDef::Filter(wasm) | NodeDef::Router(wasm) => Some(wasm),
            NodeDef::Source(_) | NodeDef::Sink(_) => None,
        }) else {
            continue;
        };
        if wasm.plugin.is_native() && !wasm.capabilities.outbound_http.is_empty() {
            errors.push(ValidationError::new(format!(
                "nodes.{node_id}.capabilities.outbound_http is supported only for Wasm nodes"
            )));
            continue;
        }
        let mut destinations = std::collections::HashSet::new();
        for (index, destination) in wasm.capabilities.outbound_http.iter().enumerate() {
            match destination.canonicalize() {
                Ok(destination) => {
                    if !destinations.insert(destination) {
                        errors.push(ValidationError::new(format!(
                            "nodes.{node_id}.capabilities.outbound_http[{index}]: duplicate normalized destination"
                        )));
                    }
                }
                Err(error) => errors.push(ValidationError::new(format!(
                    "nodes.{node_id}.capabilities.outbound_http[{index}]: {error}"
                ))),
            }
        }
    }
}

fn check_unsupported_allow_inference(config: &Config, errors: &mut Vec<ValidationError>) {
    if config.nodes.values().any(|node| match node {
        NodeDef::Transform(wasm) => wasm.capabilities.allow_inference && wasm.plugin.is_native(),
        NodeDef::Filter(wasm) | NodeDef::Router(wasm) => wasm.capabilities.allow_inference,
        NodeDef::Source(_) | NodeDef::Sink(_) => false,
    }) {
        errors.push(ValidationError::new(UNSUPPORTED_ALLOW_INFERENCE_MESSAGE));
    }
}

/// Every node must be connected by at least one edge (no isolated nodes in a multi-node graph).
///
/// Single-node pipelines are allowed (test fixtures, benchmarks).
fn check_orphan_nodes(config: &Config, errors: &mut Vec<ValidationError>) {
    if config.nodes.len() <= 1 {
        return;
    }

    let referenced: std::collections::HashSet<&str> =
        config.edges.iter().flat_map(|e| [e.from.as_str(), e.to.as_str()]).collect();

    for id in config.nodes.keys() {
        if !referenced.contains(id.as_str()) {
            errors.push(ValidationError::new(format!(
                "node '{id}' is not connected to any edge (orphan)"
            )));
        }
    }
}

/// The DAG must be acyclic.
fn check_no_cycles(config: &Config, errors: &mut Vec<ValidationError>) {
    // Build a petgraph DiGraph and run toposort to detect cycles.
    use petgraph::algo::toposort;
    use petgraph::graph::DiGraph;
    use std::collections::HashMap;

    let mut graph: DiGraph<(), ()> = DiGraph::new();
    let mut index_map: HashMap<&str, petgraph::graph::NodeIndex> = HashMap::new();

    for id in config.nodes.keys() {
        let idx = graph.add_node(());
        index_map.insert(id.as_str(), idx);
    }

    for edge in &config.edges {
        let Some(&from_idx) = index_map.get(edge.from.as_str()) else { continue };
        let Some(&to_idx) = index_map.get(edge.to.as_str()) else { continue };
        graph.add_edge(from_idx, to_idx, ());
    }

    if toposort(&graph, None).is_err() {
        errors.push(ValidationError::new("pipeline DAG contains a cycle"));
    }
}

// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use wafer_types::config::{
        Capabilities, Config, EdgeDef, HttpScheme, NodeDef, OutboundHttpDestination,
        OverflowPolicy, PluginSpec, PluginSpecStructured, SinkDef, SourceDef, StdinSourceConfig,
        StdoutSinkConfig, WasmNodeDef,
    };

    // -------------------------------------------------------------------------
    // Test helpers
    // -------------------------------------------------------------------------

    fn stdin_source() -> NodeDef {
        NodeDef::Source(SourceDef::Stdin(StdinSourceConfig::default()))
    }

    fn stdout_sink() -> NodeDef {
        NodeDef::Sink(SinkDef::Stdout(StdoutSinkConfig::default()))
    }

    fn transform(plugin: &str) -> NodeDef {
        NodeDef::Transform(WasmNodeDef {
            plugin: PluginSpec::WasmPath(plugin.to_string()),
            ..Default::default()
        })
    }

    fn transform_with_capabilities(plugin: &str, capabilities: Capabilities) -> NodeDef {
        NodeDef::Transform(WasmNodeDef {
            plugin: PluginSpec::WasmPath(plugin.to_string()),
            capabilities,
            ..Default::default()
        })
    }

    fn inference_capabilities() -> Capabilities {
        Capabilities { allow_inference: true, ..Default::default() }
    }

    fn native_transform_with_inference() -> NodeDef {
        NodeDef::Transform(WasmNodeDef {
            plugin: PluginSpec::Structured(PluginSpecStructured::Native {
                function: "passthrough".to_string(),
            }),
            capabilities: inference_capabilities(),
            ..Default::default()
        })
    }

    fn filter(plugin: &str) -> NodeDef {
        NodeDef::Filter(WasmNodeDef {
            plugin: PluginSpec::WasmPath(plugin.to_string()),
            ..Default::default()
        })
    }

    fn router(plugin: &str) -> NodeDef {
        NodeDef::Router(WasmNodeDef {
            plugin: PluginSpec::WasmPath(plugin.to_string()),
            ..Default::default()
        })
    }

    fn edge(from: &str, to: &str) -> EdgeDef {
        EdgeDef {
            from: from.to_string(),
            to: to.to_string(),
            port: None,
            capacity: None,
            overflow: None,
        }
    }

    fn edge_with_port(from: &str, to: &str, port: &str) -> EdgeDef {
        EdgeDef {
            from: from.to_string(),
            to: to.to_string(),
            port: Some(port.to_string()),
            capacity: None,
            overflow: None,
        }
    }

    fn edge_with_overflow(from: &str, to: &str, overflow: OverflowPolicy) -> EdgeDef {
        EdgeDef {
            from: from.to_string(),
            to: to.to_string(),
            port: None,
            capacity: None,
            overflow: Some(overflow),
        }
    }

    fn simple_config(nodes: Vec<(&str, NodeDef)>, edges: Vec<EdgeDef>) -> Config {
        Config {
            nodes: nodes.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
            edges,
            ..Default::default()
        }
    }

    // -------------------------------------------------------------------------
    // Validation rule tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_no_cycles() {
        let config = simple_config(
            vec![
                ("a", transform("a.wasm")),
                ("b", transform("b.wasm")),
                ("c", transform("c.wasm")),
            ],
            vec![edge("a", "b"), edge("b", "c"), edge("c", "a")],
        );
        let result = validate(&config);
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("cycle")));
    }

    #[test]
    fn test_orphan_node() {
        let config = simple_config(
            vec![("in", stdin_source()), ("out", stdout_sink()), ("orphan", transform("o.wasm"))],
            vec![edge("in", "out")],
        );
        let result = validate(&config);
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("orphan")));
    }

    #[test]
    fn test_duplicate_edge_rejected() {
        let config = simple_config(
            vec![("in", stdin_source()), ("t", transform("t.wasm")), ("out", stdout_sink())],
            vec![edge("in", "t"), edge("t", "out"), edge("t", "out")],
        );
        let errors = validate(&config).unwrap_err();
        assert!(
            errors.iter().any(|e| e.message == "edge[2]: duplicate edge 't' -> 'out'"),
            "{errors:?}"
        );
    }

    #[test]
    fn test_router_ports_to_same_node_are_not_duplicates() {
        let config = simple_config(
            vec![("in", stdin_source()), ("r", router("r.wasm")), ("out", stdout_sink())],
            vec![
                edge("in", "r"),
                edge_with_port("r", "out", "high"),
                edge_with_port("r", "out", "low"),
            ],
        );
        validate(&config).expect("distinct router ports may share a destination");
    }

    #[test]
    fn test_port_on_non_router_edge_rejected() {
        let config = simple_config(
            vec![("in", stdin_source()), ("t", transform("t.wasm")), ("out", stdout_sink())],
            vec![edge("in", "t"), edge_with_port("t", "out", "alert")],
        );
        let errors = validate(&config).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("'port' is only allowed on edges from a router")),
            "{errors:?}"
        );
    }

    #[test]
    fn test_processing_node_without_outbound_edge_rejected() {
        let config = simple_config(
            vec![
                ("in", stdin_source()),
                ("t", transform("t.wasm")),
                ("dead_end", transform("d.wasm")),
                ("out", stdout_sink()),
            ],
            vec![edge("in", "t"), edge("t", "out"), edge("t", "dead_end")],
        );
        let errors = validate(&config).unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains("'dead_end' has no outbound edge")),
            "{errors:?}"
        );
    }

    #[test]
    fn test_processing_node_without_inbound_edge_rejected() {
        let config = simple_config(
            vec![
                ("in", stdin_source()),
                ("t", transform("t.wasm")),
                ("starved", filter("f.wasm")),
                ("out", stdout_sink()),
            ],
            vec![edge("in", "t"), edge("t", "out"), edge("starved", "out")],
        );
        let errors = validate(&config).unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains("'starved' has no inbound edge")),
            "{errors:?}"
        );
    }

    #[test]
    fn test_zero_epoch_tick_rejected() {
        let mut config = simple_config(
            vec![("in", stdin_source()), ("out", stdout_sink())],
            vec![edge("in", "out")],
        );
        config.engine.epoch_tick_ms = 0;
        let errors = validate(&config).unwrap_err();
        assert!(
            errors.iter().any(|e| e.message == "engine.epoch_tick_ms must be greater than zero"),
            "{errors:?}"
        );
    }

    #[test]
    fn test_router_edge_needs_port() {
        let config = simple_config(
            vec![("in", stdin_source()), ("r", router("r.wasm")), ("out", stdout_sink())],
            vec![
                edge("in", "r"),
                edge("r", "out"), // missing port!
            ],
        );
        let result = validate(&config);
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("port")));
    }

    #[test]
    fn test_allow_inference_true_is_valid_for_wasm_transform() {
        let config = simple_config(
            vec![
                ("in", stdin_source()),
                (
                    "transform",
                    transform_with_capabilities(
                        "transform.wasm",
                        Capabilities { allow_inference: true, ..Default::default() },
                    ),
                ),
                ("out", stdout_sink()),
            ],
            vec![edge("in", "transform"), edge("transform", "out")],
        );

        validate(&config).expect("Wasm Transform should accept allow_inference=true");
    }

    #[test]
    fn test_allow_inference_true_is_rejected_for_native_transform() {
        let config = simple_config(
            vec![
                ("in", stdin_source()),
                ("transform", native_transform_with_inference()),
                ("out", stdout_sink()),
            ],
            vec![edge("in", "transform"), edge("transform", "out")],
        );

        let errors = validate(&config).expect_err("native Transform must not obtain inference");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].message, UNSUPPORTED_ALLOW_INFERENCE_MESSAGE);
    }

    #[test]
    fn test_allow_inference_true_is_rejected_for_filter_and_router() {
        let filter_config = simple_config(
            vec![
                ("in", stdin_source()),
                (
                    "filter",
                    NodeDef::Filter(WasmNodeDef {
                        plugin: PluginSpec::WasmPath("filter.wasm".to_string()),
                        capabilities: inference_capabilities(),
                        ..Default::default()
                    }),
                ),
                ("out", stdout_sink()),
            ],
            vec![edge("in", "filter"), edge("filter", "out")],
        );
        let filter_errors = validate(&filter_config).expect_err("Filter must not obtain inference");
        assert_eq!(filter_errors[0].message, UNSUPPORTED_ALLOW_INFERENCE_MESSAGE);

        let router_config = simple_config(
            vec![
                ("in", stdin_source()),
                (
                    "router",
                    NodeDef::Router(WasmNodeDef {
                        plugin: PluginSpec::WasmPath("router.wasm".to_string()),
                        capabilities: inference_capabilities(),
                        ..Default::default()
                    }),
                ),
                ("out", stdout_sink()),
            ],
            vec![edge("in", "router"), edge_with_port("router", "out", "default")],
        );
        let router_errors = validate(&router_config).expect_err("Router must not obtain inference");
        assert_eq!(router_errors[0].message, UNSUPPORTED_ALLOW_INFERENCE_MESSAGE);
    }

    #[test]
    fn test_allow_inference_false_or_omitted_stays_valid() {
        let omitted = simple_config(
            vec![
                ("in", stdin_source()),
                ("transform", transform("transform.wasm")),
                ("out", stdout_sink()),
            ],
            vec![edge("in", "transform"), edge("transform", "out")],
        );
        validate(&omitted).expect("omitted allow_inference should remain valid");

        let explicit_false = simple_config(
            vec![
                ("in", stdin_source()),
                (
                    "transform",
                    transform_with_capabilities(
                        "transform.wasm",
                        Capabilities { inherit_stdio: true, ..Default::default() },
                    ),
                ),
                ("out", stdout_sink()),
            ],
            vec![edge("in", "transform"), edge("transform", "out")],
        );
        validate(&explicit_false).expect("allow_inference=false should remain valid");
    }

    #[test]
    fn outbound_http_wildcard_is_rejected() {
        let config: Config = toml::from_str(
            r#"
[nodes.transform]
type = "transform"
plugin = "transform.wasm"

[nodes.transform.capabilities]
outbound_http = [{ scheme = "https", host = "*.example.com" }]
"#,
        )
        .expect("configuration must deserialize before semantic validation");

        let errors = validate(&config).expect_err("wildcard destination must be rejected");
        assert!(
            errors.iter().any(|error| {
                error.message.contains("nodes.transform.capabilities.outbound_http[0]")
                    && error.message.contains("wildcard")
            }),
            "unexpected validation errors: {errors:?}"
        );
    }

    #[test]
    fn invalid_outbound_http_destinations_report_index_without_echoing_value() {
        for (host, port) in [
            (".example.com", None),
            ("10.0.0.0/8", None),
            ("api.example.com/path", None),
            ("user@example.com", None),
            ("café.example", None),
            ("bad_label.example", None),
            ("api.example.com", Some(0)),
        ] {
            let capabilities = Capabilities {
                outbound_http: vec![OutboundHttpDestination {
                    scheme: HttpScheme::Https,
                    host: host.to_string(),
                    port,
                }],
                ..Default::default()
            };
            let config = simple_config(
                vec![("transform", transform_with_capabilities("transform.wasm", capabilities))],
                vec![],
            );

            let errors = validate(&config).expect_err("invalid destination must be rejected");
            assert!(
                errors.iter().any(|error| {
                    error.message.contains("nodes.transform.capabilities.outbound_http[0]")
                        && !error.message.contains(host)
                }),
                "validation must identify index without echoing {host}: {errors:?}"
            );
        }
    }

    #[test]
    fn omitted_and_empty_outbound_http_are_valid() {
        let omitted = simple_config(vec![("transform", transform("transform.wasm"))], vec![]);
        validate(&omitted).expect("omitted outbound HTTP grant must be valid");

        let empty = simple_config(
            vec![(
                "transform",
                transform_with_capabilities("transform.wasm", Capabilities::default()),
            )],
            vec![],
        );
        validate(&empty).expect("empty outbound HTTP grant must be valid");
    }

    #[test]
    fn duplicate_normalized_outbound_http_destination_is_rejected() {
        let capabilities = Capabilities {
            outbound_http: vec![
                OutboundHttpDestination {
                    scheme: HttpScheme::Https,
                    host: "API.EXAMPLE.COM".to_string(),
                    port: None,
                },
                OutboundHttpDestination {
                    scheme: HttpScheme::Https,
                    host: "api.example.com".to_string(),
                    port: Some(443),
                },
            ],
            ..Default::default()
        };
        let config = simple_config(
            vec![("transform", transform_with_capabilities("transform.wasm", capabilities))],
            vec![],
        );

        let errors = validate(&config).expect_err("duplicate normalized destination must fail");
        assert!(errors.iter().any(|error| error.message.contains("duplicate normalized")));
    }

    #[test]
    fn native_node_cannot_receive_outbound_http() {
        let capabilities = Capabilities {
            outbound_http: vec![OutboundHttpDestination {
                scheme: HttpScheme::Http,
                host: "127.0.0.1".to_string(),
                port: Some(8080),
            }],
            ..Default::default()
        };
        let config = simple_config(
            vec![(
                "transform",
                NodeDef::Transform(WasmNodeDef {
                    plugin: PluginSpec::Structured(PluginSpecStructured::Native {
                        function: "passthrough".to_string(),
                    }),
                    capabilities,
                    ..Default::default()
                }),
            )],
            vec![],
        );

        let errors = validate(&config).expect_err("native node must not obtain outbound HTTP");
        assert!(errors.iter().any(|error| error.message.contains("supported only for Wasm")));
    }

    #[test]
    fn test_transform_allows_multiple_inbound() {
        // Two edges to same transform is OK (implicit merge)
        let config = simple_config(
            vec![
                ("s1", stdin_source()),
                ("s2", NodeDef::Source(SourceDef::Stdin(StdinSourceConfig::default()))),
                ("t", transform("t.wasm")),
                ("out", stdout_sink()),
            ],
            vec![edge("s1", "t"), edge("s2", "t"), edge("t", "out")],
        );
        // Only stdin singleton check should trigger (two stdinss)
        let result = validate(&config);
        // The transform multiple-inbound should NOT produce an error
        let errors = result.unwrap_err();
        assert!(!errors.iter().any(|e| e.message.contains("transform")));
    }

    #[test]
    fn test_filter_rejects_multiple_inbound() {
        let config = simple_config(
            vec![
                ("a", transform("a.wasm")),
                ("b", transform("b.wasm")),
                ("f", filter("f.wasm")),
                ("out", stdout_sink()),
            ],
            vec![edge("a", "f"), edge("b", "f"), edge("f", "out")],
        );
        let result = validate(&config);
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("filter")));
    }

    #[test]
    fn test_router_rejects_multiple_inbound() {
        let config = simple_config(
            vec![
                ("a", transform("a.wasm")),
                ("b", transform("b.wasm")),
                ("r", router("r.wasm")),
                ("out", stdout_sink()),
            ],
            vec![edge("a", "r"), edge("b", "r"), edge_with_port("r", "out", "default")],
        );
        let result = validate(&config);
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("router")));
    }

    #[test]
    fn test_source_zero_inbound() {
        let config = simple_config(
            vec![("in", stdin_source()), ("t", transform("t.wasm"))],
            vec![edge("t", "in"), edge("in", "t")], // edge INTO source
        );
        let result = validate(&config);
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("source")));
    }

    #[test]
    fn test_sink_zero_outbound() {
        let config = simple_config(
            vec![("in", stdin_source()), ("out", stdout_sink()), ("t", transform("t.wasm"))],
            vec![edge("in", "out"), edge("out", "t")], // edge FROM sink
        );
        let result = validate(&config);
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("sink")));
    }

    #[test]
    fn test_dlq_required_for_dead_letter_overflow() {
        let config = simple_config(
            vec![("in", stdin_source()), ("out", stdout_sink())],
            vec![edge_with_overflow("in", "out", OverflowPolicy::DeadLetter)],
        );
        let result = validate(&config);
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("dead-letter") || e.message.contains("dead_letter"))
        );
    }

    #[test]
    fn zero_queue_capacities_fail_validation_before_channel_construction() {
        let mut config = simple_config(
            vec![("in", stdin_source()), ("out", stdout_sink())],
            vec![edge("in", "out")],
        );
        config.engine.default_queue_capacity = 0;
        config.edges[0].capacity = Some(0);
        config.error_policy.retry_buffer_capacity = 0;
        config.dead_letter = Some(wafer_types::config::DeadLetterConfig::File {
            path: "/tmp/wafer-dlq.jsonl".to_string(),
            queue_capacity: 0,
        });

        let errors = validate(&config).expect_err("all zero capacities must be rejected");
        for field in [
            "engine.default_queue_capacity",
            "edges[0].capacity",
            "error_policy.retry_buffer_capacity",
            "dead_letter.queue_capacity",
        ] {
            assert!(
                errors.iter().any(|error| error.message.contains(field)),
                "missing validation error for {field}: {errors:?}"
            );
        }
    }

    #[test]
    fn test_stdin_singleton() {
        let config = simple_config(
            vec![("in1", stdin_source()), ("in2", stdin_source()), ("out", stdout_sink())],
            vec![edge("in1", "out"), edge("in2", "out")],
        );
        let result = validate(&config);
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("stdin")));
    }

    #[test]
    fn test_stdout_singleton() {
        let config = simple_config(
            vec![("in", stdin_source()), ("out1", stdout_sink()), ("out2", stdout_sink())],
            vec![edge("in", "out1"), edge("in", "out2")],
        );
        let result = validate(&config);
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("stdout")));
    }

    #[test]
    fn test_all_edges_reference_existing_nodes() {
        let config = simple_config(vec![("in", stdin_source())], vec![edge("in", "nonexistent")]);
        let result = validate(&config);
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("nonexistent")));
    }

    #[test]
    fn test_accumulated_errors() {
        // Config with multiple problems — should report all of them
        let config = simple_config(
            vec![
                ("in", stdin_source()),
                ("in2", stdin_source()), // duplicate stdin
                ("out", stdout_sink()),
                ("out2", stdout_sink()),         // duplicate stdout
                ("orphan", transform("o.wasm")), // orphan
            ],
            vec![edge("in", "out"), edge("in2", "out2")],
        );
        let result = validate(&config);
        let errors = result.unwrap_err();
        // Expect at least 3 errors: stdin singleton, stdout singleton, orphan
        assert!(errors.len() >= 3, "expected ≥3 errors, got {}", errors.len());
    }

    #[test]
    fn test_valid_complex_pipeline() {
        // Source → Filter → Router → [Transform-A, Transform-B] → Sink
        let config = simple_config(
            vec![
                ("source", stdin_source()),
                ("f", filter("f.wasm")),
                ("r", router("r.wasm")),
                ("ta", transform("ta.wasm")),
                ("tb", transform("tb.wasm")),
                ("sink", stdout_sink()),
            ],
            vec![
                edge("source", "f"),
                edge("f", "r"),
                edge_with_port("r", "ta", "a"),
                edge_with_port("r", "tb", "b"),
                edge("ta", "sink"),
                edge("tb", "sink"),
            ],
        );
        let result = validate(&config);
        assert!(result.is_ok(), "expected valid, got errors: {result:?}");
    }
}
