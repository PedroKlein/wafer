use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use sha2::{Digest, Sha256};

const MODEL_SHA256: &str = "2f06e72de813a8635c9bc0397ac447a601bdbfa7df4bebc278723b958831c9bf";
const DIGIT_SHA256: &str = "e960be7e1f0d63c211aeebe76c3e49672683fed626525edd80ea1e478890f1d1";
const RAW_OUTPUT_SHA256: &str = "cc4bc94ff02cefb52b5910acb3c05d4e362c20cccf70887c9875562c914db61e";
const FORMATTED_OUTPUT_SHA256: &str =
    "09e2fa9d7643793490f2cbdf3cf5d70a7fd27d611237d6466f12a432f7d4f959";

fn repo_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(relative)
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn sha256(path: &Path) -> Result<String, Box<dyn Error>> {
    Ok(sha256_bytes(&fs::read(path)?))
}

fn require(condition: bool, message: impl Into<String>) -> Result<(), Box<dyn Error>> {
    if condition { Ok(()) } else { Err(message.into().into()) }
}

fn run_pipeline(raw_path: &Path, formatted_path: &Path) -> Result<Output, Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let config_path = dir.path().join("mnist.toml");
    let digit = repo_path("tests/fixtures/digit_7.bin");
    let inference = repo_path("crates/wafer-runtime/tests/fixtures/mnist-inference.component.bin");
    let format = repo_path("crates/wafer-runtime/tests/fixtures/result-format.component.bin");

    fs::write(
        &config_path,
        format!(
            r#"
[engine]
epoch_deadline = 1000
epoch_tick_ms = 10

[engine.fuel]
transform = 100000000

[engine.memory]
transform = 67108864

[nodes.file-source]
type = "source"
kind = "file"
path = "{}"

[nodes.mnist-inference]
type = "transform"
plugin = "{}"
plugin_version = "mnist-cpu-v1"
config = {{ execution_target = "cpu" }}

[nodes.mnist-inference.capabilities]
allow_inference = true

[nodes.raw-sink]
type = "sink"
kind = "file"
path = "{}"

[nodes.result-format]
type = "transform"
plugin = "{}"
config = {{ labels = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"], threshold = 0.0 }}

[nodes.formatted-sink]
type = "sink"
kind = "file"
path = "{}"

[[edges]]
from = "file-source"
to = "mnist-inference"

[[edges]]
from = "mnist-inference"
to = "raw-sink"

[[edges]]
from = "mnist-inference"
to = "result-format"

[[edges]]
from = "result-format"
to = "formatted-sink"
"#,
            digit.display(),
            inference.display(),
            raw_path.display(),
            format.display(),
            formatted_path.display(),
        ),
    )?;

    Ok(Command::new(env!("CARGO_BIN_EXE_wafer"))
        .arg("--config")
        .arg(config_path)
        .arg("--no-api")
        .output()?)
}

fn run_once(root: &Path) -> Result<(Vec<u8>, Vec<u8>), Box<dyn Error>> {
    let raw_path = root.join("logits.bin");
    let formatted_path = root.join("prediction.jsonl");
    let output = run_pipeline(&raw_path, &formatted_path)?;
    if !output.status.success() {
        return Err(
            format!("MNIST pipeline failed:\n{}", String::from_utf8_lossy(&output.stderr)).into()
        );
    }
    let mut raw = fs::read(raw_path)?;
    if raw.pop() != Some(b'\n') {
        return Err("raw file sink output must end with one newline".into());
    }
    Ok((raw, fs::read(formatted_path)?))
}

#[test]
fn known_digit_runs_through_real_cpu_pipeline() -> Result<(), Box<dyn Error>> {
    let model = fs::read(repo_path("models/mnist-8.onnx"))?;
    let component =
        fs::read(repo_path("crates/wafer-runtime/tests/fixtures/mnist-inference.component.bin"))?;
    require(sha256_bytes(&model) == MODEL_SHA256, "model hash changed")?;
    require(
        sha256(&repo_path("tests/fixtures/digit_7.bin"))? == DIGIT_SHA256,
        "digit fixture hash changed",
    )?;
    require(
        component.windows(model.len()).any(|window| window == model),
        "the real component must embed the checked-in model bytes",
    )?;

    let first = tempfile::tempdir()?;
    let second = tempfile::tempdir()?;
    let (raw, formatted) = run_once(first.path())?;
    let (raw_again, formatted_again) = run_once(second.path())?;
    require(raw_again == raw, "CPU logits changed across fresh Stores")?;
    require(formatted_again == formatted, "formatted output changed across fresh Stores")?;
    require(raw.len() == 10 * size_of::<f32>(), "wrong raw output size")?;
    require(sha256_bytes(&raw) == RAW_OUTPUT_SHA256, "raw output hash changed")?;
    require(sha256_bytes(&formatted) == FORMATTED_OUTPUT_SHA256, "formatted output hash changed")?;

    let logits = raw
        .as_chunks::<{ size_of::<f32>() }>()
        .0
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect::<Vec<_>>();
    require(logits.len() == 10, "expected ten logits")?;
    require(logits.iter().all(|logit| logit.is_finite()), "non-finite logit")?;
    let prediction = logits
        .iter()
        .enumerate()
        .max_by(|(_, left), (_, right)| left.total_cmp(right))
        .map(|(index, _)| index);
    require(prediction == Some(7), "expected digit 7")?;

    let json: Value = serde_json::from_slice(&formatted)?;
    require(json["prediction"] == "7", "formatted prediction changed")?;
    require(json["scores"].as_array().map(Vec::len) == Some(10), "wrong score count")?;
    Ok(())
}

#[test]
fn lifecycle_rejects_unknown_execution_target() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let config_path = dir.path().join("invalid-target.toml");
    let inference = repo_path("crates/wafer-runtime/tests/fixtures/mnist-inference.component.bin");
    fs::write(
        &config_path,
        format!(
            r#"
[nodes.stdin]
type = "source"
kind = "stdin"

[nodes.mnist-inference]
type = "transform"
plugin = "{}"
config = {{ execution_target = "cuda" }}

[nodes.mnist-inference.capabilities]
allow_inference = true

[nodes.stdout]
type = "sink"
kind = "stdout"

[[edges]]
from = "stdin"
to = "mnist-inference"

[[edges]]
from = "mnist-inference"
to = "stdout"
"#,
            inference.display()
        ),
    )?;
    let output = Command::new(env!("CARGO_BIN_EXE_wafer"))
        .arg("--config")
        .arg(config_path)
        .arg("--no-api")
        .output()?;
    require(!output.status.success(), "unknown target must fail launch")?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    require(stderr.contains("validate() rejected config"), stderr.to_string())?;
    require(stderr.contains("unknown variant `cuda`"), stderr.to_string())?;
    Ok(())
}
