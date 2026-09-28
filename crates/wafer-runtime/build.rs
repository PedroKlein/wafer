//! Compile-time capture of build provenance so runtime-produced
//! `metadata.json` survives cross-compilation and names what actually ran:
//! the wasmtime and ONNX Runtime (`ort-sys`) packages resolved by the
//! workspace lockfile (version and git source), rustc, the Cargo profile,
//! target, rustflags and enabled features, and the git commit the binary was
//! built from. Failing gracefully with `unknown` keeps metadata.json
//! well-formed on hosts without rustc or git on PATH (e.g. the docker
//! cross-compile image).

use std::path::Path;
use std::process::Command;

fn main() -> std::io::Result<()> {
    println!("cargo:rerun-if-changed=../../Cargo.lock");
    println!("cargo:rerun-if-env-changed=ORT_LIB_LOCATION");
    println!("cargo:rerun-if-env-changed=ORT_STRATEGY");

    let lockfile = std::fs::read_to_string("../../Cargo.lock")?;
    for (package, key) in [("wasmtime", "WASMTIME"), ("ort-sys", "ORT_SYS")] {
        let (version, source) = lock_package(&lockfile, package);
        println!(
            "cargo:rustc-env=WAFER_{key}_VERSION={}",
            version.unwrap_or_else(|| "unknown".to_string())
        );
        println!(
            "cargo:rustc-env=WAFER_{key}_SOURCE={}",
            source.unwrap_or_else(|| "unknown".to_string())
        );
    }
    let ort_lib_location = std::env::var("ORT_LIB_LOCATION").unwrap_or_default();
    let ort_link = match (ort_lib_location.is_empty(), std::env::var_os("CARGO_FEATURE_ORT_DOWNLOAD")) {
        (false, _) => format!("ORT_LIB_LOCATION={ort_lib_location}"),
        (true, Some(_)) => "download-binaries".to_string(),
        (true, None) => "system".to_string(),
    };
    println!("cargo:rustc-env=WAFER_ORT_LINK={ort_link}");

    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let rustc_version = command_output(&rustc, &["--version"], None);
    println!("cargo:rustc-env=WAFER_RUSTC_VERSION={rustc_version}");

    for key in ["PROFILE", "OPT_LEVEL", "TARGET"] {
        println!("cargo:rustc-env=WAFER_BUILD_{key}={}", std::env::var(key).unwrap_or_default());
    }
    // Rustflags are joined by 0x1f; spaces keep the value on one env line.
    let rustflags =
        std::env::var("CARGO_ENCODED_RUSTFLAGS").unwrap_or_default().replace('\x1f', " ");
    println!("cargo:rustc-env=WAFER_BUILD_RUSTFLAGS={rustflags}");
    let mut features: Vec<String> = std::env::vars()
        .filter_map(|(key, _)| key.strip_prefix("CARGO_FEATURE_").map(str::to_lowercase))
        .collect();
    features.sort();
    println!("cargo:rustc-env=WAFER_BUILD_FEATURES={}", features.join(","));

    capture_git();
    Ok(())
}

/// Record the commit the binary was built from, so a stale binary cannot be
/// attributed to the harness's run-time `git_sha`. `--no-optional-locks`
/// keeps `git status` from rewriting the index, which would retrigger this
/// script on every build. The dirty flag reflects the tree when this script
/// last ran; the canonical harness separately requires a clean tree.
fn capture_git() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let dir = Path::new(&manifest_dir);
    // Cargo reruns the script on every build when a watched path is missing,
    // so only existing files are watched. Branch refs live in the common dir
    // (shared by worktrees) and move to `packed-refs` after `git gc`.
    let git_dir = command_output("git", &["rev-parse", "--absolute-git-dir"], Some(dir));
    if git_dir != "unknown" {
        let common_dir = command_output(
            "git",
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            Some(dir),
        );
        let common_dir = if common_dir == "unknown" { git_dir.clone() } else { common_dir };
        let head_ref = command_output("git", &["symbolic-ref", "-q", "HEAD"], Some(dir));
        let mut watched = vec![
            format!("{git_dir}/HEAD"),
            format!("{git_dir}/index"),
            format!("{common_dir}/packed-refs"),
        ];
        if head_ref != "unknown" {
            watched.push(format!("{common_dir}/{head_ref}"));
        }
        for path in watched.iter().filter(|path| Path::new(path).exists()) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    let sha = command_output("git", &["rev-parse", "HEAD"], Some(dir));
    println!("cargo:rustc-env=WAFER_BUILD_GIT_SHA={sha}");
    let dirty = if sha == "unknown" {
        "unknown"
    } else {
        match Command::new("git")
            .args(["--no-optional-locks", "status", "--porcelain", "--untracked-files=no"])
            .current_dir(dir)
            .output()
        {
            Ok(out) if out.status.success() && out.stdout.is_empty() => "false",
            Ok(out) if out.status.success() => "true",
            _ => "unknown",
        }
    };
    println!("cargo:rustc-env=WAFER_BUILD_GIT_DIRTY={dirty}");
}

fn command_output(program: &str, args: &[&str], dir: Option<&Path>) -> String {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    command
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Parse `Cargo.lock` for a package's `version` and `source` lines. Uses a
/// hand-written state machine instead of a TOML crate to keep build-time
/// dependencies at zero. The source line carries the git rev for git
/// dependencies, which the version alone does not identify.
fn lock_package(text: &str, name: &str) -> (Option<String>, Option<String>) {
    let name_line = format!("name = \"{name}\"");
    let mut in_pkg = false;
    let mut version = None;
    let mut source = None;
    for line in text.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            if in_pkg {
                break;
            }
            continue;
        }
        if line == name_line {
            in_pkg = true;
            continue;
        }
        if !in_pkg {
            continue;
        }
        if let Some(v) = line.strip_prefix("version = \"").and_then(|r| r.strip_suffix('"')) {
            version = Some(v.to_string());
        } else if let Some(s) = line.strip_prefix("source = \"").and_then(|r| r.strip_suffix('"')) {
            source = Some(s.to_string());
        }
    }
    (version, source)
}
