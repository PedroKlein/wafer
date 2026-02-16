//! Integration tests for WAFER MVP pipeline.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const CONFIG_PATH: &str = "examples/dag-passthrough.toml";

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
        combined.contains("pipeline started") || combined.contains("Pipeline started"),
        "Expected 'pipeline started' in logs, got: {}",
        combined
    );

    assert!(
        combined.contains("pipeline stopped") || combined.contains("Pipeline stopped"),
        "Expected 'pipeline stopped' in logs, got: {}",
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

use tempfile::tempdir;
use wafer_poc::config::{DagConfig, EdgeDefinition, NodeDefinition, NodeType};
use wafer_poc::dag::DagOrchestrator;
use wafer_poc::engine::{Capabilities, TransformInstance, WaferEngine};
use wafer_poc::node::{AnyNode, FileSink, FileSource, Lifecycle, NodeConfig, WasmTransform};

fn plugin_path() -> PathBuf {
    project_root().join("plugins/pass-through/target/wasm32-wasip2/release/pass_through_transform.wasm")
}

fn uppercase_plugin_path() -> PathBuf {
    project_root().join("plugins/uppercase/target/wasm32-wasip2/release/uppercase_transform.wasm")
}

fn json_parse_plugin_path() -> PathBuf {
    project_root().join("plugins/json-parse/target/wasm32-wasip2/release/json_parse_transform.wasm")
}

fn filter_plugin_path() -> PathBuf {
    project_root().join("plugins/filter/target/wasm32-wasip2/release/filter_transform.wasm")
}

fn tensor_prep_plugin_path() -> PathBuf {
    project_root().join("plugins/tensor-prep/target/wasm32-wasip2/release/tensor_prep.wasm")
}

fn result_format_plugin_path() -> PathBuf {
    project_root().join("plugins/result-format/target/wasm32-wasip2/release/result_format.wasm")
}

async fn create_wasm_transform(id: &str) -> WasmTransform {
    let engine = WaferEngine::new().expect("Failed to create engine");
    let component = engine
        .load_component(plugin_path())
        .expect("Failed to load plugin");
    let instance = TransformInstance::new(&engine, &component, Capabilities::with_stdio())
        .await
        .expect("Failed to create instance");
    let config = NodeConfig::new(id, "transform/passthrough");
    WasmTransform::new(engine, instance, config)
}

async fn create_uppercase_transform(id: &str) -> WasmTransform {
    let engine = WaferEngine::new().expect("Failed to create engine");
    let component = engine
        .load_component(uppercase_plugin_path())
        .expect("Failed to load uppercase plugin");
    let instance = TransformInstance::new(&engine, &component, Capabilities::with_stdio())
        .await
        .expect("Failed to create uppercase instance");
    let config = NodeConfig::new(id, "transform/uppercase");
    WasmTransform::new(engine, instance, config)
}

async fn create_json_parse_transform(id: &str) -> WasmTransform {
    let engine = WaferEngine::new().expect("Failed to create engine");
    let component = engine
        .load_component(json_parse_plugin_path())
        .expect("Failed to load json-parse plugin");
    let instance = TransformInstance::new(&engine, &component, Capabilities::with_stdio())
        .await
        .expect("Failed to create json-parse instance");
    let config = NodeConfig::new(id, "transform/json-parse");
    WasmTransform::new(engine, instance, config)
}

async fn create_filter_transform(id: &str, pattern: &str) -> WasmTransform {
    let engine = WaferEngine::new().expect("Failed to create engine");
    let component = engine
        .load_component(filter_plugin_path())
        .expect("Failed to load filter plugin");
    let instance = TransformInstance::new(&engine, &component, Capabilities::with_stdio())
        .await
        .expect("Failed to create filter instance");
    let config_toml = format!("pattern = \"{}\"", pattern);
    let config =
        NodeConfig::new(id, "transform/filter").with_config_bytes(config_toml.into_bytes());
    WasmTransform::new(engine, instance, config)
}

async fn create_tensor_prep_transform(id: &str) -> WasmTransform {
    let engine = WaferEngine::new().expect("Failed to create engine");
    let component = engine
        .load_component(tensor_prep_plugin_path())
        .expect("Failed to load tensor-prep plugin");
    let instance = TransformInstance::new(&engine, &component, Capabilities::with_stdio())
        .await
        .expect("Failed to create tensor-prep instance");
    let config = NodeConfig::new(id, "transform/tensor-prep");
    WasmTransform::new(engine, instance, config)
}

async fn create_result_format_transform(id: &str) -> WasmTransform {
    let engine = WaferEngine::new().expect("Failed to create engine");
    let component = engine
        .load_component(result_format_plugin_path())
        .expect("Failed to load result-format plugin");
    let instance = TransformInstance::new(&engine, &component, Capabilities::with_stdio())
        .await
        .expect("Failed to create result-format instance");
    let config = NodeConfig::new(id, "transform/result-format");
    WasmTransform::new(engine, instance, config)
}

#[tokio::test]
async fn test_dag_source_transform_sink() {
    let dir = tempdir().expect("Failed to create temp dir");
    let input_path = dir.path().join("input.txt");
    let output_path = dir.path().join("output.txt");

    std::fs::write(&input_path, "line1\nline2\nline3\n").expect("Failed to write input");

    let config = DagConfig {
        nodes: vec![
            NodeDefinition {
                id: "source".to_string(),
                node_type: NodeType::Source,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
            },
            NodeDefinition {
                id: "transform".to_string(),
                node_type: NodeType::Transform,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
            },
            NodeDefinition {
                id: "sink".to_string(),
                node_type: NodeType::Sink,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
            },
        ],
        edges: vec![
            EdgeDefinition {
                from: "source".to_string(),
                to: "transform".to_string(),
                queue_capacity: None,
            },
            EdgeDefinition {
                from: "transform".to_string(),
                to: "sink".to_string(),
                queue_capacity: None,
            },
        ],
        default_queue_capacity: 1024,
    };

    let mut orchestrator = DagOrchestrator::from_config(config).expect("Failed to create orchestrator");

    let source = FileSource::new("source", &input_path);
    let transform = create_wasm_transform("transform").await;
    let sink = FileSink::new("sink", output_path.clone());

    orchestrator
        .register_node("source", AnyNode::from_source(source))
        .expect("Failed to register source");
    orchestrator
        .register_node("transform", AnyNode::from_transform(transform))
        .expect("Failed to register transform");
    orchestrator
        .register_node("sink", AnyNode::from_sink(sink))
        .expect("Failed to register sink");

    orchestrator.wire_queues().expect("Failed to wire queues");
    orchestrator.run().await.expect("Failed to run DAG");

    let output = std::fs::read_to_string(&output_path).expect("Failed to read output");
    assert_eq!(output, "line1\nline2\nline3\n");
}

#[tokio::test]
async fn test_dag_two_transforms() {
    let dir = tempdir().expect("Failed to create temp dir");
    let input_path = dir.path().join("input.txt");
    let output_path = dir.path().join("output.txt");

    std::fs::write(&input_path, "alpha\nbeta\ngamma\ndelta\n").expect("Failed to write input");

    let config = DagConfig {
        nodes: vec![
            NodeDefinition {
                id: "source".to_string(),
                node_type: NodeType::Source,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
            },
            NodeDefinition {
                id: "transform-1".to_string(),
                node_type: NodeType::Transform,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
            },
            NodeDefinition {
                id: "transform-2".to_string(),
                node_type: NodeType::Transform,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
            },
            NodeDefinition {
                id: "sink".to_string(),
                node_type: NodeType::Sink,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
            },
        ],
        edges: vec![
            EdgeDefinition {
                from: "source".to_string(),
                to: "transform-1".to_string(),
                queue_capacity: None,
            },
            EdgeDefinition {
                from: "transform-1".to_string(),
                to: "transform-2".to_string(),
                queue_capacity: None,
            },
            EdgeDefinition {
                from: "transform-2".to_string(),
                to: "sink".to_string(),
                queue_capacity: None,
            },
        ],
        default_queue_capacity: 1024,
    };

    let mut orchestrator = DagOrchestrator::from_config(config).expect("Failed to create orchestrator");

    let source = FileSource::new("source", &input_path);
    let transform1 = create_wasm_transform("transform-1").await;
    let transform2 = create_wasm_transform("transform-2").await;
    let sink = FileSink::new("sink", output_path.clone());

    orchestrator
        .register_node("source", AnyNode::from_source(source))
        .expect("Failed to register source");
    orchestrator
        .register_node("transform-1", AnyNode::from_transform(transform1))
        .expect("Failed to register transform-1");
    orchestrator
        .register_node("transform-2", AnyNode::from_transform(transform2))
        .expect("Failed to register transform-2");
    orchestrator
        .register_node("sink", AnyNode::from_sink(sink))
        .expect("Failed to register sink");

    orchestrator.wire_queues().expect("Failed to wire queues");
    orchestrator.run().await.expect("Failed to run DAG");

    let output = std::fs::read_to_string(&output_path).expect("Failed to read output");
    assert_eq!(output, "alpha\nbeta\ngamma\ndelta\n");
}

#[tokio::test]
async fn test_dag_empty_input() {
    let dir = tempdir().expect("Failed to create temp dir");
    let input_path = dir.path().join("input.txt");
    let output_path = dir.path().join("output.txt");

    std::fs::write(&input_path, "").expect("Failed to write empty input");

    let config = DagConfig {
        nodes: vec![
            NodeDefinition {
                id: "source".to_string(),
                node_type: NodeType::Source,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
            },
            NodeDefinition {
                id: "transform".to_string(),
                node_type: NodeType::Transform,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
            },
            NodeDefinition {
                id: "sink".to_string(),
                node_type: NodeType::Sink,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
            },
        ],
        edges: vec![
            EdgeDefinition {
                from: "source".to_string(),
                to: "transform".to_string(),
                queue_capacity: None,
            },
            EdgeDefinition {
                from: "transform".to_string(),
                to: "sink".to_string(),
                queue_capacity: None,
            },
        ],
        default_queue_capacity: 1024,
    };

    let mut orchestrator = DagOrchestrator::from_config(config).expect("Failed to create orchestrator");

    let source = FileSource::new("source", &input_path);
    let transform = create_wasm_transform("transform").await;
    let sink = FileSink::new("sink", output_path.clone());

    orchestrator
        .register_node("source", AnyNode::from_source(source))
        .expect("Failed to register source");
    orchestrator
        .register_node("transform", AnyNode::from_transform(transform))
        .expect("Failed to register transform");
    orchestrator
        .register_node("sink", AnyNode::from_sink(sink))
        .expect("Failed to register sink");

    orchestrator.wire_queues().expect("Failed to wire queues");
    orchestrator.run().await.expect("Failed to run DAG with empty input");

    let output = std::fs::read_to_string(&output_path).expect("Failed to read output");
    assert_eq!(output, "");
}

#[tokio::test]
async fn test_dag_large_file() {
    let dir = tempdir().expect("Failed to create temp dir");
    let input_path = dir.path().join("input.txt");
    let output_path = dir.path().join("output.txt");

    let lines: Vec<String> = (0..1000).map(|i| format!("line-{i}")).collect();
    let input_content = lines.join("\n") + "\n";
    std::fs::write(&input_path, &input_content).expect("Failed to write large input");

    let config = DagConfig {
        nodes: vec![
            NodeDefinition {
                id: "source".to_string(),
                node_type: NodeType::Source,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
            },
            NodeDefinition {
                id: "transform".to_string(),
                node_type: NodeType::Transform,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
            },
            NodeDefinition {
                id: "sink".to_string(),
                node_type: NodeType::Sink,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
            },
        ],
        edges: vec![
            EdgeDefinition {
                from: "source".to_string(),
                to: "transform".to_string(),
                queue_capacity: None,
            },
            EdgeDefinition {
                from: "transform".to_string(),
                to: "sink".to_string(),
                queue_capacity: None,
            },
        ],
        default_queue_capacity: 2048,
    };

    let mut orchestrator = DagOrchestrator::from_config(config).expect("Failed to create orchestrator");

    let source = FileSource::new("source", &input_path);
    let transform = create_wasm_transform("transform").await;
    let sink = FileSink::new("sink", output_path.clone());

    orchestrator
        .register_node("source", AnyNode::from_source(source))
        .expect("Failed to register source");
    orchestrator
        .register_node("transform", AnyNode::from_transform(transform))
        .expect("Failed to register transform");
    orchestrator
        .register_node("sink", AnyNode::from_sink(sink))
        .expect("Failed to register sink");

    orchestrator.wire_queues().expect("Failed to wire queues");
    orchestrator.run().await.expect("Failed to run DAG with large file");

    let output = std::fs::read_to_string(&output_path).expect("Failed to read output");
    let output_lines: Vec<&str> = output.lines().collect();
    assert_eq!(output_lines.len(), 1000);
    assert_eq!(output_lines[0], "line-0");
    assert_eq!(output_lines[999], "line-999");
}

#[tokio::test]
async fn test_dag_uppercase_transform() {
    let dir = tempdir().expect("Failed to create temp dir");
    let input_path = dir.path().join("input.txt");
    let output_path = dir.path().join("output.txt");

    std::fs::write(&input_path, "hello world\n").expect("Failed to write input");

    let config = DagConfig {
        nodes: vec![
            NodeDefinition {
                id: "source".to_string(),
                node_type: NodeType::Source,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
            },
            NodeDefinition {
                id: "transform".to_string(),
                node_type: NodeType::Transform,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
            },
            NodeDefinition {
                id: "sink".to_string(),
                node_type: NodeType::Sink,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
            },
        ],
        edges: vec![
            EdgeDefinition {
                from: "source".to_string(),
                to: "transform".to_string(),
                queue_capacity: None,
            },
            EdgeDefinition {
                from: "transform".to_string(),
                to: "sink".to_string(),
                queue_capacity: None,
            },
        ],
        default_queue_capacity: 1024,
    };

    let mut orchestrator = DagOrchestrator::from_config(config).expect("Failed to create orchestrator");

    let source = FileSource::new("source", &input_path);
    let transform = create_uppercase_transform("transform").await;
    let sink = FileSink::new("sink", output_path.clone());

    orchestrator
        .register_node("source", AnyNode::from_source(source))
        .expect("Failed to register source");
    orchestrator
        .register_node("transform", AnyNode::from_transform(transform))
        .expect("Failed to register transform");
    orchestrator
        .register_node("sink", AnyNode::from_sink(sink))
        .expect("Failed to register sink");

    orchestrator.wire_queues().expect("Failed to wire queues");
    orchestrator.run().await.expect("Failed to run DAG");

    let output = std::fs::read_to_string(&output_path).expect("Failed to read output");
    assert_eq!(output, "HELLO WORLD\n");
}

#[tokio::test]
async fn test_dag_json_parse_transform() {
    let dir = tempdir().expect("Failed to create temp dir");
    let input_path = dir.path().join("input.txt");
    let output_path = dir.path().join("output.txt");

    std::fs::write(&input_path, "{\"key\":\"value\"}\n").expect("Failed to write input");

    let config = DagConfig {
        nodes: vec![
            NodeDefinition {
                id: "source".to_string(),
                node_type: NodeType::Source,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
            },
            NodeDefinition {
                id: "transform".to_string(),
                node_type: NodeType::Transform,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
            },
            NodeDefinition {
                id: "sink".to_string(),
                node_type: NodeType::Sink,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
            },
        ],
        edges: vec![
            EdgeDefinition {
                from: "source".to_string(),
                to: "transform".to_string(),
                queue_capacity: None,
            },
            EdgeDefinition {
                from: "transform".to_string(),
                to: "sink".to_string(),
                queue_capacity: None,
            },
        ],
        default_queue_capacity: 1024,
    };

    let mut orchestrator = DagOrchestrator::from_config(config).expect("Failed to create orchestrator");

    let source = FileSource::new("source", &input_path);
    let transform = create_json_parse_transform("transform").await;
    let sink = FileSink::new("sink", output_path.clone());

    orchestrator
        .register_node("source", AnyNode::from_source(source))
        .expect("Failed to register source");
    orchestrator
        .register_node("transform", AnyNode::from_transform(transform))
        .expect("Failed to register transform");
    orchestrator
        .register_node("sink", AnyNode::from_sink(sink))
        .expect("Failed to register sink");

    orchestrator.wire_queues().expect("Failed to wire queues");
    orchestrator.run().await.expect("Failed to run DAG");

    let output = std::fs::read_to_string(&output_path).expect("Failed to read output");
    assert!(output.contains("\"key\""), "Expected key in output: {}", output);
    assert!(output.contains("\"value\""), "Expected value in output: {}", output);
    assert!(output.contains(": ") || output.contains(":\n"), "Expected pretty-printed JSON: {}", output);
}

#[tokio::test]
async fn test_dag_filter_transform() {
    let dir = tempdir().expect("Failed to create temp dir");
    let input_path = dir.path().join("input.txt");
    let output_path = dir.path().join("output.txt");

    std::fs::write(&input_path, "info: startup\ndebug: trace\ninfo: ready\ndebug: data\n")
        .expect("Failed to write input");

    let config = DagConfig {
        nodes: vec![
            NodeDefinition {
                id: "source".to_string(),
                node_type: NodeType::Source,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
            },
            NodeDefinition {
                id: "transform".to_string(),
                node_type: NodeType::Transform,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
            },
            NodeDefinition {
                id: "sink".to_string(),
                node_type: NodeType::Sink,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
            },
        ],
        edges: vec![
            EdgeDefinition {
                from: "source".to_string(),
                to: "transform".to_string(),
                queue_capacity: None,
            },
            EdgeDefinition {
                from: "transform".to_string(),
                to: "sink".to_string(),
                queue_capacity: None,
            },
        ],
        default_queue_capacity: 1024,
    };

    let mut orchestrator = DagOrchestrator::from_config(config).expect("Failed to create orchestrator");

    let source = FileSource::new("source", &input_path);
    let transform = create_filter_transform("transform", "debug").await;
    let sink = FileSink::new("sink", output_path.clone());

    orchestrator
        .register_node("source", AnyNode::from_source(source))
        .expect("Failed to register source");
    orchestrator
        .register_node("transform", AnyNode::from_transform(transform))
        .expect("Failed to register transform");
    orchestrator
        .register_node("sink", AnyNode::from_sink(sink))
        .expect("Failed to register sink");

    orchestrator.wire_queues().expect("Failed to wire queues");
    orchestrator.run().await.expect("Failed to run DAG");

    let output = std::fs::read_to_string(&output_path).expect("Failed to read output");
    assert!(!output.contains("debug"), "Expected debug lines to be filtered out: {}", output);
    assert!(output.contains("info: startup"), "Expected info lines to remain: {}", output);
    assert!(output.contains("info: ready"), "Expected info lines to remain: {}", output);
}

#[tokio::test]
async fn test_dag_filter_no_match() {
    let dir = tempdir().expect("Failed to create temp dir");
    let input_path = dir.path().join("input.txt");
    let output_path = dir.path().join("output.txt");

    std::fs::write(&input_path, "info: startup\nwarn: caution\ninfo: ready\n")
        .expect("Failed to write input");

    let config = DagConfig {
        nodes: vec![
            NodeDefinition {
                id: "source".to_string(),
                node_type: NodeType::Source,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
            },
            NodeDefinition {
                id: "transform".to_string(),
                node_type: NodeType::Transform,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
            },
            NodeDefinition {
                id: "sink".to_string(),
                node_type: NodeType::Sink,
                source_type: None,
                sink_type: None,
                config: toml::Value::Table(toml::map::Map::new()),
            },
        ],
        edges: vec![
            EdgeDefinition {
                from: "source".to_string(),
                to: "transform".to_string(),
                queue_capacity: None,
            },
            EdgeDefinition {
                from: "transform".to_string(),
                to: "sink".to_string(),
                queue_capacity: None,
            },
        ],
        default_queue_capacity: 1024,
    };

    let mut orchestrator = DagOrchestrator::from_config(config).expect("Failed to create orchestrator");

    let source = FileSource::new("source", &input_path);
    let transform = create_filter_transform("transform", "ERROR").await;
    let sink = FileSink::new("sink", output_path.clone());

    orchestrator
        .register_node("source", AnyNode::from_source(source))
        .expect("Failed to register source");
    orchestrator
        .register_node("transform", AnyNode::from_transform(transform))
        .expect("Failed to register transform");
    orchestrator
        .register_node("sink", AnyNode::from_sink(sink))
        .expect("Failed to register sink");

    orchestrator.wire_queues().expect("Failed to wire queues");
    orchestrator.run().await.expect("Failed to run DAG");

    let output = std::fs::read_to_string(&output_path).expect("Failed to read output");
    assert_eq!(output, "info: startup\nwarn: caution\ninfo: ready\n", "No lines should be filtered when pattern doesn't match");
}

// ============================================================================
// MNIST / wasi-nn integration tests
// ============================================================================

use wafer_poc::node::Transform;
use wafer_poc::queue::RuntimeEnvelope;

#[tokio::test]
async fn test_dag_tensor_prep_transform() {
    let mut transform = create_tensor_prep_transform("tensor-prep").await;
    transform.init().await.expect("Failed to init transform");

    let input_255 = RuntimeEnvelope::new("test", vec![255u8; 784]);
    let result = transform.process(input_255).await.expect("Failed to process");

    match result {
        wafer_poc::node::ProcessResult::Emit(envelope) => {
            assert_eq!(envelope.payload.len(), 3136, "Expected 3136 bytes (784 F32 values × 4 bytes)");
            let first_f32 = f32::from_le_bytes([
                envelope.payload[0],
                envelope.payload[1],
                envelope.payload[2],
                envelope.payload[3],
            ]);
            assert!(
                (first_f32 - 1.0).abs() < 0.001,
                "Expected first value ~1.0 (normalized from 255), got {}",
                first_f32
            );
        }
        other => panic!("Expected Emit result, got {:?}", other),
    }

    let input_0 = RuntimeEnvelope::new("test", vec![0u8; 784]);
    let result_0 = transform.process(input_0).await.expect("Failed to process zeros");

    match result_0 {
        wafer_poc::node::ProcessResult::Emit(envelope) => {
            assert_eq!(envelope.payload.len(), 3136);
            let zero_f32 = f32::from_le_bytes([
                envelope.payload[0],
                envelope.payload[1],
                envelope.payload[2],
                envelope.payload[3],
            ]);
            assert!(
                zero_f32.abs() < 0.001,
                "Expected first value ~0.0 (normalized from 0), got {}",
                zero_f32
            );
        }
        other => panic!("Expected Emit result, got {:?}", other),
    }

    transform.close().await.expect("Failed to close transform");
}

#[tokio::test]
async fn test_dag_result_format_transform() {
    let mut transform = create_result_format_transform("result-format").await;
    transform.init().await.expect("Failed to init transform");

    let logits_with_digit_7_highest: [f32; 10] = [0.01, 0.01, 0.01, 0.01, 0.01, 0.01, 0.01, 0.95, 0.01, 0.01];
    let mut input_bytes = Vec::with_capacity(40);
    for &val in &logits_with_digit_7_highest {
        input_bytes.extend_from_slice(&val.to_le_bytes());
    }
    assert_eq!(input_bytes.len(), 40, "Input should be 40 bytes (10 F32 values)");

    let input = RuntimeEnvelope::new("test", input_bytes);
    let result = transform.process(input).await.expect("Failed to process");

    match result {
        wafer_poc::node::ProcessResult::Emit(envelope) => {
            let output = String::from_utf8_lossy(&envelope.payload);
            
            assert!(output.contains("\"digit\""), "Expected 'digit' field in JSON: {}", output);
            assert!(output.contains("\"confidence\""), "Expected 'confidence' field in JSON: {}", output);
            assert!(output.contains("\"all_scores\""), "Expected 'all_scores' field in JSON: {}", output);
            
            let json: serde_json::Value = serde_json::from_str(&output)
                .expect("Output should be valid JSON");
            assert_eq!(json["digit"], 7, "Expected digit to be 7");
        }
        other => panic!("Expected Emit result, got {:?}", other),
    }

    transform.close().await.expect("Failed to close transform");
}

#[test]
fn test_dag_mnist_inference_pipeline() {
    let output = Command::new(wafer_binary())
        .args(["--config", "examples/dag-mnist-inference.toml"])
        .current_dir(project_root())
        .output()
        .expect("Failed to execute");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let wasi_nn_or_plugin_not_ready = stderr.contains("wasi") 
            || stderr.contains("nn") 
            || stderr.contains("model")
            || stderr.contains("PluginInit")
            || stderr.contains("no exported instance");
        if wasi_nn_or_plugin_not_ready {
            eprintln!("MNIST inference test skipped: wasi-nn or plugin not ready");
            eprintln!("stderr: {}", stderr);
            return;
        }
        panic!(
            "Process failed with unexpected error.\nstderr: {}\nstdout: {}",
            stderr,
            String::from_utf8_lossy(&output.stdout)
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    
    assert!(
        stdout.contains("\"digit\""),
        "Expected 'digit' field in output: {}",
        stdout
    );
    assert!(
        stdout.contains("\"confidence\""),
        "Expected 'confidence' field in output: {}",
        stdout
    );
}
