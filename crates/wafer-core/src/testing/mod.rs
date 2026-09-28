//! Test utilities for WAFER pipeline integration and unit tests.
//!
//! Provides in-memory Source/Sink implementations (channel-backed) that
//! decouple tests from real I/O (MQTT, files, HTTP), and a harness that calls
//! pre-built Wasm plugins directly. Compiled only for tests and benches, or
//! with the `test-support` feature.

use std::path::Path;

pub mod channel;
pub mod harness;

pub use channel::{ChannelSink, ChannelSource};
pub use harness::{PluginTestHarness, TransformHarness};

/// Environment switch that turns a missing test artifact into a failure.
pub const REQUIRE_PLUGINS_ENV: &str = "WAFER_REQUIRE_PLUGINS";

/// Whether the pre-built artifact a test needs is on disk.
///
/// A missing file panics when [`REQUIRE_PLUGINS_ENV`] is `1`, which `mise run
/// test` and CI set after building the plugins. Otherwise it prints a `SKIP`
/// line so the run shows what it did not check, and the caller returns early.
///
/// # Panics
///
/// When the file is missing and [`REQUIRE_PLUGINS_ENV`] is `1`.
#[must_use]
#[expect(clippy::print_stderr, reason = "a skipped test must be visible in the test output")]
pub fn artifact_available(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    if path.is_file() {
        return true;
    }
    let required = std::env::var(REQUIRE_PLUGINS_ENV).is_ok_and(|value| value == "1");
    assert!(
        !required,
        "{} is not built (run `mise run //plugins:build-plugins`); unset {REQUIRE_PLUGINS_ENV} to skip instead",
        path.display()
    );
    eprintln!("SKIP: {} is not built (run `mise run //plugins:build-plugins`)", path.display());
    false
}
