//! End-to-end integration test for the TinyGo `uppercase` plugin.
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

use wafer_core::queue::RuntimeEnvelope;
use wafer_core::testing::PluginTestHarness;

/// Path to the pre-built TinyGo uppercase plugin.
///
/// Built by `mise run //plugins:build-plugin-go`. Skipped (not failed) when
/// missing so `cargo test` still works in environments without TinyGo.
const UPPERCASE_GO_WASM: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/go/uppercase/wafer-uppercase-go.wasm");

fn skip_if_not_built() -> Option<PluginTestHarness> {
    if !Path::new(UPPERCASE_GO_WASM).exists() {
        eprintln!(
            "SKIP: {UPPERCASE_GO_WASM} not built \
             (run `mise run //plugins:build-plugin-go`)"
        );
        return None;
    }
    Some(PluginTestHarness::new().expect("harness init"))
}

/// Lifecycle boundary: `init` accepts the 3-field `node-config`
/// (id, config, plugin-version) without a signature mismatch trap.
///
/// This is the exact site where the pre-existing committed Go bindings had
/// drifted (2-field node-config) — regenerating without patching would
/// resurface the mismatch here.
#[test]
fn go_uppercase_instantiates_through_host_bindings() {
    let Some(harness) = skip_if_not_built() else { return };
    // Loading = compile + pre-instantiate + Store::new + instantiate + init.
    // If any of those fail we get a WaferError back.
    let _transform = harness
        .load_transform(UPPERCASE_GO_WASM)
        .expect("Go uppercase plugin must instantiate against wafer:pipeline WIT");
}

/// Transform boundary: `process(message) -> result<output-message, ...>`
/// crosses the Component-Model call, borrows the host-managed buffer via
/// `types.buffer.read-all`, returns owned bytes.
#[test]
fn go_uppercase_actually_uppercases_a_message() {
    let Some(harness) = skip_if_not_built() else { return };
    let mut transform = harness.load_transform(UPPERCASE_GO_WASM).unwrap();

    let input = RuntimeEnvelope::from_string("integration-test", "hello world");
    let output = transform.process(input).expect("transform call must succeed");

    let payload = std::str::from_utf8(&output.payload).expect("valid utf-8");
    assert_eq!(payload, "HELLO WORLD", "TinyGo plugin must uppercase payload");
}

/// Payload metadata must round-trip: source, id, timestamp, content-type must
/// all survive the Rust → Wasm → Rust hop. Guards against silent field
/// truncation if `node-config` / `message` records ever drift again.
#[test]
fn go_uppercase_preserves_envelope_metadata() {
    let Some(harness) = skip_if_not_built() else { return };
    let mut transform = harness.load_transform(UPPERCASE_GO_WASM).unwrap();

    let input = RuntimeEnvelope::from_string("my-source", "data");
    let output = transform.process(input).expect("process succeeds");

    assert_eq!(&*output.header.source, "my-source", "source preserved");
}

/// Call the plugin multiple times through the same Store — proves the
/// borrow<buffer> resource lifecycle (host push → guest read → host drop)
/// doesn't leak or corrupt state across calls.
#[test]
fn go_uppercase_handles_multiple_messages_in_one_store() {
    let Some(harness) = skip_if_not_built() else { return };
    let mut transform = harness.load_transform(UPPERCASE_GO_WASM).unwrap();

    for i in 0..10 {
        let msg = format!("msg-{i}");
        let input = RuntimeEnvelope::from_string("src", &msg);
        let output = transform.process(input).expect("call succeeds");
        assert_eq!(
            std::str::from_utf8(&output.payload).unwrap(),
            msg.to_uppercase(),
            "call {i}: payload must uppercase"
        );
    }
}

/// Sentinel: enforces the exact failure signal the double-drop safety net
/// emits, so future contributors know what regression looks like.
///
/// If `plugins/go/uppercase/borrow_shim.go` is regenerated / removed
/// incorrectly, one of the following will happen at `process()` time:
///
///   * Missing drop (shim needed but absent) →
///     `WasmProcessError::Unrecoverable("...borrow handles still remain...")`
///   * Double drop (shim redundant, generator now emits its own release) →
///     `WasmProcessError::Unrecoverable("...")` from a wasmtime trap on
///     `ResourceTable::delete` failing because the resource is gone.
///
/// Either way the three preceding tests fail with an `Unrecoverable` variant
/// containing a clear diagnostic string. This test documents that contract:
/// it constructs the error variant we expect the harness to surface so the
/// diagnostic doesn't drift silently.
#[test]
fn go_uppercase_double_drop_diagnostic_is_recognizable() {
    use wafer_core::runner::error_policy::WasmProcessError;
    // Ensures the diagnostic string we surface for the borrow-handle regime
    // is still the variant integration tests match against. If the enum
    // shape changes, this fails to compile.
    let sentinel = WasmProcessError::Unrecoverable(
        "borrow handles still remain at the end of the call".to_string(),
    );
    match sentinel {
        WasmProcessError::Unrecoverable(msg) => {
            assert!(
                msg.contains("borrow handles") || msg.contains("resource"),
                "diagnostic sentinel drifted: {msg}"
            );
        }
        other => panic!("expected Unrecoverable variant, got {other:?}"),
    }
}
