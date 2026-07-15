//! Semantic validation for pipeline configs.
//!
//! Validation is intentionally two-phase: serde handles structural checks (wrong
//! types, missing required fields) during deserialization; this module handles
//! semantic checks that require cross-field reasoning.
//!
//! ALL errors are accumulated before returning — the user sees the full picture
//! rather than fixing problems one at a time.

use wafer_types::config::{Config, NodeCategory, OverflowPolicy, SourceDef, SinkDef};

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
    check_stdin_singleton(config, &mut errors);
    check_stdout_singleton(config, &mut errors);
    check_dead_letter_required_for_overflow(config, &mut errors);
    check_orphan_nodes(config, &mut errors);
    check_no_cycles(config, &mut errors);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
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
        *inbound_count.entry(edge.to.as_str()).or_default() += 1;
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
        *inbound_count.entry(edge.to.as_str()).or_default() += 1;
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

/// Every node must be connected by at least one edge (no isolated nodes in a multi-node graph).
///
/// Single-node pipelines are allowed (test fixtures, benchmarks).
fn check_orphan_nodes(config: &Config, errors: &mut Vec<ValidationError>) {
    if config.nodes.len() <= 1 {
        return;
    }

    let referenced: std::collections::HashSet<&str> = config
        .edges
        .iter()
        .flat_map(|e| [e.from.as_str(), e.to.as_str()])
        .collect();

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
        Config, EdgeDef, NodeDef, OverflowPolicy, SourceDef, SinkDef,
        StdinSourceConfig, StdoutSinkConfig, WasmNodeDef,
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
        NodeDef::Transform(WasmNodeDef { plugin: plugin.to_string(), ..Default::default() })
    }

    fn filter(plugin: &str) -> NodeDef {
        NodeDef::Filter(WasmNodeDef { plugin: plugin.to_string(), ..Default::default() })
    }

    fn router(plugin: &str) -> NodeDef {
        NodeDef::Router(WasmNodeDef { plugin: plugin.to_string(), ..Default::default() })
    }

    fn edge(from: &str, to: &str) -> EdgeDef {
        EdgeDef { from: from.to_string(), to: to.to_string(), port: None, capacity: None, overflow: None }
    }

    fn edge_with_port(from: &str, to: &str, port: &str) -> EdgeDef {
        EdgeDef { from: from.to_string(), to: to.to_string(), port: Some(port.to_string()), capacity: None, overflow: None }
    }

    fn edge_with_overflow(from: &str, to: &str, overflow: OverflowPolicy) -> EdgeDef {
        EdgeDef { from: from.to_string(), to: to.to_string(), port: None, capacity: None, overflow: Some(overflow) }
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
            vec![
                ("in", stdin_source()),
                ("out", stdout_sink()),
                ("orphan", transform("o.wasm")),
            ],
            vec![edge("in", "out")],
        );
        let result = validate(&config);
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("orphan")));
    }

    #[test]
    fn test_router_edge_needs_port() {
        let config = simple_config(
            vec![
                ("in", stdin_source()),
                ("r", router("r.wasm")),
                ("out", stdout_sink()),
            ],
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
            vec![
                edge("a", "r"),
                edge("b", "r"),
                edge_with_port("r", "out", "default"),
            ],
        );
        let result = validate(&config);
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("router")));
    }

    #[test]
    fn test_source_zero_inbound() {
        let config = simple_config(
            vec![
                ("in", stdin_source()),
                ("t", transform("t.wasm")),
            ],
            vec![edge("t", "in"), edge("in", "t")], // edge INTO source
        );
        let result = validate(&config);
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("source")));
    }

    #[test]
    fn test_sink_zero_outbound() {
        let config = simple_config(
            vec![
                ("in", stdin_source()),
                ("out", stdout_sink()),
                ("t", transform("t.wasm")),
            ],
            vec![edge("in", "out"), edge("out", "t")], // edge FROM sink
        );
        let result = validate(&config);
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("sink")));
    }

    #[test]
    fn test_dlq_required_for_dead_letter_overflow() {
        let config = simple_config(
            vec![
                ("in", stdin_source()),
                ("out", stdout_sink()),
            ],
            vec![edge_with_overflow("in", "out", OverflowPolicy::DeadLetter)],
        );
        // dead_letter is None by default
        let result = validate(&config);
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("dead-letter") || e.message.contains("dead_letter")));
    }

    #[test]
    fn test_stdin_singleton() {
        let config = simple_config(
            vec![
                ("in1", stdin_source()),
                ("in2", stdin_source()),
                ("out", stdout_sink()),
            ],
            vec![edge("in1", "out"), edge("in2", "out")],
        );
        let result = validate(&config);
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("stdin")));
    }

    #[test]
    fn test_stdout_singleton() {
        let config = simple_config(
            vec![
                ("in", stdin_source()),
                ("out1", stdout_sink()),
                ("out2", stdout_sink()),
            ],
            vec![edge("in", "out1"), edge("in", "out2")],
        );
        let result = validate(&config);
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("stdout")));
    }

    #[test]
    fn test_all_edges_reference_existing_nodes() {
        let config = simple_config(
            vec![("in", stdin_source())],
            vec![edge("in", "nonexistent")],
        );
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
                ("in2", stdin_source()),       // duplicate stdin
                ("out", stdout_sink()),
                ("out2", stdout_sink()),       // duplicate stdout
                ("orphan", transform("o.wasm")),  // orphan
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
