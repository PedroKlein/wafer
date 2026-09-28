use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use wafer_config::UNSUPPORTED_ALLOW_INFERENCE_MESSAGE;

fn inference_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../wafer-core/tests/fixtures/inference-test-component.component.bin")
}

fn run_config(contents: &str) -> Result<Output, Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let config = dir.path().join("pipeline.toml");
    let dead_letter = format!(
        "[dead_letter]\nkind = \"file\"\npath = \"{}\"\n",
        dir.path().join("dlq.jsonl").display()
    );
    fs::write(&config, format!("{dead_letter}{contents}"))?;

    Ok(Command::new(env!("CARGO_BIN_EXE_wafer"))
        .arg("--config")
        .arg(&config)
        .arg("--no-api")
        .output()?)
}

fn require(condition: bool, message: impl Into<String>) -> Result<(), Box<dyn Error>> {
    if condition { Ok(()) } else { Err(message.into().into()) }
}

fn wasm_transform_config(allow_inference: Option<bool>) -> String {
    let plugin = inference_fixture();
    let capabilities = allow_inference.map_or_else(String::new, |allowed| {
        format!("[nodes.transform.capabilities]\nallow_inference = {allowed}\n")
    });
    format!(
        r#"
[nodes.in]
type = "source"
kind = "stdin"

[nodes.transform]
type = "transform"
plugin = "{}"

{capabilities}
[nodes.out]
type = "sink"
kind = "stdout"

[[edges]]
from = "in"
to = "transform"

[[edges]]
from = "transform"
to = "out"
"#,
        plugin.display()
    )
}

#[test]
fn granted_wasm_transform_instantiates_and_initializes() -> Result<(), Box<dyn Error>> {
    let output = run_config(&wasm_transform_config(Some(true)))?;

    require(
        output.status.success(),
        format!(
            "granted inference component should launch and initialize: {}",
            String::from_utf8_lossy(&output.stderr)
        ),
    )?;
    Ok(())
}

#[test]
fn ungranted_inference_component_fails_closed_during_preparation() -> Result<(), Box<dyn Error>> {
    for allow_inference in [None, Some(false)] {
        let output = run_config(&wasm_transform_config(allow_inference))?;

        require(!output.status.success(), "ungranted inference component must fail")?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        require(
            stderr.contains("wasi:nn/"),
            format!("stderr should identify unresolved wasi-nn imports: {stderr}"),
        )?;
        require(
            !stderr.contains("configuration validation failed"),
            format!(
                "ordinary Transform config remains valid and must fail at component preparation: {stderr}"
            ),
        )?;
    }
    Ok(())
}

#[test]
fn native_transform_cannot_obtain_inference() -> Result<(), Box<dyn Error>> {
    let output = run_config(
        r#"
[nodes.in]
type = "source"
kind = "stdin"

[nodes.transform]
type = "transform"
plugin = { kind = "native", function = "passthrough" }

[nodes.transform.capabilities]
allow_inference = true

[nodes.out]
type = "sink"
kind = "stdout"

[[edges]]
from = "in"
to = "transform"

[[edges]]
from = "transform"
to = "out"
"#,
    )?;

    require(!output.status.success(), "native Transform grant must be rejected")?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    require(stderr.contains("configuration validation failed"), stderr.to_string())?;
    require(stderr.contains(UNSUPPORTED_ALLOW_INFERENCE_MESSAGE), stderr.to_string())?;
    require(!stderr.contains("Failed to launch pipeline"), stderr.to_string())?;
    Ok(())
}
