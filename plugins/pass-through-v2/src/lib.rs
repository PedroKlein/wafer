//! Pass-through v2 — hot-swap experiment version marker.
//!
//! Reads `plugin_version` from `NodeConfig` (default "2.0.0") and stamps two
//! metadata keys on every output:
//!   - `plugin.version` (typically "2.0.0")
//!   - `swapped = "true"` (extra sentinel so downstream assertions can spot
//!     v2 output without parsing version strings)
//!
//! State handling: v2 does NOT preserve state from v1 (per thesis-statement-v3
//! scope qualifier #3). Fresh init, fresh instance.

wit_bindgen::generate!({
    path: "../../wit/node",
    world: "transform-node",
    generate_all,
});

use std::sync::Mutex;

use exports::pipeline::node::lifecycle::NodeConfig;
use exports::pipeline::node::transform::{Message, OutputMessage, ProcessError};

const DEFAULT_VERSION: &str = "2.0.0";

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

struct PassThroughV2;

impl exports::pipeline::node::lifecycle::Guest for PassThroughV2 {
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

impl exports::pipeline::node::transform::Guest for PassThroughV2 {
    fn process(input: Message) -> Result<OutputMessage, ProcessError> {
        let payload = input.payload.read_all();
        let mut metadata = input.metadata;
        metadata.retain(|(k, _)| k != "plugin.version" && k != "swapped");
        metadata.push(("plugin.version".to_owned(), plugin_version()));
        metadata.push(("swapped".to_owned(), "true".to_owned()));

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

export!(PassThroughV2);
