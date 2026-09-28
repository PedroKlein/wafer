//! Runtime-side provenance emission for `metadata.json` (F2 / RESULT-CONTRACT §runtime).
//!
//! Writes a JSON snapshot of the fields only the runtime can produce
//! authoritatively: resolved wasmtime and ONNX Runtime packages, build
//! profile/flags/features/commit, config sha256, per-plugin sha256 (shared
//! with the P0.12 hot-swap guard), runtime-binary sha256, rustc version,
//! kernel string, and the effective tokio worker count and CPU affinity.
//! The shell harness (`run-experiment.sh`) reads this file after the runtime
//! exits and merges the fields into the per-run `metadata.json`.
//!
//! The file is written up to three times, each overwrite replacing the
//! last atomically: at launch (so a mid-run crash still leaves provenance),
//! after each successful timed hot-swap (so the live plugin hashes are on
//! disk), and at shutdown. Only the shutdown write carries
//! `wafer_runtime_sha256`: hashing the whole binary reads O(100 MB), which
//! must not overlap a measured window (E-Perf-9 `first_process` in
//! particular), so earlier writes record it as `null`. `provenance_written_at` says
//! which write the file holds.
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

use wafer_core::orchestrator::PipelineHandle;
use wafer_types::config::Config;

/// Filename written under `$WAFER_BENCH_OUTPUT_DIR` when the more specific
/// `WAFER_METADATA_OUTPUT` env var is not set. Consumed by
/// `run-experiment.sh::_write_metadata` on shutdown.
pub const DEFAULT_FILENAME: &str = "runtime-provenance.json";

/// Which point of the run a provenance write describes.
#[derive(Debug, Clone, Copy)]
pub enum WrittenAt {
    Launch,
    Swap,
    Shutdown,
}

impl WrittenAt {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Launch => "launch",
            Self::Swap => "swap",
            Self::Shutdown => "shutdown",
        }
    }
}

/// Resolve the provenance output path from env vars. Precedence:
/// 1. `WAFER_METADATA_OUTPUT` (explicit path — file or dir).
/// 2. `WAFER_BENCH_OUTPUT_DIR/runtime-provenance.json` (existing convention).
///
/// Returns `None` when neither is set so callers can no-op instead of
/// littering the working directory with provenance files.
pub fn resolve_output_path() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("WAFER_METADATA_OUTPUT")
        && !explicit.is_empty()
    {
        let path = PathBuf::from(explicit);
        return Some(if path.is_dir() { path.join(DEFAULT_FILENAME) } else { path });
    }
    std::env::var("WAFER_BENCH_OUTPUT_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|dir| PathBuf::from(dir).join(DEFAULT_FILENAME))
}

/// Write the runtime-owned provenance JSON to `path` via a temporary file and
/// rename, so the harness never reads a half-written file. Callers log and
/// continue on error: the harness records a missing sidecar as `null`, and
/// the canonical verifier rejects such a leaf.
pub fn write_provenance(
    path: &Path,
    handle: &PipelineHandle,
    config_path: &Path,
    config: &Config,
    written_at: WrittenAt,
    runtime_sha256: Option<&str>,
) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create provenance parent dir {}", parent.display()))?;
    }
    let payload = provenance_json(handle, config_path, config, written_at, runtime_sha256)?;
    let text = serde_json::to_string_pretty(&payload).context("serialize runtime provenance")?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))?;
    Ok(())
}

/// Build the JSON payload. Split from `write_provenance` so the
/// integration test can assert on the object without touching disk.
pub fn provenance_json(
    handle: &PipelineHandle,
    config_path: &Path,
    config: &Config,
    written_at: WrittenAt,
    runtime_sha256: Option<&str>,
) -> Result<Value> {
    let config_sha256 = sha256_of_file(config_path)
        .with_context(|| format!("hash config file {}", config_path.display()))?;
    let plugin_hashes_map: Map<String, Value> =
        handle.plugin_hashes_snapshot().into_iter().map(|(k, v)| (k, Value::String(v))).collect();

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
        "provenance_written_at": written_at.as_str(),
        "wasmtime_version": env!("WAFER_WASMTIME_VERSION"),
        "wasmtime_source": env!("WAFER_WASMTIME_SOURCE"),
        "ort_sys_version": env!("WAFER_ORT_SYS_VERSION"),
        "ort_sys_source": env!("WAFER_ORT_SYS_SOURCE"),
        "ort_link": env!("WAFER_ORT_LINK"),
        "rustc_version": env!("WAFER_RUSTC_VERSION"),
        "wafer_runtime_version": env!("CARGO_PKG_VERSION"),
        "wafer_runtime_sha256": runtime_sha256,
        "runtime_build": {
            "git_sha": env!("WAFER_BUILD_GIT_SHA"),
            "git_dirty": env!("WAFER_BUILD_GIT_DIRTY"),
            "profile": env!("WAFER_BUILD_PROFILE"),
            "opt_level": env!("WAFER_BUILD_OPT_LEVEL"),
            "target": env!("WAFER_BUILD_TARGET"),
            "rustflags": env!("WAFER_BUILD_RUSTFLAGS"),
            "features": env!("WAFER_BUILD_FEATURES"),
        },
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
        "tokio_worker_threads": tokio::runtime::Handle::try_current()
            .ok()
            .map(|handle| handle.metrics().num_workers()),
        "available_parallelism": std::thread::available_parallelism().ok().map(std::num::NonZeroUsize::get),
        "cpus_allowed_list": cpus_allowed_list(),
    }))
}

/// SHA-256 (hex) of a file's bytes. Read fully into memory — configs
/// and runtime binaries are both bounded (< 1 GB in practice).
fn sha256_of_file(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

/// Best-effort self-hash. Call it only outside measured windows and off the
/// async workers (`spawn_blocking`): it reads the whole binary. Returns Err on
/// hosts where `current_exe()` is unreliable (some sandboxed environments).
pub fn runtime_binary_sha256() -> std::io::Result<String> {
    let exe = std::env::current_exe()?;
    sha256_of_file(&exe)
}

/// Kernel release. On Linux this is one small procfs read; forking `uname`
/// is kept only as the fallback for hosts without procfs (macOS dev runs).
fn kernel_string() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::process::Command::new("uname")
                .arg("-r")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// CPUs this process may run on (`taskset` / cpuset), from
/// `/proc/self/status`. `None` off Linux.
fn cpus_allowed_list() -> Option<String> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("Cpus_allowed_list:"))
        .map(|value| value.trim().to_string())
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

    #[test]
    fn kernel_string_is_non_empty() {
        assert!(!kernel_string().is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cpus_allowed_list_is_read_on_linux() {
        assert!(cpus_allowed_list().is_some_and(|list| !list.is_empty()));
    }
}
