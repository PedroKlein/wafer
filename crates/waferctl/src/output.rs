//! Output formatting for waferctl.

use tabled::{Table, Tabled};
use wafer_types::{HotSwapResult, MetricsSnapshot, NodeInfo, PipelineStatus, ReloadResult};

use crate::config::CtlConfig;

/// Prints pipeline status in human-readable format.
pub fn print_status(status: &PipelineStatus) {
    println!("Pipeline: {}", status.name);
    println!("State:    {}", status.state);
    println!("Uptime:   {}s", status.uptime_secs);
    println!("Nodes:    {}", status.node_count);
    println!();
    println!("Messages:");
    println!("  Processed: {}", status.messages_processed);
    println!("  Failed:    {}", status.messages_failed);
    if status.swap_in_progress {
        println!();
        println!("⚠ Hot-swap in progress");
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
                node_type: n.node_type.to_string(),
                state: n.state.to_string(),
                swappable: if n.swappable { "yes" } else { "no" }.to_string(),
                processed: n.messages_processed,
                failed: n.messages_failed,
                avg_ms: format!("{:.2}", n.avg_process_us as f64 / 1000.0),
                queue: n.queue_depth.map(|d| d.to_string()).unwrap_or_else(|| "-".to_string()),
            })
            .collect();

        let table = Table::new(rows).to_string();
        println!("{}", table);
    } else {
        let rows: Vec<NodeRow> = nodes
            .iter()
            .map(|n| NodeRow {
                id: n.id.clone(),
                node_type: n.node_type.to_string(),
                state: n.state.to_string(),
                processed: n.messages_processed,
                avg_ms: format!("{:.2}", n.avg_process_us as f64 / 1000.0),
            })
            .collect();

        let table = Table::new(rows).to_string();
        println!("{}", table);
    }
}

/// Prints detailed node information.
pub fn print_node_detail(node: &NodeInfo) {
    println!("Node: {}", node.id);
    println!("Type:       {}", node.node_type);
    println!("State:      {}", node.state);
    println!("Swappable:  {}", if node.swappable { "yes" } else { "no" });
    println!();
    println!("Metrics:");
    println!("  Processed: {}", node.messages_processed);
    println!("  Failed:    {}", node.messages_failed);
    println!("  Avg Time:  {:.2}ms", node.avg_process_us as f64 / 1000.0);
    if let Some(depth) = node.queue_depth {
        println!("  Queue:     {}", depth);
    }
}

/// Prints hot-swap result.
pub fn print_hot_swap_result(result: &HotSwapResult) {
    println!("✓ Hot-swap completed for node '{}'", result.node_id);
    println!();
    println!("Timing:");
    println!("  Drain:  {:?}", result.drain_duration);
    println!("  Load:   {:?}", result.load_duration);
    println!("  Total:  {:?}", result.total_duration);
    println!();
    println!("Messages drained: {}", result.messages_drained);
}

/// Prints reload result.
pub fn print_reload_result(result: &ReloadResult) {
    if result.swapped_nodes.is_empty() {
        println!("No changes detected");
    } else {
        println!("✓ Configuration reloaded");
        println!();
        println!("Nodes hot-swapped:");
        for node in &result.swapped_nodes {
            println!("  - {}", node);
        }
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
                    value.labels.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
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
                    value.labels.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
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
