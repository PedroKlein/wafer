#![cfg(test)]
#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

use wafer_core::testing::artifact_available;

fn repo_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join(relative)
}

fn send_sigterm(child: &Child) {
    let status = Command::new("kill").args(["-TERM", &child.id().to_string()]).status().unwrap();
    assert!(status.success());
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> Option<ExitStatus> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if let Some(status) = child.try_wait().unwrap() {
            return Some(status);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

/// A guest that never returns, with no fuel or epoch limit, keeps the graceful
/// shutdown from finishing. The second signal must still end the process.
#[test]
fn second_sigterm_exits_while_a_guest_blocks_the_graceful_shutdown() {
    let plugin = repo_path(
        "plugins/attacks/infinite-loop/target/wasm32-wasip2/release/wafer_attack_infinite_loop.wasm",
    );
    if !artifact_available(&plugin) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("pipeline.toml");
    std::fs::write(
        &config,
        format!(
            r#"
[pipeline]
name = "unbounded-guest"

[nodes.source]
type = "source"
kind = "bench-source"
rate = 100.0
total_messages = 1
warmup_messages = 0
payload_size = 16

[nodes.spin]
type = "transform"
plugin = "{}"

[nodes.sink]
type = "sink"
kind = "bench-sink"
warmup_secs = 0

[[edges]]
from = "source"
to = "spin"

[[edges]]
from = "spin"
to = "sink"
"#,
            plugin.display()
        ),
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_wafer"))
        .arg("--config")
        .arg(&config)
        .arg("--no-api")
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_secs(3));

    send_sigterm(&child);
    let after_first = wait_for_exit(&mut child, Duration::from_secs(1));
    assert!(after_first.is_none(), "runtime exited on the first signal: {after_first:?}");
    send_sigterm(&child);
    let after_second = wait_for_exit(&mut child, Duration::from_secs(5));

    if after_second.is_none() {
        child.kill().unwrap();
    }
    assert_eq!(after_second.and_then(|status| status.code()), Some(143));
}
