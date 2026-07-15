//! Metrics snapshot types.

use std::collections::HashMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

/// A snapshot of current metrics from the pipeline.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    /// Counter metrics (monotonically increasing values)
    pub counters: HashMap<String, CounterMetric>,
    /// Gauge metrics (point-in-time values)
    pub gauges: HashMap<String, GaugeMetric>,
}

/// A counter metric with labels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CounterMetric {
    /// Metric description
    pub description: String,
    /// Values by label combination
    pub values: Vec<MetricValue<u64>>,
}

/// A gauge metric with labels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GaugeMetric {
    /// Metric description
    pub description: String,
    /// Values by label combination
    pub values: Vec<MetricValue<f64>>,
}

/// A metric value with its labels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricValue<T> {
    /// Label key-value pairs
    pub labels: HashMap<String, String>,
    /// The metric value
    pub value: T,
}

impl MetricsSnapshot {
    /// Creates an empty metrics snapshot.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a counter metric.
    pub fn add_counter(
        &mut self,
        name: &str,
        description: &str,
        labels: HashMap<String, String>,
        value: u64,
    ) {
        let entry = self.counters.entry(name.to_string()).or_insert_with(|| CounterMetric {
            description: description.to_string(),
            values: Vec::new(),
        });
        entry.values.push(MetricValue { labels, value });
    }

    /// Adds a gauge metric.
    pub fn add_gauge(
        &mut self,
        name: &str,
        description: &str,
        labels: HashMap<String, String>,
        value: f64,
    ) {
        let entry = self.gauges.entry(name.to_string()).or_insert_with(|| GaugeMetric {
            description: description.to_string(),
            values: Vec::new(),
        });
        entry.values.push(MetricValue { labels, value });
    }

    /// Converts the snapshot to Prometheus text format.
    #[must_use]
    #[expect(clippy::let_underscore_must_use, reason = "writeln! to String is infallible")]
    pub fn to_prometheus(&self) -> String {
        let mut output = String::new();

        for (name, metric) in &self.counters {
            let _ = writeln!(output, "# HELP {name} {}", metric.description);
            let _ = writeln!(output, "# TYPE {name} counter");
            for mv in &metric.values {
                let labels = format_labels(&mv.labels);
                let _ = writeln!(output, "{name}{labels} {}", mv.value);
            }
        }

        for (name, metric) in &self.gauges {
            let _ = writeln!(output, "# HELP {name} {}", metric.description);
            let _ = writeln!(output, "# TYPE {name} gauge");
            for mv in &metric.values {
                let labels = format_labels(&mv.labels);
                let _ = writeln!(output, "{name}{labels} {}", mv.value);
            }
        }

        output
    }
}

fn format_labels(labels: &HashMap<String, String>) -> String {
    if labels.is_empty() {
        return String::new();
    }
    let pairs: Vec<String> = labels
        .iter()
        .map(|(k, v)| format!("{k}=\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect();
    format!("{{{}}}", pairs.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_snapshot_serialization() {
        let mut snapshot = MetricsSnapshot::new();

        let mut labels = HashMap::new();
        labels.insert("node".to_string(), "filter".to_string());

        snapshot.add_counter(
            "wafer_messages_total",
            "Total messages processed",
            labels.clone(),
            1000,
        );

        snapshot.add_gauge("wafer_queue_depth", "Current queue depth", labels, 42.0);

        let json = serde_json::to_string(&snapshot).unwrap();
        let parsed: MetricsSnapshot = serde_json::from_str(&json).unwrap();

        assert!(parsed.counters.contains_key("wafer_messages_total"));
        assert!(parsed.gauges.contains_key("wafer_queue_depth"));
    }

    #[test]
    fn test_prometheus_format() {
        let mut snapshot = MetricsSnapshot::new();

        let mut labels = HashMap::new();
        labels.insert("node".to_string(), "filter".to_string());

        snapshot.add_counter("wafer_messages_total", "Total messages processed", labels, 1000);

        let prometheus = snapshot.to_prometheus();
        assert!(prometheus.contains("# HELP wafer_messages_total Total messages processed"));
        assert!(prometheus.contains("# TYPE wafer_messages_total counter"));
        assert!(prometheus.contains("wafer_messages_total{node=\"filter\"} 1000"));
    }
}
