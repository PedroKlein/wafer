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
            failures.push(format!(
                "{display}: validate returned {} error(s): {errs:?}",
                errs.len()
            ));
            continue;
        }
        // Topology sanity — the historical bug was silent zero-node parsing.
        let has_source = config
            .nodes
            .values()
            .any(|n| n.category() == NodeCategory::Source);
        let has_sink = config
            .nodes
            .values()
            .any(|n| n.category() == NodeCategory::Sink);
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
fn e_iso_7_uses_independent_source_and_sink_populations() {
    let root = workspace_root();
    let control = load_config(&root.join("eval/configs/e-iso-7/pipeline-control.toml"))
        .expect("load E-Iso-7 control config");
    let attack = load_config(&root.join("eval/configs/e-iso-7/pipeline.toml"))
        .expect("load E-Iso-7 panic config");
    let epoch_attack = load_config(&root.join("eval/configs/e-iso-7/pipeline-epoch-attack.toml"))
        .expect("load E-Iso-7 epoch-loop config");

    for config in [&control, &attack, &epoch_attack] {
        assert_eq!(config.engine.epoch_deadline.map(std::num::NonZeroU64::get), Some(2));
        assert_eq!(config.engine.epoch_tick_ms, 1);
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
        value
    });
    assert!(normalized.windows(2).all(|pair| pair[0] == pair[1]));
}
