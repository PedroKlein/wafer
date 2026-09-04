//! Assert every `eval/configs/*.toml` loads and validates.
//!
//! Every pipeline TOML shipped in the evaluation infrastructure must be
//! runnable via `wafer-runtime --config ...`. This test walks the
//! directory, loads each config, and asserts:
//! 1. Load succeeds (parse errors surface as concrete failures).
//! 2. `validate()` returns Ok (topology is sensible: at least one source,
//!    at least one sink, no dangling edges).
//! 3. There is at least one source, at least one sink, and one edge.
//!
//! Ownership: this test guards against the historical situation where
//! `[pipeline.source]` etc. were silently ignored and every eval run started
//! with zero nodes (the P0.13 reconciliation).

use std::path::{Path, PathBuf};

use wafer_config::{load_config, validate};
use wafer_types::config::{NodeCategory, NodeDef, SinkDef, SourceDef};

#[expect(
    clippy::expect_used,
    reason = "test-only helper: workspace bans expect() in production code, tests are the exception"
)]
fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<repo>/crates/wafer-config`; the eval dir lives
    // two levels up.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    Path::new(manifest_dir)
        .parent()
        .and_then(Path::parent)
        .expect("CARGO_MANIFEST_DIR should have a grandparent")
        .to_path_buf()
}

fn all_configs() -> Vec<PathBuf> {
    let dir = workspace_root().join("eval/configs");
    #[expect(clippy::panic, reason = "test-only: fail loudly if the eval dir is missing")]
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read eval/configs: {e}"))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "eval/configs/ contains no .toml files");
    paths
}

#[test]
fn every_eval_config_loads_and_validates() {
    let mut failures: Vec<String> = Vec::new();
    for path in all_configs() {
        let display = path.strip_prefix(workspace_root()).unwrap_or(&path).display().to_string();
        let config = match load_config(&path) {
            Ok(c) => c,
            Err(e) => {
                failures.push(format!("{display}: load failed: {e}"));
                continue;
            }
        };
        if let Err(errs) = validate(&config) {
            failures
                .push(format!("{display}: validate returned {} error(s): {errs:?}", errs.len()));
            continue;
        }
        // Topology sanity — the historical bug was silent zero-node parsing.
        let has_source = config.nodes.values().any(|n| n.category() == NodeCategory::Source);
        let has_sink = config.nodes.values().any(|n| n.category() == NodeCategory::Sink);
        let edges = config.edges.len();
        if !has_source || !has_sink || edges == 0 {
            failures.push(format!(
                "{display}: parsed shape is degenerate (has_source={has_source}, has_sink={has_sink}, edges={edges}); the P0.13 schema-reconcile guard failed"
            ));
        }
        // Extra check: NodeDef variants are non-empty (each entry deserialised
        // into a real variant, not silently defaulted).
        for (id, node) in &config.nodes {
            match node {
                NodeDef::Source(_)
                | NodeDef::Sink(_)
                | NodeDef::Transform(_)
                | NodeDef::Filter(_)
                | NodeDef::Router(_) => {}
            }
            // Simply exercise the match arms so a future addition to NodeDef
            // triggers a compile error here — keeping this test enum-complete.
            let _ = id;
        }
    }
    assert!(
        failures.is_empty(),
        "eval/configs/*.toml validation failures:\n{}",
        failures.join("\n")
    );
}

#[test]
fn capacity_scout_wafer_is_explicitly_metered() {
    let config = load_config(&workspace_root().join("eval/configs/capacity-scout-wafer.toml"))
        .expect("load capacity scout config");
    assert_eq!(config.engine.epoch_deadline.map(std::num::NonZeroU64::get), Some(100));
    assert_eq!(config.engine.epoch_tick_ms, 10);
    assert_eq!(config.engine.fuel.filter.map(std::num::NonZeroU64::get), Some(500_000));
}

#[test]
fn e_iso_7_uses_independent_source_and_sink_populations() {
    let root = workspace_root();
    let control = load_config(&root.join("eval/configs/e-iso-7/pipeline-control.toml"))
        .expect("load E-Iso-7 control config");
    let attack = load_config(&root.join("eval/configs/e-iso-7/pipeline.toml"))
        .expect("load E-Iso-7 panic config");
    let epoch_attack = load_config(&root.join("eval/configs/e-iso-7/pipeline-epoch-attack.toml"))
        .expect("load E-Iso-7 epoch-loop config");

    for config in [&control, &attack, &epoch_attack] {
        assert_eq!(config.engine.epoch_deadline.map(std::num::NonZeroU64::get), Some(100));
        assert_eq!(config.engine.epoch_tick_ms, 10);
        for (source_id, branch_id, sink_id, expected_dir) in [
            ("source_a", "branch_a", "branch_a_sink", "branch-a"),
            ("source_b", "branch_b", "branch_b_sink", "branch-b"),
        ] {
            let NodeDef::Source(SourceDef::BenchSource(source)) = &config.nodes[source_id] else {
                panic!("{source_id} must be a bench source");
            };
            assert!((source.rate - 1000.0).abs() < f64::EPSILON);
            assert_eq!(source.total_messages, 90_000);
            assert_eq!(source.warmup_messages, 30_000);
            let NodeDef::Sink(SinkDef::BenchSink(sink)) = &config.nodes[sink_id] else {
                panic!("{sink_id} must be a bench sink");
            };
            assert_eq!(sink.output_dir.as_deref(), Some(expected_dir));
            assert!(config.edges.iter().any(|edge| edge.from == source_id && edge.to == branch_id));
            assert!(config.edges.iter().any(|edge| edge.from == branch_id && edge.to == sink_id));
        }
        assert!(!config.nodes.contains_key("source"));
        assert!(!config.edges.iter().any(|edge| edge.from == "source"));
        assert!(!config.edges.iter().any(|edge| edge.to == "sink"));
    }

    let normalized = [&control, &attack, &epoch_attack].map(|config| {
        let mut value = toml::Value::try_from(config).expect("serialize E-Iso-7 config");
        value["nodes"]["branch_b"]["plugin"] = toml::Value::String(String::new());
        value.as_table_mut().expect("config table").remove("engine");
        value
    });
    assert!(normalized.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn e_swap_4_uses_the_frozen_source_driven_burst() {
    let config =
        load_config(&workspace_root().join("eval/configs/e-swap/pipeline-hotswap-burst.toml"))
            .expect("load E-Swap-4 config");
    let NodeDef::Source(SourceDef::BenchSource(source)) = &config.nodes["source"] else {
        panic!("E-Swap-4 source must be a bench source");
    };
    assert!((source.rate - 1_000.0).abs() < f64::EPSILON);
    assert_eq!(source.warmup_messages, 30_000);
    assert_eq!(source.total_messages, 160_000);
    let burst = source.burst.as_ref().expect("E-Swap-4 burst schedule");
    assert!((burst.rate - 2_000.0).abs() < f64::EPSILON);
    assert_eq!((burst.start_secs, burst.end_secs), (55, 65));
}

#[test]
fn final_wafer_catalog_has_explicit_effective_metering() {
    let root = workspace_root();
    let matrix: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("eval/canonical-matrix.json")).expect("read matrix"),
    )
    .expect("parse matrix");
    let campaign = &matrix["final_campaign"];
    let canonical = &campaign["canonical_metering"];
    let catalog =
        campaign["wafer_config_catalog"].as_array().expect("wafer_config_catalog must be an array");
    assert!(!catalog.is_empty());

    for entry in catalog {
        let experiment = entry["experiment"].as_str().expect("catalog experiment");
        let condition = entry["condition"].as_str().expect("catalog condition");
        let path = entry["config"].as_str().expect("catalog config");
        let config = load_config(&root.join(path)).unwrap_or_else(|error| {
            panic!("{experiment}/{condition} failed to load {path}: {error}")
        });
        let expected = if experiment == "e-perf-7" {
            &matrix["experiments"][experiment]["metering_modes"][condition]
        } else if let Some(exception) = matrix["experiments"][experiment]
            .get("metering_exceptions")
            .and_then(|value| value.get(condition))
        {
            exception
        } else {
            canonical
        };
        let expected_epoch = expected["epoch_deadline"].as_u64();
        assert_eq!(
            config.engine.epoch_deadline.map(std::num::NonZeroU64::get),
            expected_epoch,
            "{experiment}/{condition} epoch deadline differs in {path}"
        );
        assert_eq!(
            config.engine.epoch_tick_ms,
            expected.get("epoch_tick_ms").and_then(serde_json::Value::as_u64).unwrap_or(10),
            "{experiment}/{condition} epoch tick differs in {path}"
        );
        for node in config.nodes.values() {
            let (category, override_fuel) = match node {
                NodeDef::Transform(node) if node.plugin.wasm_path().is_some() => {
                    ("transform", node.fuel)
                }
                NodeDef::Filter(node) if node.plugin.wasm_path().is_some() => ("filter", node.fuel),
                NodeDef::Router(node) if node.plugin.wasm_path().is_some() => ("router", node.fuel),
                _ => continue,
            };
            let expected_fuel = expected.get("fuel").and_then(|fuel| {
                fuel.as_u64().or_else(|| fuel.get(category).and_then(serde_json::Value::as_u64))
            });
            let engine_fuel = match node {
                NodeDef::Transform(_) => config.engine.fuel.transform,
                NodeDef::Filter(_) => config.engine.fuel.filter,
                NodeDef::Router(_) => config.engine.fuel.router,
                _ => None,
            };
            assert_eq!(
                override_fuel.or(engine_fuel).map(std::num::NonZeroU64::get),
                expected_fuel,
                "{experiment}/{condition} {category} fuel differs in {path}"
            );
        }
    }
}

#[test]
fn e_perf_7_uses_true_option_ablation_modes_without_sentinels() {
    let root = workspace_root();
    for (condition, fuel, epoch) in [
        ("neither", None, None),
        ("fuel-only", Some(10_000_000), None),
        ("epoch-only", None, Some(100)),
        ("both", Some(10_000_000), Some(100)),
    ] {
        let file = if condition == "both" {
            "pipeline-c-passthrough.toml".to_owned()
        } else {
            format!("pipeline-c-{condition}.toml")
        };
        let path = root.join("eval/configs").join(file);
        let text = std::fs::read_to_string(&path).expect("read E-Perf-7 config");
        assert!(!text.contains("999999999999") && !text.contains("1000000000"));
        let config = load_config(&path).expect("load E-Perf-7 config");
        assert_eq!(config.engine.fuel.transform.map(std::num::NonZeroU64::get), fuel);
        assert_eq!(config.engine.epoch_deadline.map(std::num::NonZeroU64::get), epoch);
    }
}
