//! Pass-through v1 — hot-swap experiment version marker.
//!
//! Reads its `plugin_version` from `NodeConfig` at `init()` and stamps
//! `plugin.version = "1.0.0"` into every outgoing envelope's metadata. This
//! is what `BenchSink::HotSwapRecorder` observes at the sink to detect the
//! v1 → v2 boundary during E-Swap-1 pause measurement.
//!
//! Payload is echoed unchanged (same as base pass-through). The distinction
//! between v1 and v2 is purely the version stamp and (for v2) an additional
//! `swapped = "true"` metadata key.
//!
//! # Config
//! Accepts `NodeConfig.plugin-version` (empty means the operator did not set
//! `[nodes.<id>].plugin_version` in TOML — treat as 1.0.0).

wit_bindgen::generate!({
    path: "../../wit/node",
    world: "transform-node",
    generate_all,
});

use std::sync::Mutex;

use exports::pipeline::node::lifecycle::NodeConfig;
use exports::pipeline::node::transform::{Message, OutputMessage, ProcessError};

/// Fallback version when the operator did not stamp one in TOML.
const DEFAULT_VERSION: &str = "1.0.0";

/// Version reported to the sink via output metadata. Set at `init()`, read
/// on every `process()`. Wasm is single-threaded so the Mutex is never
/// contested, but using `Mutex<Option<String>>` sidesteps Rust 2024's ban
/// on `static mut` references.
static PLUGIN_VERSION: Mutex<Option<String>> = Mutex::new(None);

fn plugin_version() -> String {
    PLUGIN_VERSION
        .lock()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_else(|| DEFAULT_VERSION.to_owned())
}

fn set_plugin_version(v: String) {
    if let Ok(mut g) = PLUGIN_VERSION.lock() {
        *g = Some(v);
    }
}

struct PassThroughV1;

impl exports::pipeline::node::lifecycle::Guest for PassThroughV1 {
    fn validate(_config: NodeConfig) -> Option<String> {
        None
    }

    fn init(config: NodeConfig) -> Result<(), ProcessError> {
        let version = if config.plugin_version.is_empty() {
            DEFAULT_VERSION.to_owned()
        } else {
            config.plugin_version
        };
        set_plugin_version(version);
        Ok(())
    }

    fn close() {}
}

impl exports::pipeline::node::transform::Guest for PassThroughV1 {
    fn process(input: Message) -> Result<OutputMessage, ProcessError> {
        let payload = input.payload.read_all();
        let mut metadata = input.metadata;
        // Add plugin.version — dedupe on key to survive re-processing paths.
        metadata.retain(|(k, _)| k != "plugin.version");
        metadata.push(("plugin.version".to_owned(), plugin_version()));

        Ok(OutputMessage {
            id: input.id,
            timestamp: input.timestamp,
            source: input.source,
            content_type: input.content_type,
            metadata,
            payload,
        })
    }
}

export!(PassThroughV1);
