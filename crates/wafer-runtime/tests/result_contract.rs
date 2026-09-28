//! A small native run writes exactly the artifacts `eval/result-schema.json`
//! lists for the runtime, with the listed headers and keys, and
//! `eval/RESULT-CONTRACT.md` names every one of them.

#![cfg(test)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

const CONFIG: &str = r#"
[pipeline]
name = "result-contract"

[nodes.source]
type = "source"
kind = "bench-source"
rate = 1000.0
total_messages = 300
warmup_messages = 100
payload_size = 16

[nodes.transform]
type = "transform"
plugin = { kind = "native", function = "passthrough" }

[nodes.sink]
type = "sink"
kind = "bench-sink"
warmup_secs = 0
track_sequences = true

[[edges]]
from = "source"
to = "transform"

[[edges]]
from = "transform"
to = "sink"

[dead_letter]
kind = "file"
path = "dlq.jsonl"
"#;

fn repo_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join(relative)
}

fn read_schema() -> Value {
    serde_json::from_slice(&std::fs::read(repo_path("eval/result-schema.json")).unwrap()).unwrap()
}

fn run_runtime(output: &Path) {
    let config = output.join("config.toml");
    std::fs::write(&config, CONFIG).unwrap();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_wafer"))
        .arg("--config")
        .arg(&config)
        .arg("--no-api")
        .stdout(std::process::Stdio::null())
        .env("WAFER_BENCH_OUTPUT_DIR", output)
        .env("WAFER_QUEUE_DEPTH_OUTPUT", output.join("queue-depth.csv"))
        .spawn()
        .unwrap();
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        // Shutdown hashes the runtime binary, which is slow for a debug build.
        if started.elapsed() >= Duration::from_secs(180) {
            child.kill().unwrap();
            panic!("runtime timed out");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "runtime exited with {status}");
    std::fs::remove_file(config).unwrap();
}

fn assert_matches(dir: &Path, name: &str, expected: &Value) {
    let path = dir.join(name);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
    if let Some(header) = expected["csv_header"].as_str() {
        let text = String::from_utf8(bytes).unwrap();
        assert_eq!(text.lines().next(), Some(header), "{name} header");
    } else if let Some(keys) = expected["json_keys"].as_array() {
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_object_keys(name, &value, keys);
    } else if let Some(keys) = expected["jsonl_keys"].as_array() {
        for line in String::from_utf8(bytes).unwrap().lines() {
            let value: Value = serde_json::from_str(line).unwrap();
            assert_object_keys(name, &value, keys);
        }
    } else {
        assert_eq!(expected["hdr_encoding"], "interval-log", "{name}: unknown schema entry");
        assert!(bytes.starts_with(b"#"), "{name} is not an interval log");
    }
}

fn assert_object_keys(name: &str, value: &Value, keys: &[Value]) {
    let actual: BTreeSet<&str> = value.as_object().unwrap().keys().map(String::as_str).collect();
    let expected: BTreeSet<&str> = keys.iter().map(|key| key.as_str().unwrap()).collect();
    assert_eq!(actual, expected, "{name} keys");
}

#[test]
fn native_run_writes_the_artifacts_the_schema_lists() {
    let schema = read_schema();
    let expected = schema["producers"]["wafer-runtime"].as_object().unwrap();
    let output = tempfile::tempdir().unwrap();

    run_runtime(output.path());

    let written: BTreeSet<String> = std::fs::read_dir(output.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    let listed: BTreeSet<String> = expected.keys().cloned().collect();
    assert_eq!(written, listed, "written artifacts differ from eval/result-schema.json");
    for (name, entry) in expected {
        assert_matches(output.path(), name, entry);
    }
}

#[test]
fn contract_documents_every_schema_entry() {
    let schema = read_schema();
    let contract = std::fs::read_to_string(repo_path("eval/RESULT-CONTRACT.md")).unwrap();
    let mut missing = Vec::new();
    for (producer, artifacts) in schema["producers"].as_object().unwrap() {
        for (name, entry) in artifacts.as_object().unwrap() {
            let names = entry["csv_header"]
                .as_str()
                .map(|header| vec![header])
                .or_else(|| {
                    entry["json_keys"]
                        .as_array()
                        .or_else(|| entry["jsonl_keys"].as_array())
                        .map(|keys| keys.iter().filter_map(Value::as_str).collect())
                })
                .unwrap_or_default();
            for documented in std::iter::once(name.as_str()).chain(names) {
                if !contract.contains(&format!("`{documented}`")) {
                    missing.push(format!("{producer} {name}: `{documented}`"));
                }
            }
        }
    }
    assert!(missing.is_empty(), "RESULT-CONTRACT.md does not name:\n{}", missing.join("\n"));
}
