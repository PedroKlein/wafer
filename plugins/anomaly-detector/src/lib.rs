//! Anomaly detector plugin for WAFER pipeline.
//!
//! Stateful transform using Exponential Weighted Moving Average (EWMA) for
//! per-field anomaly detection with z-score calculation.
//!
//! Config: { "alpha": 0.1, "warmup_samples": 50, "z_threshold": 3.0, "fields": ["temperature", "pressure"] }

wit_bindgen::generate!({
    path: "../../wit",
    world: "transform-node",
    generate_all,
});

use exports::wafer::pipeline::transform::{OutputMessage, ProcessError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use wafer_plugin::{bad_input, define_state, output_with_type, payload_as_str, set_state, with_state};

#[derive(Deserialize)]
struct AnomalyConfig {
    alpha: f64,
    warmup_samples: u64,
    z_threshold: f64,
    fields: Vec<String>,
}

struct AnomalyState {
    config: AnomalyConfig,
    accumulators: HashMap<String, EwmaAccumulator>,
}

#[derive(Default)]
struct EwmaAccumulator {
    mean: f64,
    variance: f64,
    count: u64,
}

impl EwmaAccumulator {
    fn update(&mut self, value: f64, alpha: f64) {
        if !value.is_finite() {
            return;
        }

        if self.count == 0 {
            self.mean = value;
            self.variance = 0.0;
        } else {
            let diff = value - self.mean;
            self.mean += alpha * diff;
            self.variance = (1.0 - alpha) * (self.variance + alpha * diff * diff);
        }
        self.count += 1;
    }

    fn z_score(&self, value: f64, warmup: u64) -> f64 {
        if !value.is_finite() || self.count < warmup {
            return 0.0;
        }

        if self.variance <= 0.0 || !self.variance.is_finite() {
            // Zero variance means all samples were identical.
            // Any deviation from the mean is infinitely anomalous.
            let diff = (value - self.mean).abs();
            if diff < f64::EPSILON {
                return 0.0;
            }
            return f64::MAX;
        }

        (value - self.mean) / self.variance.sqrt()
    }
}

define_state!(AnomalyState);

struct AnomalyDetector;

impl exports::wafer::pipeline::lifecycle::Guest for AnomalyDetector {
    fn validate(config: exports::wafer::pipeline::lifecycle::NodeConfig) -> Option<String> {
        match serde_json::from_str::<AnomalyConfig>(&config.config) {
            Ok(c) => {
                if c.fields.is_empty() {
                    return Some("'fields' must not be empty".to_string());
                }
                if c.alpha <= 0.0 || c.alpha >= 1.0 {
                    return Some("'alpha' must be in (0, 1)".to_string());
                }
                None
            }
            Err(e) => Some(format!("config parse error: {e}")),
        }
    }

    fn init(
        config: exports::wafer::pipeline::lifecycle::NodeConfig,
    ) -> Result<(), exports::wafer::pipeline::lifecycle::ProcessError> {
        let cfg: AnomalyConfig = serde_json::from_str(&config.config)
            .map_err(|e| exports::wafer::pipeline::lifecycle::ProcessError::BadInput(format!("config parse error: {e}")))?;

        let accumulators = cfg.fields.iter().map(|f| (f.clone(), EwmaAccumulator::default())).collect();

        set_state!(AnomalyState {
            config: cfg,
            accumulators,
        });
        Ok(())
    }

    fn close() {}
}

impl exports::wafer::pipeline::transform::Guest for AnomalyDetector {
    fn process(
        input: exports::wafer::pipeline::transform::Message,
    ) -> Result<
        exports::wafer::pipeline::transform::OutputMessage,
        exports::wafer::pipeline::transform::ProcessError,
    > {
        let text = payload_as_str!(&input)?;

        let parsed: serde_json::Value = serde_json::from_str(&text)
            .map_err(|_| bad_input!("payload is not valid JSON"))?;

        with_state!(state => {
            let mut field_scores: Vec<(&str, f64)> = Vec::new();
            let mut max_score: f64 = 0.0;

            for field in &state.config.fields {
                let value = parsed.get(field).and_then(|v| v.as_f64());

                let z = match value {
                    Some(v) => {
                        let acc = state.accumulators.entry(field.clone()).or_default();
                        let z = acc.z_score(v, state.config.warmup_samples);
                        acc.update(v, state.config.alpha);
                        z
                    }
                    // Missing field: skip with z_score = 0
                    None => 0.0,
                };

                let abs_z = z.abs();
                if abs_z > max_score {
                    max_score = abs_z;
                }
                field_scores.push((field.as_str(), z));
            }

            let severity = if max_score < state.config.z_threshold {
                "normal"
            } else if max_score < state.config.z_threshold * 2.0 {
                "warning"
            } else {
                "critical"
            };

            // Build output JSON
            let output = build_output(&parsed, max_score, severity, &field_scores);

            Ok(output_with_type!(
                &input,
                output.into_bytes(),
                "application/json"
            ))
        })
    }
}

#[derive(Serialize)]
struct AnomalyOutput<'a> {
    #[serde(flatten)]
    original: &'a serde_json::Value,
    anomaly_score: f64,
    severity: &'a str,
    field_scores: HashMap<&'a str, f64>,
}

fn build_output(original: &serde_json::Value, anomaly_score: f64, severity: &str, field_scores: &[(&str, f64)]) -> String {
    let scores: HashMap<&str, f64> = field_scores.iter().copied().collect();
    let output = AnomalyOutput {
        original,
        anomaly_score,
        severity,
        field_scores: scores,
    };
    serde_json::to_string(&output).unwrap_or_else(|_| "{}".to_string())
}

export!(AnomalyDetector);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ewma_convergence() {
        let mut acc = EwmaAccumulator::default();
        let alpha = 0.1;
        let target = 42.0;

        for _ in 0..100 {
            acc.update(target, alpha);
        }

        assert!((acc.mean - target).abs() < 0.01, "mean should converge to target: got {}", acc.mean);
    }

    #[test]
    fn test_spike_detection() {
        let mut acc = EwmaAccumulator::default();
        let alpha = 0.1;

        // Steady state at 20.0
        for _ in 0..100 {
            acc.update(20.0, alpha);
        }

        // Spike at 50.0
        let z = acc.z_score(50.0, 50);
        assert!(z.abs() > 3.0, "spike should have z_score > 3.0: got {}", z);
    }

    #[test]
    fn test_warmup_period() {
        let mut acc = EwmaAccumulator::default();
        let alpha = 0.1;

        for i in 0..10 {
            acc.update(i as f64, alpha);
        }

        // During warmup (count < 50), z_score should be 0
        let z = acc.z_score(100.0, 50);
        assert_eq!(z, 0.0, "z_score should be 0 during warmup");
    }

    #[test]
    fn test_nan_safety() {
        let mut acc = EwmaAccumulator::default();
        let alpha = 0.1;

        // Build up some state
        for _ in 0..100 {
            acc.update(10.0, alpha);
        }

        // NaN input should not panic
        acc.update(f64::NAN, alpha);
        let z = acc.z_score(f64::NAN, 50);
        assert_eq!(z, 0.0, "NaN should yield z_score of 0");
    }

    #[test]
    fn test_multiple_fields() {
        let mut acc_a = EwmaAccumulator::default();
        let mut acc_b = EwmaAccumulator::default();
        let alpha = 0.1;

        // Different steady states
        for _ in 0..100 {
            acc_a.update(10.0, alpha);
            acc_b.update(50.0, alpha);
        }

        assert!((acc_a.mean - 10.0).abs() < 0.01);
        assert!((acc_b.mean - 50.0).abs() < 0.01);

        // Spike on A only
        let z_a = acc_a.z_score(30.0, 50);
        let z_b = acc_b.z_score(50.0, 50);
        assert!(z_a.abs() > 1.0, "field A should detect deviation");
        assert!(z_b.abs() < 0.01, "field B should be normal: got {}", z_b);
    }
}
