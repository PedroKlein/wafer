#![cfg(test)]
#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// A line typed into a running pipeline reaches the stdout sink while stdin
/// stays open, and closing stdin then ends the run.
#[test]
fn stdin_line_is_delivered_before_input_closes() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("pipeline.toml");
    std::fs::write(
        &config,
        format!(
            r#"
[nodes.source]
type = "source"
kind = "stdin"

[nodes.sink]
type = "sink"
kind = "stdout"

[[edges]]
from = "source"
to = "sink"

[dead_letter]
kind = "file"
path = "{}"
"#,
            dir.path().join("dlq.jsonl").display()
        ),
    )
    .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_wafer"))
        .arg("--config")
        .arg(&config)
        .arg("--no-api")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    let (lines, received) = mpsc::channel();
    std::thread::spawn(move || {
        for line in stdout.lines().map_while(Result::ok) {
            if lines.send(line).is_err() {
                return;
            }
        }
    });

    writeln!(stdin, "marker-line").unwrap();
    stdin.flush().unwrap();
    let delivered = loop {
        match received.recv_timeout(Duration::from_secs(20)) {
            Ok(line) if line.contains("marker-line") => break true,
            Ok(_) => {}
            Err(_) => break false,
        }
    };
    drop(stdin);
    let status = child.wait().unwrap();

    assert!(delivered, "line was not written to stdout while stdin was open");
    assert!(status.success(), "{status}");
}
