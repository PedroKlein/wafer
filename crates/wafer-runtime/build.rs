//! Compile-time capture of wasmtime + rustc versions so runtime-produced
//! `metadata.json` provenance survives cross-compilation and matches the
//! wasmtime resolved by the workspace lockfile (not the Cargo.toml
//! declaration). Failing gracefully with `unknown` keeps metadata.json
//! well-formed on hosts without rustc on PATH.

use std::process::Command;

fn main() -> std::io::Result<()> {
    println!("cargo:rerun-if-changed=../../Cargo.lock");

    let lockfile = std::fs::read_to_string("../../Cargo.lock")?;
    let wasmtime_version = wasmtime_version_from_lockfile(&lockfile)
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=WAFER_WASMTIME_VERSION={wasmtime_version}");

    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let rustc_version = Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map_or_else(|| "unknown".to_string(), |s| s.trim().to_string());
    println!("cargo:rustc-env=WAFER_RUSTC_VERSION={rustc_version}");

    Ok(())
}

/// Parse `Cargo.lock` for the top-level `wasmtime` package version. Uses a
/// hand-written state machine instead of a TOML crate to keep build-time
/// dependencies at zero.
fn wasmtime_version_from_lockfile(text: &str) -> Option<String> {
    let mut in_wasmtime_pkg = false;
    for line in text.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            in_wasmtime_pkg = false;
            continue;
        }
        if line == "name = \"wasmtime\"" {
            in_wasmtime_pkg = true;
            continue;
        }
        if in_wasmtime_pkg {
            if let Some(rest) = line.strip_prefix("version = \"") {
                if let Some(v) = rest.strip_suffix('"') {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}
