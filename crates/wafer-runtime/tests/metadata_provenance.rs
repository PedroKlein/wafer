#![cfg(test)]
#![expect(
    clippy::let_underscore_must_use,
    reason = "test: fire-and-forget on task handles during setup"
)]
#![expect(
    clippy::large_futures,
    reason = "test: launch_pipeline future is large due to WASM Store/Component loading"
)]
//! F2 regression: `metadata.json` provenance completeness (RESULT-CONTRACT).
//!
//! Before F2, `metadata.json` produced by `eval/scripts/run-experiment.sh`
//! recorded `git_sha` + `host_tag` + arch + os but missed the fields the
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
use wafer_core::testing::artifact_available;

/// Absolute path to a repo-relative artefact (uses `CARGO_MANIFEST_DIR`
/// so the test works regardless of the cwd cargo picks).
fn repo_path(rel: &str) -> PathBuf {
    let manifest =
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is always set by cargo");
    PathBuf::from(manifest).join("..").join("..").join(rel)
}

const PASS_THROUGH_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm"
);

fn pass_through_plugin_bytes() -> Vec<u8> {
    std::fs::read(PASS_THROUGH_WASM).expect("read pass-through plugin")
}

/// AC1 + AC2 + AC3: fresh launch produces a provenance JSON containing all
/// six new keys, populated non-empty, with `wafer_plugin_hashes` matching
/// the SHA256 of the on-disk .wasm bytes (single-source-of-truth invariant).
#[tokio::test]
async fn metadata_provenance_complete() {
    let config_path = repo_path("eval/configs/pipeline-shakedown.toml");
    if !artifact_available(PASS_THROUGH_WASM) {
        return;
    }
    let plugin_bytes = pass_through_plugin_bytes();
    let expected_plugin_hash = hex::encode(Sha256::digest(&plugin_bytes));

    let config = load_config(&config_path).expect("load pipeline-shakedown.toml");
    validate(&config).expect("shakedown config validates");

    let orchestrator = launch_pipeline(config, Some(&config_path)).await.expect("launch_pipeline");

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
    let _ = output.wait_with_output().expect("wait wafer binary");

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
        "engine_fuel_budgets",
        "epoch_deadline",
        "epoch_tick_ms",
        "effective_metering_mode",
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
    let plugin_map =
        json["wafer_plugin_hashes"].as_object().expect("wafer_plugin_hashes must be an object");
    assert!(!plugin_map.is_empty(), "wafer_plugin_hashes empty on a Wasm pipeline");
    for (node, hash_value) in plugin_map {
        let hash_str = hash_value.as_str().expect("hash must be string");
        assert_eq!(hash_str.len(), 64, "sha256 hex length for node {node}");
        assert_eq!(
            hash_str, expected_plugin_hash,
            "plugin hash for '{node}' must match the on-disk .wasm sha256"
        );
    }
    assert!(json["engine_fuel_budgets"]["transform"].is_null());
    assert!(json["epoch_deadline"].is_null());
    assert_eq!(json["epoch_tick_ms"], 10);
    assert_eq!(json["effective_metering_mode"], "neither");
    assert_eq!(json["config_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(json["wafer_runtime_sha256"].as_str().unwrap().len(), 64);

    assert_shutdown_build_provenance(&json, exe);
}

/// The file the harness merges is the shutdown write, the only
/// one that hashes the binary, and it carries the build determinants.
fn assert_shutdown_build_provenance(json: &serde_json::Value, exe: &str) {
    for key in [
        "ort_sys_source",
        "ort_link",
        "runtime_build",
        "available_parallelism",
        "cpus_allowed_list",
    ] {
        assert!(json.get(key).is_some(), "provenance missing key: {key}");
    }
    assert_eq!(json["provenance_written_at"], "shutdown");
    let exe_hash = hex::encode(Sha256::digest(std::fs::read(exe).expect("read runtime binary")));
    assert_eq!(json["wafer_runtime_sha256"], exe_hash.as_str());
    assert!(json["wasmtime_source"].as_str().unwrap().contains("rev="));
    assert!(json["ort_sys_version"].as_str().is_some_and(|v| v != "unknown"));
    for key in ["git_sha", "git_dirty", "profile", "opt_level", "target", "rustflags", "features"] {
        assert!(json["runtime_build"][key].is_string(), "runtime_build.{key} must be a string");
    }
    assert!(json["tokio_worker_threads"].as_u64().is_some_and(|n| n > 0));
}

/// A timed hot-swap run's final provenance names the replacement
/// binary, not the one loaded at launch.
#[test]
fn swap_run_provenance_records_replacement_hash() {
    let config_path = repo_path("eval/configs/pipeline-shakedown.toml");
    let plugin_path = PathBuf::from(PASS_THROUGH_WASM);
    if !artifact_available(&plugin_path) {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (v1_hash, v2_hash, v2_path) = write_replacement_plugin(&plugin_path, tmp.path());
    let provenance_path = tmp.path().join("provenance.json");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_wafer"))
        .arg("--config")
        .arg(&config_path)
        .arg("--no-api")
        .args(["--swap-after-secs", "1", "--swap-node", "transform", "--swap-plugin"])
        .arg(&v2_path)
        .env("WAFER_METADATA_OUTPUT", &provenance_path)
        .env("WAFER_BENCH_OUTPUT_DIR", tmp.path())
        .output()
        .expect("run wafer binary");
    assert!(output.status.success(), "runtime failed: {}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Hot-swap dispatched"), "swap never dispatched:\n{stdout}");

    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&provenance_path).expect("read provenance"))
            .expect("parse provenance JSON");
    assert_eq!(json["provenance_written_at"], "shutdown");
    assert_ne!(v1_hash, v2_hash);
    assert_eq!(json["wafer_plugin_hashes"]["transform"], v2_hash.as_str());
}

/// A swap adopted by an idle node is the loaded plugin even though no
/// message ever runs on it, so the provenance written at shutdown names the
/// replacement. The stdin source stays open until the swap has been
/// dispatched, then closes without sending anything more.
#[test]
fn swap_adopted_on_idle_node_records_replacement_hash() {
    use std::io::{BufRead, Write};

    let plugin_path = PathBuf::from(PASS_THROUGH_WASM);
    if !artifact_available(&plugin_path) {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (v1_hash, v2_hash, v2_path) = write_replacement_plugin(&plugin_path, tmp.path());
    let config_path = tmp.path().join("idle.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"[pipeline]
name = "idle-swap"

[nodes.source]
type = "source"
kind = "stdin"

[nodes.transform]
type = "transform"
plugin = "{}"

[nodes.sink]
type = "sink"
kind = "file"
path = "{}"

[[edges]]
from = "source"
to = "transform"

[[edges]]
from = "transform"
to = "sink"

[dead_letter]
kind = "file"
path = "{}"
"#,
            plugin_path.display(),
            tmp.path().join("out.jsonl").display(),
            tmp.path().join("dlq.jsonl").display()
        ),
    )
    .unwrap();
    let provenance_path = tmp.path().join("provenance.json");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_wafer"))
        .arg("--config")
        .arg(&config_path)
        .arg("--no-api")
        .args(["--swap-after-secs", "1", "--swap-node", "transform", "--swap-plugin"])
        .arg(&v2_path)
        .env("WAFER_METADATA_OUTPUT", &provenance_path)
        .env_remove("WAFER_BENCH_OUTPUT_DIR")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("run wafer binary");
    let mut stdin = child.stdin.take().expect("piped stdin");
    let mut stdout = std::io::BufReader::new(child.stdout.take().expect("piped stdout"));
    stdin.write_all(b"before the swap\n").unwrap();
    stdin.flush().unwrap();

    let mut log = String::new();
    loop {
        let mut line = String::new();
        assert_ne!(stdout.read_line(&mut line).unwrap(), 0, "runtime exited early:\n{log}");
        log.push_str(&line);
        if line.contains("Hot-swap dispatched") {
            break;
        }
    }
    drop(stdin);

    let output = child.wait_with_output().expect("wait for wafer");
    assert!(output.status.success(), "runtime failed: {}", String::from_utf8_lossy(&output.stderr));
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&provenance_path).expect("read provenance"))
            .expect("parse provenance JSON");
    assert_eq!(json["provenance_written_at"], "shutdown");
    assert_ne!(v1_hash, v2_hash);
    assert_eq!(json["wafer_plugin_hashes"]["transform"], v2_hash.as_str());
}

/// Write a copy of the plugin with a trailing custom section: identical
/// behaviour, different bytes, so the recorded hash tells v1 and v2 apart.
/// Returns the v1 hash, the v2 hash and the v2 path.
fn write_replacement_plugin(
    plugin_path: &std::path::Path,
    dir: &std::path::Path,
) -> (String, String, PathBuf) {
    let v1 = std::fs::read(plugin_path).expect("read pass-through plugin");
    let mut v2 = v1.clone();
    let name = b"wafer-provenance-test";
    let payload = b"v2";
    v2.push(0);
    v2.push(u8::try_from([1, name.len(), payload.len()].iter().sum::<usize>()).unwrap());
    v2.push(u8::try_from(name.len()).unwrap());
    v2.extend_from_slice(name);
    v2.extend_from_slice(payload);
    let v2_path = dir.join("pass-through-v2.wasm");
    std::fs::write(&v2_path, &v2).unwrap();
    (hex::encode(Sha256::digest(&v1)), hex::encode(Sha256::digest(&v2)), v2_path)
}

/// A run stopped after the swap is adopted but before the replacement
/// processes a message must still shut down and write provenance. The
/// runner never reports completion in that case, and the swap payload keeps
/// the report channel open, so shutdown must not wait on it; the provenance
/// still names the adopted replacement.
#[cfg(unix)]
#[test]
fn swap_run_shuts_down_when_swap_never_completes() {
    let plugin_path = PathBuf::from(PASS_THROUGH_WASM);
    if !artifact_available(&plugin_path) {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let (v1_hash, v2_hash, v2_path) = write_replacement_plugin(&plugin_path, tmp.path());
    // One message every 4 s: the first goes out at launch, the swap is ready
    // by 1 s and the idle node adopts it right away, and the second message
    // only arrives at 4 s. Interrupting a second after the dispatch lands
    // between adoption and the first message on the replacement.
    let config_path = tmp.path().join("slow.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"[pipeline]
name = "slow-swap"

[nodes.source]
type = "source"
kind = "bench-source"
rate = 0.25
total_messages = 100
warmup_messages = 0
payload_size = 16

[nodes.transform]
type = "transform"
plugin = "{}"

[nodes.sink]
type = "sink"
kind = "bench-sink"
warmup_secs = 0

[[edges]]
from = "source"
to = "transform"

[[edges]]
from = "transform"
to = "sink"

[dead_letter]
kind = "file"
path = "{}"
"#,
            plugin_path.display(),
            tmp.path().join("dlq.jsonl").display()
        ),
    )
    .unwrap();
    let provenance_path = tmp.path().join("provenance.json");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_wafer"))
        .arg("--config")
        .arg(&config_path)
        .arg("--no-api")
        .args(["--swap-after-secs", "1", "--swap-node", "transform", "--swap-plugin"])
        .arg(&v2_path)
        .env("WAFER_METADATA_OUTPUT", &provenance_path)
        .env("WAFER_BENCH_OUTPUT_DIR", tmp.path())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("run wafer binary");
    let stdout = child.stdout.take().expect("piped stdout");
    let (dispatched_tx, dispatched_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::BufRead;
        let mut stdout = std::io::BufReader::new(stdout);
        let mut line = String::new();
        while stdout.read_line(&mut line).unwrap() != 0 {
            if line.contains("Hot-swap dispatched") {
                let _ = dispatched_tx.send(());
            }
            line.clear();
        }
    });
    dispatched_rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("swap must be dispatched within 30 s");
    std::thread::sleep(std::time::Duration::from_secs(1));
    let status = std::process::Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("send SIGINT");
    assert!(status.success());

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let exit = loop {
        if let Some(exit) = child.try_wait().expect("poll wafer") {
            break exit;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            panic!("runtime did not shut down within 30 s of SIGINT");
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    assert!(exit.success(), "runtime exited with {exit}");

    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&provenance_path).expect("read provenance"))
            .expect("parse provenance JSON");
    assert_eq!(json["provenance_written_at"], "shutdown");
    assert_ne!(v1_hash, v2_hash);
    assert_eq!(json["wafer_plugin_hashes"]["transform"], v2_hash.as_str());
}
