//! Runtime-side provenance emission for `metadata.json` (F2 / RESULT-CONTRACT §runtime).
//!
//! Writes a JSON snapshot of the fields only the runtime can produce
//! authoritatively: resolved wasmtime version, config sha256, per-plugin
//! sha256 (shared with the P0.12 hot-swap guard), runtime-binary sha256,
//! rustc version, kernel string. The shell harness (`run-experiment.sh`)
//! reads this file and merges the fields into the per-run `metadata.json`.
//!
//! Emission is opt-in via `WAFER_METADATA_OUTPUT` (path to a JSON file) or
//! falls back to `$WAFER_BENCH_OUTPUT_DIR/runtime-provenance.json` when the
//! bench-output env var is set. Absence of both means "no provenance
//! sink" — the runtime stays silent, matching the existing shakedown
//! script path that writes its own metadata.json.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use wafer_core::orchestrator::PipelineOrchestrator;
use wafer_types::config::Config;

/// Filename written under `$WAFER_BENCH_OUTPUT_DIR` when the more specific
/// `WAFER_METADATA_OUTPUT` env var is not set. Consumed by
/// `run-experiment.sh::_write_metadata` on shutdown.
pub const DEFAULT_FILENAME: &str = "runtime-provenance.json";

/// Resolve the provenance output path from env vars. Precedence:
/// 1. `WAFER_METADATA_OUTPUT` (explicit path — file or dir).
/// 2. `WAFER_BENCH_OUTPUT_DIR/runtime-provenance.json` (existing convention).
///
/// Returns `None` when neither is set so callers can no-op instead of
/// littering the working directory with provenance files.
pub fn resolve_output_path() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("WAFER_METADATA_OUTPUT") {
        if !explicit.is_empty() {
            let path = PathBuf::from(explicit);
            return Some(if path.is_dir() { path.join(DEFAULT_FILENAME) } else { path });
        }
    }
    std::env::var("WAFER_BENCH_OUTPUT_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|dir| PathBuf::from(dir).join(DEFAULT_FILENAME))
}

/// Write the runtime-owned provenance JSON to `path`. Errors here abort
/// startup rather than degrade silently because a missing provenance
/// artefact breaks canonical-runs reproducibility (RESULT-CONTRACT).
pub fn write_provenance(
    path: &Path,
    orchestrator: &PipelineOrchestrator,
    config_path: &Path,
    config: &Config,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create provenance parent dir {}", parent.display()))?;
        }
    }
    let payload = provenance_json(orchestrator, config_path, config)?;
    let text = serde_json::to_string_pretty(&payload).context("serialize runtime provenance")?;
    std::fs::write(path, text).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Build the JSON payload. Split from `write_provenance` so the
/// integration test can assert on the object without touching disk.
pub fn provenance_json(
    orchestrator: &PipelineOrchestrator,
    config_path: &Path,
    config: &Config,
) -> Result<Value> {
    let config_sha256 = sha256_of_file(config_path)
        .with_context(|| format!("hash config file {}", config_path.display()))?;
    let wafer_runtime_sha256 = runtime_binary_sha256().unwrap_or_else(|_| "unknown".to_string());
    let plugin_hashes_map: Map<String, Value> = orchestrator
        .handle()
        .plugin_hashes_snapshot()
        .into_iter()
        .map(|(k, v)| (k, Value::String(v)))
        .collect();

    let fuel = &config.engine.fuel;
    let has_fuel = fuel.transform.is_some() || fuel.filter.is_some() || fuel.router.is_some();
    let has_epoch = config.engine.epoch_deadline.is_some();
    let effective_metering_mode = match (has_fuel, has_epoch) {
        (false, false) => "neither",
        (true, false) => "fuel-only",
        (false, true) => "epoch-only",
        (true, true) => "fuel-and-epoch",
    };

    Ok(json!({
        "wasmtime_version": env!("WAFER_WASMTIME_VERSION"),
        "rustc_version": env!("WAFER_RUSTC_VERSION"),
        "wafer_runtime_version": env!("CARGO_PKG_VERSION"),
        "wafer_runtime_sha256": wafer_runtime_sha256,
        "config_path": config_path.display().to_string(),
        "config_sha256": config_sha256,
        "wafer_plugin_hashes": Value::Object(plugin_hashes_map),
        "engine_fuel_budgets": {
            "transform": fuel.transform.map(std::num::NonZeroU64::get),
            "filter": fuel.filter.map(std::num::NonZeroU64::get),
            "router": fuel.router.map(std::num::NonZeroU64::get),
        },
        "epoch_deadline": config.engine.epoch_deadline.map(std::num::NonZeroU64::get),
        "epoch_tick_ms": config.engine.epoch_tick_ms,
        "effective_metering_mode": effective_metering_mode,
        "kernel": kernel_string(),
    }))
}

/// SHA-256 (hex) of a file's bytes. Read fully into memory — configs
/// and runtime binaries are both bounded (< 100 MB in practice).
fn sha256_of_file(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

/// Best-effort self-hash. Returns Err on hosts where `current_exe()` is
/// unreliable (some sandboxed environments); callers substitute "unknown".
fn runtime_binary_sha256() -> std::io::Result<String> {
    let exe = std::env::current_exe()?;
    sha256_of_file(&exe)
}

/// `uname -r`-equivalent via a subprocess so we don't take a new dep just
/// for a single string. Falls back to `unknown` on hosts without `uname`
/// (e.g. Windows CI, which is out of scope but must not panic).
fn kernel_string() -> String {
    std::process::Command::new("uname")
        .arg("-r")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn sha256_of_file_hex_encodes_expected_bytes() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(b"wafer-provenance-fixture").unwrap();
        let hash = sha256_of_file(file.path()).unwrap();
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn resolve_output_path_prefers_explicit_env() {
        // Explicit var wins over bench dir; use unique names so parallel
        // tests do not clobber each other's env.
        let dir = tempfile::tempdir().unwrap();
        let explicit = dir.path().join("explicit.json");
        // SAFETY: single-threaded test wrt these vars; no other thread reads them.
        unsafe { std::env::set_var("WAFER_METADATA_OUTPUT", &explicit) };
        // SAFETY: single-threaded test wrt these vars; no other thread reads them.
        unsafe { std::env::set_var("WAFER_BENCH_OUTPUT_DIR", dir.path()) };
        let resolved = resolve_output_path().unwrap();
        assert_eq!(resolved, explicit);
        // SAFETY: single-threaded test wrt these vars; no other thread reads them.
        unsafe { std::env::remove_var("WAFER_METADATA_OUTPUT") };
        // SAFETY: single-threaded test wrt these vars; no other thread reads them.
        unsafe { std::env::remove_var("WAFER_BENCH_OUTPUT_DIR") };
    }
}
