//! Output formatting for waferctl.

use tabled::{Table, Tabled};
use wafer_types::MetricsSnapshot;

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
                node_type: if n.swappable { "wasm" } else { "native" }.to_string(),
                state: n.state.clone(),
                swappable: if n.swappable { "yes" } else { "no" }.to_string(),
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
                node_type: if n.swappable { "wasm" } else { "native" }.to_string(),
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
    println!("Type:       {}", if node.swappable { "wasm" } else { "native" });
    println!("State:      {}", node.state);
    println!("Swappable:  {}", if node.swappable { "yes" } else { "no" });
    println!();
    println!("Metrics:");
    println!("  Processed: {}", node.processed);
    println!("  Failed:    {}", node.failed);
}

/// Prints hot-swap result.
pub fn print_hot_swap_result(result: &HotSwapResult) {
    println!("✓ Hot-swap request sent for node '{}'", result.node_id);
    println!("Status: {}", result.status);
    println!();
    println!("Timing:");
    if let Some(compile_ns) = result.timeline.compile_ns {
        println!("  Compile:     {compile_ns} ns");
    }
    if let Some(instantiate_ns) = result.timeline.instantiate_ns {
        println!("  Instantiate: {instantiate_ns} ns");
    }
}

/// Prints metrics in human-readable format.
pub fn print_metrics(metrics: &MetricsSnapshot) {
    if metrics.counters.is_empty() && metrics.gauges.is_empty() {
        println!("No metrics available");
        return;
    }

    if !metrics.counters.is_empty() {
        println!("Counters:");
        for (name, metric) in &metrics.counters {
            println!("  {} - {}", name, metric.description);
            for value in &metric.values {
                let labels: Vec<String> =
                    value.labels.iter().map(|(k, v)| format!("{k}={v}")).collect();
                if labels.is_empty() {
                    println!("    {}", value.value);
                } else {
                    println!("    {{{}}} {}", labels.join(", "), value.value);
                }
            }
        }
    }

    if !metrics.gauges.is_empty() {
        println!();
        println!("Gauges:");
        for (name, metric) in &metrics.gauges {
            println!("  {} - {}", name, metric.description);
            for value in &metric.values {
                let labels: Vec<String> =
                    value.labels.iter().map(|(k, v)| format!("{k}={v}")).collect();
                if labels.is_empty() {
                    println!("    {}", value.value);
                } else {
                    println!("    {{{}}} {}", labels.join(", "), value.value);
                }
            }
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
