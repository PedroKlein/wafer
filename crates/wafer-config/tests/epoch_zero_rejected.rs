//! AC4: `epoch_deadline = 0` fails at load time with a clear error message.
//!
//! The P0.13 lesson: `0` traps immediately on the first epoch check, which is
//! indistinguishable from a real plugin bug. The type-safe guard (`NonZeroU64` +
//! custom deserializer) makes this a parse-time error, not a runtime surprise.

use std::io::Write;
use tempfile::NamedTempFile;

use wafer_config::load_config;

#[test]
fn epoch_zero_toml_rejected_at_load() {
    let mut file = NamedTempFile::new().unwrap();
    write!(
        file,
        r#"
[engine]
epoch_deadline = 0

[nodes.src]
type = "source"
kind = "stdin"

[nodes.snk]
type = "sink"
kind = "stdout"

[[edges]]
from = "src"
to = "snk"
"#
    )
    .unwrap();

    let result = load_config(file.path());
    let err = result.expect_err("epoch_deadline = 0 must be rejected at load time");
    let msg = err.to_string();
    assert!(
        msg.contains("trap") || msg.contains('0') || msg.contains("metering"),
        "error message should explain why 0 is invalid: {msg}"
    );
}

#[test]
fn fuel_zero_toml_rejected_at_load() {
    let mut file = NamedTempFile::new().unwrap();
    write!(
        file,
        r#"
[engine.fuel]
transform = 0

[nodes.src]
type = "source"
kind = "stdin"

[nodes.snk]
type = "sink"
kind = "stdout"

[[edges]]
from = "src"
to = "snk"
"#
    )
    .unwrap();

    let result = load_config(file.path());
    assert!(result.is_err(), "fuel.transform = 0 must be rejected at load time");
}

#[test]
fn per_node_fuel_zero_rejected_at_load() {
    let mut file = NamedTempFile::new().unwrap();
    write!(
        file,
        r#"
[nodes.src]
type = "source"
kind = "stdin"

[nodes.transform]
type = "transform"
plugin = "t.wasm"
fuel = 0

[nodes.snk]
type = "sink"
kind = "stdout"

[[edges]]
from = "src"
to = "transform"

[[edges]]
from = "transform"
to = "snk"
"#
    )
    .unwrap();

    let result = load_config(file.path());
    assert!(result.is_err(), "per-node fuel = 0 must be rejected at load time");
}
