#![expect(
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    reason = "integration test harness: setup expects fail the test explicitly; Instant + Duration deadline math is safe within test lifetimes"
)]

use std::fs;
use std::io::Write as _;
use std::net::{SocketAddr, TcpListener};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use tempfile::TempDir;

fn free_addr() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    listener.local_addr().expect("local addr")
}

fn write_config(dir: &TempDir, api_enabled: bool, api_addr: SocketAddr) -> std::path::PathBuf {
    let output = dir.path().join("out.txt");
    let config = dir.path().join("pipeline.toml");
    fs::write(
        &config,
        format!(
            r#"
[api]
enabled = {api_enabled}
bind = "{api_addr}"

[nodes.in]
type = "source"
kind = "stdin"

[nodes.out]
type = "sink"
kind = "file"
path = "{}"

[[edges]]
from = "in"
to = "out"
"#,
            output.display()
        ),
    )
    .expect("write config");
    config
}

fn spawn_wafer(config: &std::path::Path) -> Child {
    let mut child = Command::new(env!("CARGO_BIN_EXE_wafer"))
        .arg("--config")
        .arg(config)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn wafer");

    // Keep stdin open so the stdin source keeps the pipeline alive.
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(b"control-plane-test\n")
        .expect("write stdin");

    child
}

async fn wait_for_health(addr: SocketAddr) -> reqwest::Result<reqwest::Response> {
    let url = format!("http://{addr}/health");
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        match client.get(&url).send().await {
            Ok(response) => return Ok(response),
            Err(error) if tokio::time::Instant::now() < deadline => {
                let _ = error;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => return Err(error),
        }
    }
}

fn terminate(mut child: Child) -> std::process::ExitStatus {
    #[cfg(unix)]
    {
        let status = Command::new("kill")
            .arg("-TERM")
            .arg(child.id().to_string())
            .status()
            .expect("send SIGTERM");
        assert!(status.success(), "kill -TERM failed: {status:?}");
    }

    #[cfg(not(unix))]
    child.kill().expect("kill child");

    child.wait().expect("wait child")
}

#[tokio::test]
async fn api_health_nodes_and_sigterm_shutdown() {
    let dir = tempfile::tempdir().expect("tempdir");
    let addr = free_addr();
    let config = write_config(&dir, true, addr);
    let child = spawn_wafer(&config);

    let health = wait_for_health(addr).await.expect("health should become reachable");
    assert_eq!(health.status(), reqwest::StatusCode::OK);

    let nodes: serde_json::Value = reqwest::get(format!("http://{addr}/api/v1/nodes"))
        .await
        .expect("nodes response")
        .json()
        .await
        .expect("nodes json");
    assert!(nodes.as_array().is_some(), "nodes response should be a JSON array: {nodes}");

    let status = terminate(child);
    assert!(status.success(), "wafer should exit cleanly on SIGTERM: {status:?}");
}

#[tokio::test]
async fn api_disabled_does_not_bind_health_endpoint() {
    let dir = tempfile::tempdir().expect("tempdir");
    let addr = free_addr();
    let config = write_config(&dir, false, addr);
    let mut child = spawn_wafer(&config);

    tokio::time::sleep(Duration::from_millis(300)).await;
    let result = reqwest::get(format!("http://{addr}/health")).await;
    assert!(result.is_err(), "health endpoint should not be reachable when api.enabled=false");

    drop(child.stdin.take());
    let status = child.wait().expect("wait child");
    assert!(status.success(), "wafer should exit cleanly after stdin EOF: {status:?}");
}
