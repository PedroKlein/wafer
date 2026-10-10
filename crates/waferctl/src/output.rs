//! Output formatting for waferctl.

use tabled::{Table, Tabled};

use crate::client::{HotSwapResult, NodeInfo, PipelineStatus};
use crate::config::CtlConfig;

/// Prints pipeline status in human-readable format.
pub fn print_status(status: &PipelineStatus) {
    println!("Pipeline: {}", status.name);
    println!("State:    {}", status.state);
    println!("Nodes:    {}", status.node_count);
    println!();
    println!("Messages:");
    println!("  Processed: {}", status.messages_processed);
    println!("  Failed:    {}", status.messages_failed);
    if let Some(reason) = &status.ready_reason {
        println!();
        println!("Not ready: {reason}");
    }
}

#[derive(Tabled)]
struct NodeRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "TYPE")]
    node_type: String,
    #[tabled(rename = "STATE")]
    state: String,
    #[tabled(rename = "PROCESSED")]
    processed: u64,
    #[tabled(rename = "AVG_MS")]
    avg_ms: String,
}

#[derive(Tabled)]
struct NodeRowWide {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "TYPE")]
    node_type: String,
    #[tabled(rename = "STATE")]
    state: String,
    #[tabled(rename = "SWAP")]
    swappable: String,
    #[tabled(rename = "PROCESSED")]
    processed: u64,
    #[tabled(rename = "FAILED")]
    failed: u64,
    #[tabled(rename = "AVG_MS")]
    avg_ms: String,
    #[tabled(rename = "QUEUE")]
    queue: String,
}

/// Prints nodes in table format.
pub fn print_nodes(nodes: &[NodeInfo], wide: bool) {
    if nodes.is_empty() {
        println!("No nodes found");
        return;
    }

    if wide {
        let rows: Vec<NodeRowWide> = nodes
            .iter()
            .map(|n| NodeRowWide {
                id: n.id.clone(),
                node_type: if n.replacement_eligible { "wasm" } else { "native" }.to_string(),
                state: n.state.clone(),
                swappable: if n.replacement_eligible { "yes" } else { "no" }.to_string(),
                processed: n.processed,
                failed: n.failed,
                avg_ms: "-".to_string(),
                queue: "-".to_string(),
            })
            .collect();

        let table = Table::new(rows).to_string();
        println!("{table}");
    } else {
        let rows: Vec<NodeRow> = nodes
            .iter()
            .map(|n| NodeRow {
                id: n.id.clone(),
                node_type: if n.replacement_eligible { "wasm" } else { "native" }.to_string(),
                state: n.state.clone(),
                processed: n.processed,
                avg_ms: "-".to_string(),
            })
            .collect();

        let table = Table::new(rows).to_string();
        println!("{table}");
    }
}

/// Prints detailed node information.
pub fn print_node_detail(node: &NodeInfo) {
    println!("Node: {}", node.id);
    println!("Type:       {}", if node.replacement_eligible { "wasm" } else { "native" });
    println!("State:      {}", node.state);
    println!("Swappable:  {}", if node.replacement_eligible { "yes" } else { "no" });
    println!();
    println!("Metrics:");
    println!("  Processed: {}", node.processed);
    println!("  Failed:    {}", node.failed);
}

/// Prints hot-swap result.
pub fn print_hot_swap_result(result: &HotSwapResult) {
    if let Some(status) = &result.status {
        println!("✗ Hot-swap of node '{}': {status}", result.node_id);
        if let Some(reason) = &result.reason {
            println!("Reason: {reason}");
        }
    } else if result.replacement_adopted {
        println!("✓ Node '{}' adopted the replacement", result.node_id);
    } else {
        println!("Node '{}' has not adopted the replacement yet", result.node_id);
    }
    if let Some(outcome) = &result.first_post_replacement_local_outcome {
        println!("First outcome: {}", outcome.disposition);
    }
    if let Some(compile_cache) = &result.compile_cache {
        println!("Compile cache: {compile_cache}");
    }
    println!();
    println!("Timing:");
    let timeline = &result.timeline;
    for (label, value) in [
        ("Compile:     ", timeline.compile_ns),
        ("Instantiate: ", timeline.instantiate_ns),
        ("Signal:      ", timeline.signal_ns),
        ("Adopted:     ", timeline.replacement_adopted_ns),
        ("Rollback:    ", timeline.rollback_ns),
    ] {
        if let Some(ns) = value {
            println!("  {label}{ns} ns");
        }
    }
}

/// Prints configuration.
pub fn print_config(config: &CtlConfig) {
    println!("Endpoints:");
    if config.endpoints.is_empty() {
        println!("  (none configured)");
    } else {
        for (name, endpoint) in &config.endpoints {
            let default_marker =
                if config.default.as_deref() == Some(name) { " (default)" } else { "" };
            println!("  {}: {}{}", name, endpoint.url, default_marker);
        }
    }
}
