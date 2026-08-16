//! Quality rules filter plugin for WAFER pipeline.
//!
//! Configurable filter that evaluates numeric fields against a set of rules.
//! Supports range, less_than, greater_than, equals, not_equals operators
//! with ALL or ANY logic combining.
//!
//! Config: { "rules": [{"field":"diameter","op":"range","min":9.9,"max":10.1}], "logic": "all" }

wit_bindgen::generate!({
    path: "../../wit",
    world: "filter-node",
    generate_all,
});

use exports::wafer::pipeline::filter::ProcessError;
use serde::Deserialize;
use wafer_plugin::{bad_input, define_state, payload_as_str, set_state, with_state};

#[derive(Deserialize)]
struct QualityConfigInput {
    rules: Vec<RuleInput>,
    #[serde(default = "default_logic")]
    logic: String,
}

fn default_logic() -> String {
    "all".to_string()
}

#[derive(Deserialize)]
struct RuleInput {
    field: String,
    op: String,
    #[serde(default)]
    min: Option<f64>,
    #[serde(default)]
    max: Option<f64>,
    #[serde(default)]
    value: Option<f64>,
}

#[derive(Clone)]
enum Op {
    Range { min: f64, max: f64 },
    LessThan(f64),
    GreaterThan(f64),
    Equals(f64),
    NotEquals(f64),
}

#[derive(Clone)]
struct Rule {
    field: String,
    op: Op,
}

#[derive(Clone, PartialEq)]
enum Logic {
    All,
    Any,
}

struct QualityConfig {
    rules: Vec<Rule>,
    logic: Logic,
}

define_state!(QualityConfig);

struct QualityRules;

impl exports::wafer::pipeline::lifecycle::Guest for QualityRules {
    fn validate(config: exports::wafer::pipeline::lifecycle::NodeConfig) -> Option<String> {
        match parse_quality_config(&config.config) {
            Ok(_) => None,
            Err(e) => Some(e),
        }
    }

    fn init(
        config: exports::wafer::pipeline::lifecycle::NodeConfig,
    ) -> Result<(), exports::wafer::pipeline::lifecycle::ProcessError> {
        let cfg = parse_quality_config(&config.config)
            .map_err(exports::wafer::pipeline::lifecycle::ProcessError::BadInput)?;
        set_state!(cfg);
        Ok(())
    }

    fn close() {}
}

impl exports::wafer::pipeline::filter::Guest for QualityRules {
    fn evaluate(
        input: exports::wafer::pipeline::filter::Message,
    ) -> Result<bool, exports::wafer::pipeline::filter::ProcessError> {
        let text = payload_as_str!(&input)?;

        let parsed: serde_json::Value = serde_json::from_str(&text)
            .map_err(|_| bad_input!("not valid JSON"))?;

        with_state!(cfg => {
            let results: Vec<bool> = cfg.rules.iter().map(|rule| {
                evaluate_rule(rule, &parsed)
            }).collect();

            let pass = match cfg.logic {
                Logic::All => results.iter().all(|&r| r),
                Logic::Any => results.iter().any(|&r| r),
            };

            Ok(pass)
        })
    }
}

fn evaluate_rule(rule: &Rule, data: &serde_json::Value) -> bool {
    let value = match data.get(&rule.field).and_then(|v| v.as_f64()) {
        Some(v) => v,
        // Missing field or non-numeric: treat as fail
        None => return false,
    };

    // NaN: treat as fail
    if !value.is_finite() {
        return false;
    }

    match &rule.op {
        Op::Range { min, max } => value >= *min && value <= *max,
        Op::LessThan(threshold) => value < *threshold,
        Op::GreaterThan(threshold) => value > *threshold,
        Op::Equals(expected) => (value - expected).abs() < f64::EPSILON,
        Op::NotEquals(expected) => (value - expected).abs() >= f64::EPSILON,
    }
}

fn parse_quality_config(json: &str) -> Result<QualityConfig, String> {
    let input: QualityConfigInput = serde_json::from_str(json)
        .map_err(|e| format!("config parse error: {e}"))?;

    let logic = match input.logic.as_str() {
        "all" => Logic::All,
        "any" => Logic::Any,
        other => return Err(format!("invalid logic: '{other}', must be 'all' or 'any'")),
    };

    let mut rules = Vec::with_capacity(input.rules.len());
    for r in input.rules {
        let op = match r.op.as_str() {
            "range" => {
                let min = r.min.ok_or("range op requires 'min'")?;
                let max = r.max.ok_or("range op requires 'max'")?;
                Op::Range { min, max }
            }
            "less_than" => Op::LessThan(r.value.ok_or("less_than op requires 'value'")?),
            "greater_than" => Op::GreaterThan(r.value.ok_or("greater_than op requires 'value'")?),
            "equals" => Op::Equals(r.value.ok_or("equals op requires 'value'")?),
            "not_equals" => Op::NotEquals(r.value.ok_or("not_equals op requires 'value'")?),
            other => return Err(format!("unknown op: '{other}'")),
        };
        rules.push(Rule { field: r.field, op });
    }

    Ok(QualityConfig { rules, logic })
}

export!(QualityRules);

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rule(field: &str, op: Op) -> Rule {
        Rule { field: field.to_string(), op }
    }

    fn make_json(pairs: &[(&str, f64)]) -> serde_json::Value {
        let map: serde_json::Map<String, serde_json::Value> = pairs.iter()
            .map(|(k, v)| (k.to_string(), serde_json::Value::from(*v)))
            .collect();
        serde_json::Value::Object(map)
    }

    #[test]
    fn test_range_pass() {
        let rule = make_rule("diameter", Op::Range { min: 9.9, max: 10.1 });
        let data = make_json(&[("diameter", 10.0)]);
        assert!(evaluate_rule(&rule, &data));
    }

    #[test]
    fn test_range_fail() {
        let rule = make_rule("diameter", Op::Range { min: 9.9, max: 10.1 });
        let data = make_json(&[("diameter", 11.0)]);
        assert!(!evaluate_rule(&rule, &data));
    }

    #[test]
    fn test_less_than() {
        let rule = make_rule("roughness", Op::LessThan(0.8));
        assert!(evaluate_rule(&rule, &make_json(&[("roughness", 0.5)])));
        assert!(!evaluate_rule(&rule, &make_json(&[("roughness", 0.8)])));
        assert!(!evaluate_rule(&rule, &make_json(&[("roughness", 1.0)])));
    }

    #[test]
    fn test_all_logic() {
        let rules = vec![
            make_rule("diameter", Op::Range { min: 9.9, max: 10.1 }),
            make_rule("roughness", Op::LessThan(0.8)),
        ];
        let cfg = QualityConfig { rules, logic: Logic::All };
        let data = make_json(&[("diameter", 10.0), ("roughness", 0.5)]);

        let results: Vec<bool> = cfg.rules.iter().map(|r| evaluate_rule(r, &data)).collect();
        assert!(results.iter().all(|&r| r));

        // One fails
        let data2 = make_json(&[("diameter", 10.0), ("roughness", 1.0)]);
        let results2: Vec<bool> = cfg.rules.iter().map(|r| evaluate_rule(r, &data2)).collect();
        assert!(!results2.iter().all(|&r| r));
    }

    #[test]
    fn test_any_logic() {
        let rules = vec![
            make_rule("a", Op::GreaterThan(5.0)),
            make_rule("b", Op::GreaterThan(5.0)),
        ];
        let data = make_json(&[("a", 3.0), ("b", 10.0)]);
        let results: Vec<bool> = rules.iter().map(|r| evaluate_rule(r, &data)).collect();
        assert!(results.iter().any(|&r| r));
    }

    #[test]
    fn test_missing_field() {
        let rule = make_rule("missing", Op::LessThan(1.0));
        let data = make_json(&[("other", 5.0)]);
        assert!(!evaluate_rule(&rule, &data));
    }

    #[test]
    fn test_nan_handling() {
        let rule = make_rule("x", Op::LessThan(1.0));
        let data = serde_json::json!({"x": f64::NAN});
        assert!(!evaluate_rule(&rule, &data));
    }
}
