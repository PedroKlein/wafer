//! Pass-through v2 (fault-injection variant) — traps on first `process()` call.
//!
//! Used by RFC-008 E-Swap-5 to verify the A4 rollback path: when the new
//! plugin traps on its very first invocation, the runtime must roll the
//! node back to v1's `InstancePre` and keep the pipeline running with zero
//! message loss.
//!
//! # Never run in production
//!
//! This plugin is defined in `plugins/pass-through-v2-panics/`, sits under
//! the same `plugins/` root as legitimate plugins for build parity, but must
//! only appear inside `eval/configs/*` fixtures used by the E-Swap-5 test
//! harness. The `plugin_version = "2.0.0-panic"` string in metadata is the
//! sentinel a reviewer can grep for.
//!
//! # Lifecycle semantics
//!
//! - `validate()`  → always accepts. The whole point is that the runtime
//!   must ship the bytes to the node before discovering it is broken.
//! - `init()`      → returns `Ok(())`. If init failed the runtime would
//!   refuse the swap up-front; E-Swap-5 wants the failure to surface
//!   during message processing to exercise trap-recovery.
//! - `process()`   → panics on the first call (subsequent calls are
//!   unreachable — the guest instance is discarded after the trap).

wit_bindgen::generate!({
    path: "../../wit",
    world: "transform-node",
    generate_all,
});

use exports::wafer::pipeline::lifecycle::NodeConfig;
use exports::wafer::pipeline::transform::{Message, OutputMessage, ProcessError};

/// Sentinel version. Grep this in a config review to catch accidental use.
const PANIC_VERSION: &str = "2.0.0-panic";

struct PassThroughV2Panics;

impl exports::wafer::pipeline::lifecycle::Guest for PassThroughV2Panics {
    fn validate(_config: NodeConfig) -> Option<String> {
        None
    }

    fn init(_config: NodeConfig) -> Result<(), ProcessError> {
        // Init succeeds — we want the trap to surface during process(),
        // not validation. Otherwise the runtime would reject the swap at
        // stage-and-instantiate and E-Swap-5 could not measure rollback.
        Ok(())
    }

    fn close() {}
}

impl exports::wafer::pipeline::transform::Guest for PassThroughV2Panics {
    fn process(_input: Message) -> Result<OutputMessage, ProcessError> {
        // Trap immediately. `panic!` in wasm32-wasip2 aborts the instance
        // with a trap the host observes as `wasmtime::Trap::UnreachableCodeReached`
        // (or similar, depending on codegen). Downstream RFC-008 E-Swap-5
        // treats this as "unrecoverable within-instance" and expects the
        // orchestrator to fall back to the cached v1 `InstancePre`.
        panic!(
            "pass-through-v2-panics {PANIC_VERSION}: intentional trap for E-Swap-5 rollback test"
        );
    }
}

export!(PassThroughV2Panics);
