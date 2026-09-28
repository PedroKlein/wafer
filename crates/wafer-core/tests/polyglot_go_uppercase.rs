//! End-to-end integration test for the `TinyGo` `uppercase` plugin.
//!
//! Verifies the polyglot Wasm boundary: a TinyGo-compiled component matches
//! the canonical `wafer:pipeline@0.1.0` WIT contract that the Rust host
//! generated its bindings from, and can be instantiated + called through the
//! real `WaferEngine` transform loop.
//!
//! This is the only test that exercises the Go plugin's `gen/` checked-in
//! bindings end-to-end — protects against silent regressions when the WIT
//! shape changes but Go regeneration is skipped.

use std::path::Path;
use std::sync::Arc;

use wafer_core::queue::RuntimeEnvelope;
use wafer_core::testing::PluginTestHarness;

/// Path to the `TinyGo` uppercase plugin, built by
/// `mise run //plugins:build-plugin-go`. The tests are opt-in
/// (`mise run //plugins:test-plugin-go`) because CI has no `TinyGo`.
const UPPERCASE_GO_WASM: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/go/uppercase/wafer-uppercase-go.wasm");

fn harness() -> anyhow::Result<PluginTestHarness> {
    anyhow::ensure!(
        Path::new(UPPERCASE_GO_WASM).is_file(),
        "{UPPERCASE_GO_WASM} is not built (run `mise run //plugins:build-plugin-go`)"
    );
    PluginTestHarness::new().map_err(Into::into)
}

/// Lifecycle boundary: `init` accepts the 3-field `node-config`
/// (id, config, plugin-version) without a signature mismatch trap.
///
/// This is the exact site where the pre-existing committed Go bindings had
/// drifted (2-field node-config) — regenerating without patching would
/// resurface the mismatch here.
#[tokio::test]
#[ignore = "needs the TinyGo plugin: mise run //plugins:test-plugin-go"]
async fn go_uppercase_instantiates_through_host_bindings() -> anyhow::Result<()> {
    let harness = harness()?;
    let _transform = harness.load_transform(UPPERCASE_GO_WASM).await?;
    Ok(())
}

/// Transform boundary: `process(message) -> result<output-message, ...>`
/// crosses the Component-Model call, borrows the host-managed buffer via
/// `types.buffer.read-all`, returns owned bytes.
#[tokio::test]
#[ignore = "needs the TinyGo plugin: mise run //plugins:test-plugin-go"]
async fn go_uppercase_actually_uppercases_a_message() -> anyhow::Result<()> {
    let harness = harness()?;
    let mut transform = harness.load_transform(UPPERCASE_GO_WASM).await?;

    let input = RuntimeEnvelope::from_string("integration-test", "hello world");
    let output = transform.process(input).await?;

    let payload = std::str::from_utf8(&output.payload)?;
    anyhow::ensure!(payload == "HELLO WORLD", "TinyGo plugin must uppercase payload");
    Ok(())
}

/// Guest-owned output fields must survive the host lifting boundary, while
/// lineage continues from the host-owned input envelope.
#[tokio::test]
#[ignore = "needs the TinyGo plugin: mise run //plugins:test-plugin-go"]
async fn go_uppercase_preserves_guest_fields_and_host_lineage() -> anyhow::Result<()> {
    let harness = harness()?;
    let mut transform = harness.load_transform(UPPERCASE_GO_WASM).await?;

    let mut input = RuntimeEnvelope::from_string("host-source", "hello world");
    input.set_parent_id("host-parent");
    input.ensure_trace_id();
    let trace_id = input.trace_id().expect("host trace id").to_string();
    let header = Arc::make_mut(&mut input.header);
    header.id = "host-id".into();
    header.timestamp = 1_700_000_000_000_000_123;
    header.content_type = "application/host".into();
    header.metadata = vec![("host-key".into(), "host-value".into())];

    let output = transform.process(input).await?;

    anyhow::ensure!(&*output.header.id == "guest-host-id", "guest id preserved");
    anyhow::ensure!(
        output.header.timestamp == 1_700_000_000_000_000_130,
        "guest timestamp preserved"
    );
    anyhow::ensure!(&*output.header.source == "guest-host-source", "guest source preserved");
    anyhow::ensure!(
        &*output.header.content_type == "text/uppercase",
        "guest content type preserved"
    );
    anyhow::ensure!(
        output.header.metadata
            == vec![
                ("host-key".into(), "host-value".into()),
                ("plugin".into(), "go-uppercase".into()),
            ],
        "guest metadata preserved"
    );
    anyhow::ensure!(
        std::str::from_utf8(&output.payload)? == "HELLO WORLD",
        "guest payload preserved"
    );
    anyhow::ensure!(output.parent_id() == Some("host-parent"), "host parent lineage retained");
    anyhow::ensure!(output.trace_id() == Some(trace_id.as_str()), "host trace lineage retained");
    Ok(())
}

/// Call the plugin multiple times through the same Store — proves the
/// borrow<buffer> resource lifecycle (host push → guest read → host drop)
/// doesn't leak or corrupt state across calls.
#[tokio::test]
#[ignore = "needs the TinyGo plugin: mise run //plugins:test-plugin-go"]
async fn go_uppercase_handles_multiple_messages_in_one_store() -> anyhow::Result<()> {
    let harness = harness()?;
    let mut transform = harness.load_transform(UPPERCASE_GO_WASM).await?;

    for i in 0..10 {
        let msg = format!("msg-{i}");
        let input = RuntimeEnvelope::from_string("src", &msg);
        let output = transform.process(input).await?;
        anyhow::ensure!(
            std::str::from_utf8(&output.payload)? == msg.to_uppercase(),
            "call {i}: payload must uppercase"
        );
    }
    Ok(())
}
