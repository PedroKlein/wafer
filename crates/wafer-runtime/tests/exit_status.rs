//! Exit-status contract of the `wafer` binary: a failed run must never exit 0,
//! because the evaluation harness uses the exit code to accept or reject a run.

#![cfg(test)]

use std::path::Path;
use std::process::{Command, Output};

fn run_wafer(config: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wafer"))
        .arg("--config")
        .arg(config)
        .arg("--no-api")
        .env_remove("WAFER_BENCH_OUTPUT_DIR")
        .output()
        .expect("run wafer")
}

fn write_config(dir: &Path, source: &str, sink: &str) -> std::path::PathBuf {
    let path = dir.join("pipeline.toml");
    let config = format!(
        r#"
[pipeline]
name = "exit-status"

[nodes.source]
type = "source"
{source}

[nodes.t1]
type = "transform"
plugin = {{ kind = "native", function = "passthrough" }}

[nodes.sink]
type = "sink"
{sink}

[[edges]]
from = "source"
to = "t1"

[[edges]]
from = "t1"
to = "sink"
"#
    );
    std::fs::write(&path, config).expect("write config");
    path
}

const BENCH_SOURCE: &str = r#"kind = "bench-source"
rate = 1000.0
total_messages = 5"#;

fn combined_output(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn clean_finite_run_exits_zero() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("out.jsonl");
    let config =
        write_config(dir.path(), BENCH_SOURCE, &format!("kind = \"file\"\npath = {out:?}"));

    let output = run_wafer(&config);

    assert_eq!(output.status.code(), Some(0), "{}", combined_output(&output));
}

#[test]
fn invalid_source_config_exits_two_before_running() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("out.jsonl");
    let config = write_config(
        dir.path(),
        "kind = \"bench-source\"\nrate = 0.0\ntotal_messages = 5",
        &format!("kind = \"file\"\npath = {out:?}"),
    );

    let output = run_wafer(&config);

    assert_eq!(output.status.code(), Some(2), "{}", combined_output(&output));
    assert!(combined_output(&output).contains("source 'source' is invalid"));
    assert!(!out.exists(), "no node may run when a source is invalid");
}

#[test]
fn sink_init_failure_exits_three_and_names_the_node() {
    let dir = tempfile::tempdir().expect("tempdir");
    // A directory passes the parent-exists check but cannot be opened as a
    // file, so the sink fails in init() after the pipeline was spawned.
    let config = write_config(
        dir.path(),
        BENCH_SOURCE,
        &format!("kind = \"file\"\npath = {:?}", dir.path()),
    );

    let output = run_wafer(&config);

    let text = combined_output(&output);
    assert_eq!(output.status.code(), Some(3), "{text}");
    assert!(text.contains("sink 'sink' init failed"), "{text}");
}
