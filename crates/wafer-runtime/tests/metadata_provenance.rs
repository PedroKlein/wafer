#![cfg(test)]
//! F2 regression: `metadata.json` provenance completeness (RESULT-CONTRACT).
//!
//! Before F2, `metadata.json` produced by `eval/scripts/run-experiment.sh`
//! recorded git_sha + host_tag + arch + os but missed the fields the
//! canonical Pi runs need for reproducibility: resolved wasmtime version,
//! config sha256, per-plugin sha256, runtime binary sha256, rustc version,
//! and kernel string. Without them a Pi result cannot be re-produced (the
//! wasmtime commit + rustc + plugin bytes are unrecoverable). This test
//! locks the runtime-produced provenance JSON so drift fails loudly.
//!
//! Plugin-hash source of truth: the same cached hash the P0.12 hot-swap
//! guard uses. We assert this by hot-loading pipeline-shakedown.toml (which
//! uses the pass-through plugin) and comparing the emitted hash to a
//! SHA256 computed over the same .wasm bytes.

use std::path::PathBuf;

use sha2::{Digest, Sha256};

use wafer_config::{load_config, validate};
use wafer_core::orchestrator::launch_pipeline;

/// Absolute path to a repo-relative artefact (uses `CARGO_MANIFEST_DIR`
/// so the test works regardless of the cwd cargo picks).
fn repo_path(rel: &str) -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is always set by cargo");
    PathBuf::from(manifest).join("..").join("..").join(rel)
}

fn pass_through_plugin_bytes() -> Vec<u8> {
    let path = repo_path("plugins/pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm");
    std::fs::read(&path).unwrap_or_else(|e| panic!(
        "pass-through plugin missing at {} — run `just build-plugins` first: {e}",
        path.display()
    ))
}

/// AC1 + AC2 + AC3: fresh launch produces a provenance JSON containing all
/// six new keys, populated non-empty, with `wafer_plugin_hashes` matching
/// the SHA256 of the on-disk .wasm bytes (single-source-of-truth invariant).
#[tokio::test]
async fn metadata_provenance_complete() {
    // Skip when the shakedown fixture hasn't been generated (rare in the
    // main dev loop; matches how bench_pipeline.rs handles fixtures).
    let config_path = repo_path("eval/configs/pipeline-shakedown.toml");
    if !config_path.exists() {
        eprintln!("skipping — {} missing", config_path.display());
        return;
    }
    let plugin_bytes = pass_through_plugin_bytes();
    let expected_plugin_hash = hex::encode(Sha256::digest(&plugin_bytes));

    let config = load_config(&config_path).expect("load pipeline-shakedown.toml");
    validate(&config).expect("shakedown config validates");

    let orchestrator = launch_pipeline(config, Some(&config_path))
        .await
        .expect("launch_pipeline");

    // Access the metadata module by re-declaring it here (integration tests
    // don't share the binary's private modules); the module functions are
    // pub within the binary crate so we import via the compiled binary.
    // Instead, we exercise via PipelineHandle::plugin_hashes_snapshot
    // + the same helpers the binary calls.
    let snapshot = orchestrator.handle().plugin_hashes_snapshot();
    assert!(
        !snapshot.is_empty(),
        "launch_pipeline must record at least one plugin hash for a Wasm pipeline"
    );
    let (node_id, hash) = snapshot.iter().next().unwrap();
    assert_eq!(
        hash, &expected_plugin_hash,
        "plugin hash for node '{node_id}' must match SHA256 of the loaded .wasm bytes"
    );

    // Also assert the runtime binary's provenance module produces the six
    // required keys via a direct file write (matching the wire-up in
    // `crates/wafer-runtime/src/main.rs`). Because the module is private
    // to the binary crate, spawn a helper that shells out to the binary
    // with WAFER_METADATA_OUTPUT set and inspect the file.
    let tmp = tempfile::tempdir().unwrap();
    let provenance_path = tmp.path().join("provenance.json");
    let exe = env!("CARGO_BIN_EXE_wafer");
    let output = std::process::Command::new(exe)
        .arg("--config")
        .arg(&config_path)
        .arg("--no-api")
        .env("WAFER_METADATA_OUTPUT", &provenance_path)
        .env("WAFER_BENCH_OUTPUT_DIR", tmp.path())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn wafer binary");
    // BenchSource emits total_messages then the pipeline drains; the binary
    // exits ~2 s later. Wait with a generous 30 s cap so slow CI hosts pass.
    let start = std::time::Instant::now();
    loop {
        if provenance_path.exists() {
            break;
        }
        if start.elapsed() > std::time::Duration::from_secs(30) {
            let _ = output.id();
            panic!("provenance file did not appear at {}", provenance_path.display());
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    // Drain the child so it exits cleanly.
    let _ = output
        .wait_with_output()
        .expect("wait wafer binary");

    let text = std::fs::read_to_string(&provenance_path).expect("read provenance");
    let json: serde_json::Value = serde_json::from_str(&text).expect("parse provenance JSON");

    for key in [
        "wasmtime_version",
        "rustc_version",
        "wafer_runtime_version",
        "wafer_runtime_sha256",
        "config_path",
        "config_sha256",
        "wafer_plugin_hashes",
        "kernel",
    ] {
        assert!(json.get(key).is_some(), "provenance missing key: {key}");
    }
    for key in [
        "wasmtime_version",
        "rustc_version",
        "wafer_runtime_version",
        "wafer_runtime_sha256",
        "config_sha256",
        "kernel",
    ] {
        let value = json[key].as_str().unwrap_or_else(|| panic!("{key} not a string"));
        assert!(!value.is_empty(), "{key} must be non-empty");
    }
    let plugin_map = json["wafer_plugin_hashes"]
        .as_object()
        .expect("wafer_plugin_hashes must be an object");
    assert!(!plugin_map.is_empty(), "wafer_plugin_hashes empty on a Wasm pipeline");
    for (node, hash_value) in plugin_map {
        let hash_str = hash_value.as_str().expect("hash must be string");
        assert_eq!(hash_str.len(), 64, "sha256 hex length for node {node}");
        assert_eq!(
            hash_str, expected_plugin_hash,
            "plugin hash for '{node}' must match the on-disk .wasm sha256"
        );
    }
    assert_eq!(json["config_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(json["wafer_runtime_sha256"].as_str().unwrap().len(), 64);
}
