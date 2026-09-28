//! Plugin test harness — load .wasm components and call them directly.
//!
//! Provides `PluginTestHarness` for integration tests that validate Wasm plugin
//! behavior without the full pipeline machinery (no channels, no orchestrator).
//! Needs pre-built .wasm artifacts on disk; see [`super::artifact_available`].
//!
//! # Usage
//!
//! ```ignore
//! let harness = PluginTestHarness::new().unwrap();
//! let mut transform = harness.load_transform("path/to/plugin.wasm").await.unwrap();
//! let output = transform.process(input_envelope).await.unwrap();
//! ```

use std::num::NonZeroU64;
use std::path::Path;
use std::sync::Arc;

use wasmtime::Store;

use crate::config::EngineConfig;
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

/// Default epoch deadline for the test harness: 100 ticks × 10 ms = ~1 s.
/// Generous enough for any well-behaved plugin, tight enough to catch infinite
/// loops within a second rather than hanging the test binary.
const HARNESS_DEFAULT_EPOCH_DEADLINE: u64 = 100;

impl PluginTestHarness {
    /// Create a new harness with a sandbox-safe default engine.
    ///
    /// Enables epoch interruption (100 ticks ≈ 1 s timeout per call) so that
    /// misbehaving plugins (infinite loops) are contained. Starts the OS-thread
    /// epoch ticker.
    ///
    /// # Errors
    ///
    /// Returns error if wasmtime engine creation fails.
    pub fn new() -> Result<Self> {
        let config = EngineConfig {
            epoch_deadline: NonZeroU64::new(HARNESS_DEFAULT_EPOCH_DEADLINE),
            ..EngineConfig::default()
        };
        Self::with_engine_config(&config)
    }

    /// Create a harness with a caller-supplied engine configuration.
    ///
    /// Use this when benchmarks need unlimited epoch/fuel or when a test needs
    /// specific metering settings.
    ///
    /// # Errors
    ///
    /// Returns error if wasmtime engine creation fails.
    pub fn with_engine_config(config: &EngineConfig) -> Result<Self> {
        let engine = WaferEngine::from_engine_config(config)?;
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
    pub async fn load_transform(&self, wasm_path: impl AsRef<Path>) -> Result<TransformHarness> {
        self.load_transform_with_memory_limit(wasm_path, 64 * 1024 * 1024).await
    }

    /// Same as `load_transform` but with a caller-chosen store memory limit.
    /// Useful for benchmarks that push a large number of messages through a
    /// long-lived Store.
    pub async fn load_transform_with_memory_limit(
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
        // AC F5.AC2: skip metering setters when unlimited; consume_fuel /
        // epoch_interruption are gated at Config level so set_fuel would
        // return Err when fuel_limit is None. Fuel + epoch must be applied
        // before instantiation — start functions consume fuel and the epoch
        // ticker is running.
        let fuel_limit = self.engine.fuel_budget(crate::node::NodeKind::Transform, None);
        if let Some(n) = fuel_limit {
            store.set_fuel(n.get()).map_err(|e| crate::error::WaferError::PluginInit {
                message: format!("failed to set fuel: {e}"),
            })?;
        }
        if let Some(n) = self.engine.epoch_deadline() {
            store.epoch_deadline_trap();
            store.set_epoch_deadline(n.get());
        }

        let bindings = pre
            .instantiate_async(&mut store)
            .await
            .map_err(|e| crate::error::WaferError::PluginInit { message: e.to_string() })?;

        let mut node = WasmTransformNode::new(store, bindings, pre, fuel_limit);
        // Propagate the engine's epoch deadline into the node so it resets the
        // deadline on every process() call. Without this, the per-call
        // `set_epoch_deadline` guard inside `WasmTransformNode::process` is a
        // no-op (epoch_deadline defaults to None in the constructor).
        node.configure_runtime(
            Capabilities::sandbox(),
            memory_limit,
            self.engine.epoch_deadline(),
            "{}".to_string(),
        );

        Ok(TransformHarness { node })
    }

    /// Access the underlying engine (for advanced use cases).
    pub const fn engine(&self) -> &WaferEngine {
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
    pub async fn process(
        &mut self,
        input: RuntimeEnvelope,
    ) -> std::result::Result<RuntimeEnvelope, WasmProcessError> {
        self.node.process(input).await
    }

    /// Access the underlying node (for lifecycle calls or inspection).
    pub const fn node(&self) -> &WasmTransformNode {
        &self.node
    }

    /// Mutable access to the underlying node.
    pub const fn node_mut(&mut self) -> &mut WasmTransformNode {
        &mut self.node
    }
}

#[cfg(test)]
#[expect(
    clippy::print_stderr,
    reason = "test diagnostic output for skipped tests when wasm plugins are not built"
)]
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

    #[tokio::test]
    async fn load_transform_nonexistent_path_fails() {
        let harness = PluginTestHarness::new().unwrap();
        let result = harness.load_transform("/nonexistent/path.wasm").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn pass_through_returns_same_payload() {
        if !crate::testing::artifact_available(PASS_THROUGH_WASM) {
            return;
        }

        let harness = PluginTestHarness::new().unwrap();
        let mut transform = harness.load_transform(PASS_THROUGH_WASM).await.unwrap();

        let input = RuntimeEnvelope::from_string("test-source", "hello world");
        let output = transform.process(input).await.unwrap();

        assert_eq!(
            std::str::from_utf8(&output.payload).unwrap(),
            "hello world",
            "Pass-through should return payload unchanged"
        );
    }

    #[tokio::test]
    async fn pass_through_preserves_source() {
        if !crate::testing::artifact_available(PASS_THROUGH_WASM) {
            return;
        }

        let harness = PluginTestHarness::new().unwrap();
        let mut transform = harness.load_transform(PASS_THROUGH_WASM).await.unwrap();

        let input = RuntimeEnvelope::from_string("my-source", "data");
        let output = transform.process(input).await.unwrap();

        assert_eq!(
            &*output.header.source, "my-source",
            "Pass-through should preserve the source field"
        );
    }

    #[tokio::test]
    async fn pass_through_sustains_repeated_guest_calls() {
        if !crate::testing::artifact_available(PASS_THROUGH_WASM) {
            return;
        }

        let harness = PluginTestHarness::new().unwrap();
        let mut transform = harness.load_transform(PASS_THROUGH_WASM).await.unwrap();

        for _ in 0..10_000 {
            let input = RuntimeEnvelope::from_string("src", "message");
            let output = transform.process(input).await.unwrap();
            assert_eq!(&*output.payload, b"message");
        }
    }

    /// Regression: guest metadata must reach the runtime envelope.
    #[tokio::test]
    async fn pass_through_propagates_metadata() {
        if !crate::testing::artifact_available(PASS_THROUGH_WASM) {
            return;
        }

        let harness = PluginTestHarness::new().unwrap();
        let mut transform = harness.load_transform(PASS_THROUGH_WASM).await.unwrap();

        let input = RuntimeEnvelope::from_string("src", "hello")
            .with_metadata("sensor", "a")
            .with_metadata("unit", "celsius");
        let out = transform.process(input).await.expect("transform must succeed");

        let md: Vec<(&str, &str)> =
            out.header.metadata.iter().map(|(k, v)| (k.as_ref(), v.as_ref())).collect();
        assert_eq!(md, [("sensor", "a"), ("unit", "celsius")]);
    }

    /// Bench stamps stay on the host: the guest gets no metadata for them,
    /// and the output carries the input's stamps unchanged.
    #[tokio::test]
    async fn bench_stamps_bypass_the_guest_and_survive_the_transform() {
        use crate::queue::BenchStamps;

        if !crate::testing::artifact_available(PASS_THROUGH_WASM) {
            return;
        }

        let harness = PluginTestHarness::new().unwrap();
        let mut transform = harness.load_transform(PASS_THROUGH_WASM).await.unwrap();

        let stamps = BenchStamps {
            sequence: 42,
            intended_ns: 1_234_567_890_123,
            emit_ns: 1_234_567_890_456,
            warmup: false,
            measurement_start_seq: 10,
            sequence_end: None,
            burst: None,
        };
        let input = RuntimeEnvelope::from_string("src", "hello").with_bench_stamps(stamps);
        let out = transform.process(input).await.expect("transform must succeed");

        assert!(
            out.header.metadata.is_empty(),
            "guest saw harness data: {:?}",
            out.header.metadata
        );
        assert_eq!(out.header.bench, Some(stamps));
    }

    /// Regression: `Store::set_epoch_deadline` is relative to the engine's
    /// current epoch, so it must be reset per call. 150 iterations × 10 ms
    /// sleep spans ~1.5 s wall time, well past the 100-tick default; a
    /// missing reset traps at the first check point (typically
    /// `cabi_realloc`) around iteration 100.
    #[tokio::test]
    async fn pass_through_survives_epoch_deadline_wraparound() {
        if !crate::testing::artifact_available(PASS_THROUGH_WASM) {
            return;
        }

        let harness = PluginTestHarness::new().unwrap();
        let mut transform = harness.load_transform(PASS_THROUGH_WASM).await.unwrap();

        for i in 0..150 {
            std::thread::sleep(std::time::Duration::from_millis(10));
            let payload = format!("epoch-soak-{i}");
            let input = RuntimeEnvelope::from_string("src", &payload);
            let out = transform
                .process(input)
                .await
                .unwrap_or_else(|e| panic!("iteration {i} trapped after >{} ms: {e}", i * 10));
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

    #[tokio::test]
    async fn delay_injector_smoke() {
        if !crate::testing::artifact_available(DELAY_INJECTOR_WASM) {
            return;
        }

        let harness = PluginTestHarness::new().unwrap();
        let mut transform = harness.load_transform(DELAY_INJECTOR_WASM).await.unwrap();

        // Warm up: first invocation absorbs first-call JIT / instantiation
        // slack. Discard its timing so the asserted floor is realistic.
        let warmup = RuntimeEnvelope::from_string("warmup", "first");
        let _ = transform.process(warmup).await.unwrap();

        // Real measurement.
        let input = RuntimeEnvelope::from_string("src", "payload");
        let started = std::time::Instant::now();
        let output = transform.process(input).await.unwrap();
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

    #[tokio::test]
    async fn pass_through_v2_panics_traps_on_first_call() {
        if !crate::testing::artifact_available(PASS_THROUGH_V2_PANICS_WASM) {
            return;
        }

        let harness = PluginTestHarness::new().unwrap();
        // load_transform runs validate() + init(). Both must succeed —
        // E-Swap-5 requires the trap to surface during process(), not
        // earlier at stage-and-instantiate.
        let mut transform = harness
            .load_transform(PASS_THROUGH_V2_PANICS_WASM)
            .await
            .expect("pass-through-v2-panics init() must succeed; only process() should trap");

        let input = RuntimeEnvelope::from_string("src", "any-payload");
        let result = transform.process(input).await;
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

    /// E-Perf-4 diagnostic: soak the real pass-through plugin at five
    /// payload sizes and report which — if any — trap. Ignored so it
    /// never runs in CI. Invoke with
    /// `cargo test --release -p wafer-core --lib -- --ignored --nocapture pass_through_bench_shapes`.
    #[tokio::test]
    #[ignore = "E-Perf-4 diagnostic; opt-in"]
    async fn pass_through_bench_shapes() {
        use bytes::Bytes;

        if !crate::testing::artifact_available(PASS_THROUGH_WASM) {
            return;
        }
        let harness = PluginTestHarness::new().unwrap();
        let mut t = harness.load_transform(PASS_THROUGH_WASM).await.unwrap();

        for size in [16_usize, 128, 1024, 10_240, 102_400] {
            let payload = Bytes::from(vec![0x42u8; size]);
            let env = RuntimeEnvelope::new("bench-source", payload);
            match t.process(env).await {
                Ok(out) => eprintln!("size={size:>6}: OK payload_len={}", out.payload.len()),
                Err(e) => eprintln!("size={size:>6}: FAIL {e}"),
            }
        }
    }
}
