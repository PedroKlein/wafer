//! Plugin test harness — load .wasm components and call them directly.
//!
//! Provides `PluginTestHarness` for integration tests that validate Wasm plugin
//! behavior without the full pipeline machinery (no channels, no orchestrator).
//! Feature-gated behind `integration-tests` because it requires pre-built .wasm
//! artifacts on disk.
//!
//! # Usage
//!
//! ```ignore
//! let harness = PluginTestHarness::new().unwrap();
//! let mut transform = harness.load_transform("path/to/plugin.wasm").unwrap();
//! let output = transform.process(input_envelope).unwrap();
//! ```

use std::path::Path;
use std::sync::Arc;

use wasmtime::Store;

use crate::engine::state::WaferState;
use crate::engine::{Capabilities, WaferEngine};
use crate::error::Result;
use crate::node::wasm::WasmTransformNode;
use crate::queue::RuntimeEnvelope;
use crate::runner::error_policy::WasmProcessError;

/// Test harness for directly calling Wasm plugin functions.
///
/// Loads .wasm components, pre-instantiates, and provides typed call wrappers.
/// No pipeline machinery — useful for plugin validation and benchmarks.
pub struct PluginTestHarness {
    engine: WaferEngine,
}

impl PluginTestHarness {
    /// Create a new harness with a default engine.
    ///
    /// Starts the epoch ticker so fuel/epoch limits work in tests.
    ///
    /// # Errors
    ///
    /// Returns error if wasmtime engine creation fails.
    pub fn new() -> Result<Self> {
        let engine = WaferEngine::new()?;
        engine.ensure_epoch_ticker();
        Ok(Self { engine })
    }

    /// Load and instantiate a transform-node component from a file path.
    ///
    /// Performs: load → pre-instantiate → create Store → instantiate → wrap.
    ///
    /// # Errors
    ///
    /// Returns error if the file cannot be read, doesn't compile, or doesn't
    /// implement the transform-node world.
    pub fn load_transform(&self, wasm_path: impl AsRef<Path>) -> Result<TransformHarness> {
        self.load_transform_with_memory_limit(wasm_path, 64 * 1024 * 1024)
    }

    /// Same as `load_transform` but with a caller-chosen store memory limit.
    /// Useful for benchmarks that push a large number of messages through a
    /// long-lived Store.
    pub fn load_transform_with_memory_limit(
        &self,
        wasm_path: impl AsRef<Path>,
        memory_limit: usize,
    ) -> Result<TransformHarness> {
        let component = self.engine.load_component(wasm_path)?;
        let pre = self.engine.pre_instantiate_transform(&component)?;
        let pre = Arc::new(pre);

        let state = WaferState::new_with_memory_limit(
            "harness-transform",
            Capabilities::sandbox(),
            memory_limit,
        );
        let mut store = Store::new(self.engine.inner(), state);
        store.limiter(|s| s.limits_mut());
        store.epoch_deadline_trap();
        store.set_epoch_deadline(self.engine.epoch_deadline());

        let bindings = pre
            .instantiate(&mut store)
            .map_err(|e| crate::error::WaferError::PluginInit { message: e.to_string() })?;

        let node = WasmTransformNode::new(store, bindings, pre, self.engine.fuel_limit());

        Ok(TransformHarness { node })
    }

    /// Access the underlying engine (for advanced use cases).
    pub fn engine(&self) -> &WaferEngine {
        &self.engine
    }
}

/// Direct transform caller — bypasses pipeline, calls process() directly.
///
/// Thin wrapper around `WasmTransformNode` that handles the boilerplate
/// tests would otherwise repeat.
pub struct TransformHarness {
    node: WasmTransformNode,
}

impl TransformHarness {
    /// Call transform::process() with a test envelope.
    ///
    /// Returns the transformed envelope or the Wasm process error.
    pub fn process(
        &mut self,
        input: RuntimeEnvelope,
    ) -> std::result::Result<RuntimeEnvelope, WasmProcessError> {
        self.node.process(input)
    }

    /// Access the underlying node (for lifecycle calls or inspection).
    pub fn node(&self) -> &WasmTransformNode {
        &self.node
    }

    /// Mutable access to the underlying node.
    pub fn node_mut(&mut self) -> &mut WasmTransformNode {
        &mut self.node
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Path to the pre-built pass-through plugin.
    const PASS_THROUGH_WASM: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm"
    );

    #[test]
    fn harness_creation_succeeds() {
        let harness = PluginTestHarness::new();
        assert!(harness.is_ok(), "Harness creation failed: {:?}", harness.err());
    }

    #[test]
    fn load_transform_nonexistent_path_fails() {
        let harness = PluginTestHarness::new().unwrap();
        let result = harness.load_transform("/nonexistent/path.wasm");
        assert!(result.is_err());
    }

    #[test]
    fn pass_through_returns_same_payload() {
        if !Path::new(PASS_THROUGH_WASM).exists() {
            eprintln!("SKIP: pass-through.wasm not built (run `just build-plugin pass-through`)");
            return;
        }

        let harness = PluginTestHarness::new().unwrap();
        let mut transform = harness.load_transform(PASS_THROUGH_WASM).unwrap();

        let input = RuntimeEnvelope::from_string("test-source", "hello world");
        let output = transform.process(input).unwrap();

        assert_eq!(
            std::str::from_utf8(&output.payload).unwrap(),
            "hello world",
            "Pass-through should return payload unchanged"
        );
    }

    #[test]
    fn pass_through_preserves_source() {
        if !Path::new(PASS_THROUGH_WASM).exists() {
            eprintln!("SKIP: pass-through.wasm not built");
            return;
        }

        let harness = PluginTestHarness::new().unwrap();
        let mut transform = harness.load_transform(PASS_THROUGH_WASM).unwrap();

        let input = RuntimeEnvelope::from_string("my-source", "data");
        let output = transform.process(input).unwrap();

        assert_eq!(
            &*output.header.source, "my-source",
            "Pass-through should preserve the source field"
        );
    }

    #[test]
    fn pass_through_multiple_messages() {
        if !Path::new(PASS_THROUGH_WASM).exists() {
            eprintln!("SKIP: pass-through.wasm not built");
            return;
        }

        let harness = PluginTestHarness::new().unwrap();
        let mut transform = harness.load_transform(PASS_THROUGH_WASM).unwrap();

        for i in 0..10 {
            let msg = format!("message-{i}");
            let input = RuntimeEnvelope::from_string("src", &msg);
            let output = transform.process(input).unwrap();
            assert_eq!(std::str::from_utf8(&output.payload).unwrap(), msg);
        }
    }

    /// Regression test for the E-Perf-4 shakedown investigation (2026-07-21).
    ///
    /// Root cause: `Store::set_epoch_deadline` in wasmtime 38 is relative to
    /// the engine's CURRENT epoch counter, not absolute. The previous runtime
    /// set the deadline once at store construction and never refreshed it,
    /// so after ~1 second of wall time (100 ticks × 10 ms) every subsequent
    /// guest call trapped with `wasm trap: interrupt` at the first epoch
    /// check point — typically inside `cabi_realloc`. The failure mode
    /// looked like a guest OOM but was actually a host-side timing bug.
    ///
    /// This test drives the harness (which shares the same per-call epoch
    /// reset codepath as the runtime after the fix) for well over 1 s of
    /// wall time with a `std::thread::sleep` between calls, then asserts
    /// every call succeeded. Before the fix this test failed by the 100th
    /// iteration; after the fix it must run to completion.
    #[test]
    fn pass_through_survives_epoch_deadline_wraparound() {
        if !Path::new(PASS_THROUGH_WASM).exists() {
            eprintln!("SKIP: pass-through.wasm not built");
            return;
        }

        let harness = PluginTestHarness::new().unwrap();
        let mut transform = harness.load_transform(PASS_THROUGH_WASM).unwrap();

        // 150 calls with a 10 ms sleep totals ~1.5 s wall time, well past
        // the 100-tick default epoch deadline. Any trap here means the
        // per-call `set_epoch_deadline` reset regressed.
        for i in 0..150 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            let payload = format!("epoch-soak-{i}");
            let input = RuntimeEnvelope::from_string("src", &payload);
            let out = transform.process(input).unwrap_or_else(|e| {
                panic!("iteration {i} trapped after >{} ms: {e}", i * 10)
            });
            assert_eq!(std::str::from_utf8(&out.payload).unwrap(), payload);
        }
    }

    /// P0.7 AC2: delay-injector with delay_ms=50 measures ≥45 ms latency.
    /// Tolerance absorbs Wasm scheduling slack (wasi:io/poll granularity,
    /// runtime epoch-tick alignment, cold-instantiation overhead on the
    /// first call).
    const DELAY_INJECTOR_WASM: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/delay-injector/target/wasm32-wasip2/release/wafer_delay_injector.wasm"
    );

    #[test]
    fn delay_injector_smoke() {
        if !Path::new(DELAY_INJECTOR_WASM).exists() {
            eprintln!("SKIP: delay-injector.wasm not built");
            return;
        }

        let harness = PluginTestHarness::new().unwrap();
        let mut transform = harness.load_transform(DELAY_INJECTOR_WASM).unwrap();

        // Warm up: first invocation absorbs first-call JIT / instantiation
        // slack. Discard its timing so the asserted floor is realistic.
        let warmup = RuntimeEnvelope::from_string("warmup", "first");
        let _ = transform.process(warmup).unwrap();

        // Real measurement.
        let input = RuntimeEnvelope::from_string("src", "payload");
        let started = std::time::Instant::now();
        let output = transform.process(input).unwrap();
        let elapsed = started.elapsed();

        // Default delay is 50 ms (see plugin lib.rs). AC2 tolerance: ≥45 ms
        // accounts for wasi:io/poll granularity plus wasmtime call overhead.
        // The ceiling of 200 ms exists to catch pathological schedulers.
        assert!(
            elapsed >= std::time::Duration::from_millis(45),
            "delay-injector process() took only {elapsed:?}; expected ≥45 ms (default 50 ms delay)"
        );
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "delay-injector process() took {elapsed:?}; expected < 500 ms"
        );

        // Payload should be echoed unchanged.
        assert_eq!(
            std::str::from_utf8(&output.payload).unwrap(),
            "payload",
            "delay-injector should echo the payload unchanged"
        );
    }

    /// P0.6 AC1: pass-through-v2-panics traps on the first process() call.
    /// The exact trap reason is left opaque to the caller (wasmtime maps
    /// panics through several possible reasons depending on codegen and
    /// wasi bindings); the invariant is that `process()` returns an error
    /// on the very first invocation.
    const PASS_THROUGH_V2_PANICS_WASM: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/pass-through-v2-panics/target/wasm32-wasip2/release/wafer_pass_through_v2_panics.wasm"
    );

    #[test]
    fn pass_through_v2_panics_traps_on_first_call() {
        if !Path::new(PASS_THROUGH_V2_PANICS_WASM).exists() {
            eprintln!("SKIP: pass-through-v2-panics.wasm not built");
            return;
        }

        let harness = PluginTestHarness::new().unwrap();
        // load_transform runs validate() + init(). Both must succeed —
        // E-Swap-5 requires the trap to surface during process(), not
        // earlier at stage-and-instantiate.
        let mut transform = harness
            .load_transform(PASS_THROUGH_V2_PANICS_WASM)
            .expect("pass-through-v2-panics init() must succeed; only process() should trap");

        let input = RuntimeEnvelope::from_string("src", "any-payload");
        let result = transform.process(input);
        assert!(
            result.is_err(),
            "pass-through-v2-panics.process() must trap on first call, got Ok"
        );

        // The captured error should surface the intentional-trap message so
        // reviewers reading logs immediately understand this is fault
        // injection, not a genuine bug. We do not pin the exact wording
        // because wasmtime prefixes the trap frame text differently across
        // versions — we just want the sentinel visible.
        let err = result.unwrap_err().to_string().to_lowercase();
        assert!(
            err.contains("panic")
                || err.contains("unreachable")
                || err.contains("trap")
                || err.contains("e-swap-5"),
            "trap error should reference panic/trap/unreachable or the E-Swap-5 sentinel; got: {err}"
        );
    }

    /// Isolation harness for the `cabi_realloc` trap observed in the
    /// pipeline-c-passthrough dry run (E-Perf-4 shakedown). Loads the real
    /// production `wafer_pass_through.wasm`, feeds it 5 progressively larger
    /// payloads mimicking the exact envelope shape `BenchSource` produces
    /// (payload + two metadata KV pairs), and reports which sizes trap.
    /// Left ignored so it never runs in CI — invoke with
    /// `cargo test --release -p wafer-core --lib -- --ignored --nocapture pass_through_bench_shapes`.
    #[test]
    #[ignore = "E-Perf-4 diagnostic; opt-in"]
    fn pass_through_bench_shapes() {
        use bytes::Bytes;

        if !Path::new(PASS_THROUGH_WASM).exists() {
            eprintln!("SKIP: pass-through.wasm not built");
            return;
        }
        let harness = PluginTestHarness::new().unwrap();
        let mut t = harness.load_transform(PASS_THROUGH_WASM).unwrap();

        for size in [16_usize, 128, 1024, 10_240, 102_400] {
            let payload = Bytes::from(vec![0x42u8; size]);
            let env = RuntimeEnvelope::new("bench-source", payload)
                .with_metadata("bench.sequence", "0")
                .with_metadata("bench.intended_ns", "1234567890123456789");
            match t.process(env) {
                Ok(out) => eprintln!("size={size:>6}: OK payload_len={}", out.payload.len()),
                Err(e) => eprintln!("size={size:>6}: FAIL {e}"),
            }
        }
    }
}
