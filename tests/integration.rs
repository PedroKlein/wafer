//! Integration tests for WAFER MVP pipeline.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const CONFIG_PATH: &str = "examples/pass-through.toml";

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn wafer_binary() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    path.pop();
    path.push("wafer-poc");
    path
}

#[test]
fn test_end_to_end_passthrough() {
    let mut child = Command::new(wafer_binary())
        .args(["--config", CONFIG_PATH])
        .current_dir(project_root())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn process");

    let stdin = child.stdin.as_mut().expect("Failed to open stdin");
    stdin
        .write_all(b"test\n")
        .expect("Failed to write to stdin");
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("Failed to wait for child");

    assert!(
        output.status.success(),
        "Process failed with stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("test"),
        "Expected output to contain 'test', got: {}",
        stdout
    );
}

#[test]
fn test_invalid_config_path() {
    let output = Command::new(wafer_binary())
        .args(["--config", "nonexistent.toml"])
        .current_dir(project_root())
        .output()
        .expect("Failed to execute");

    assert!(!output.status.success(), "Expected process to fail");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("config") || stderr.contains("error") || stderr.contains("Error"),
        "Expected error message about config, got: {}",
        stderr
    );
}

#[test]
fn test_invalid_component_rejected() {
    let temp_config = r#"
name = "invalid-wasm-test"

[transform]
name = "invalid"
plugin_path = "tests/fixtures/invalid.wasm"
fuel_limit = 1000000
queue_capacity = 1024
"#;

    let config_path = "tests/fixtures/invalid-plugin-config.toml";
    std::fs::write(project_root().join(config_path), temp_config)
        .expect("Failed to write temp config");

    let output = Command::new(wafer_binary())
        .args(["--config", config_path])
        .current_dir(project_root())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("Failed to execute");

    let _ = std::fs::remove_file(project_root().join(config_path));

    assert!(
        !output.status.success(),
        "Expected process to fail with invalid WASM"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("component")
            || stderr.contains("wasm")
            || stderr.contains("invalid")
            || stderr.contains("Error")
            || stderr.contains("error"),
        "Expected error about invalid component, got: {}",
        stderr
    );
}

#[test]
fn test_config_parse_error() {
    let output = Command::new(wafer_binary())
        .args(["--config", "tests/fixtures/bad-config.toml"])
        .current_dir(project_root())
        .output()
        .expect("Failed to execute");

    assert!(!output.status.success(), "Expected process to fail");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("TOML")
            || stderr.contains("parse")
            || stderr.contains("error")
            || stderr.contains("Error"),
        "Expected TOML parse error, got: {}",
        stderr
    );
}

#[test]
fn test_metrics_logged() {
    let mut child = Command::new(wafer_binary())
        .args(["--config", CONFIG_PATH])
        .current_dir(project_root())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn process");

    {
        let stdin = child.stdin.as_mut().expect("Failed to open stdin");
        stdin
            .write_all(b"hello\n")
            .expect("Failed to write to stdin");
    }
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("Failed to wait for child");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        output.status.success(),
        "Process failed with code {:?}, output: {}",
        output.status.code(),
        combined
    );

    assert!(
        combined.contains("Pipeline started"),
        "Expected 'Pipeline started' in logs, got: {}",
        combined
    );

    assert!(
        combined.contains("Pipeline stopped"),
        "Expected 'Pipeline stopped' in logs, got: {}",
        combined
    );
}

#[test]
fn test_missing_config_flag() {
    let output = Command::new(wafer_binary())
        .args([] as [&str; 0])
        .current_dir(project_root())
        .output()
        .expect("Failed to execute");

    assert!(
        !output.status.success(),
        "Expected process to fail without --config"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--config") || stderr.contains("required"),
        "Expected error about missing --config, got: {}",
        stderr
    );
}
