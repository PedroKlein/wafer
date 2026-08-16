//! Delay-injector transform plugin — sleeps `delay_ms` before echoing input.
//!
//! Purpose: RFC-008 E-Val-1 methodology validation. Injecting a known 50 ms
//! delay lets the eval scripts verify that the measurement pipeline actually
//! captures the delay in p99 (rather than reporting an artificially low
//! percentile because coordinated omission is being ignored). If the observed
//! p99 doesn't reflect the injected delay, the measurement infrastructure is
//! lying and every RQ1 / RQ2 / RQ3 number afterwards is suspect.
//!
//! # Config
//! ```json
//! { "delay_ms": 50 }
//! ```
//! Missing / malformed config defaults to 50 ms.
//!
//! # Sleep implementation
//! Uses `std::thread::sleep`, which on `wasm32-wasip2` maps to
//! `wasi:clocks/monotonic-clock.subscribe-duration` + `wasi:io/poll.poll` in
//! the component adapter — a proper cooperative wait rather than a spin loop.
//! This means the injected delay does not consume CPU and does not trip the
//! runtime's fuel budget.
//!
//! # Warning
//! Long delays interact with the runtime's epoch deadline. The default
//! `epoch_deadline` in `EngineConfig` is 200 × 20 ms = 4 s; delays approaching
//! that ceiling will trap. E-Val-1's 50 ms injection is safely under.

wit_bindgen::generate!({
    path: "../../wit",
    world: "transform-node",
    generate_all,
});

use exports::wafer::pipeline::lifecycle::NodeConfig;
use exports::wafer::pipeline::transform::{Message, OutputMessage, ProcessError};

/// Delay applied in `process()`, in milliseconds. Set by `init()` from the
/// TOML `[nodes.<id>.config]` block; defaults to 50 ms if config is empty or
/// missing `delay_ms`.
///
/// # Safety
/// Wasm modules are single-threaded, so this static mut is only ever accessed
/// from one thread at a time. No lock needed.
static mut DELAY_MS: u64 = 50;

/// Read the current delay. Sole entry point that unsafe-touches DELAY_MS.
fn delay_ms() -> u64 {
    // SAFETY: single-threaded Wasm; no other thread can access DELAY_MS.
    unsafe { DELAY_MS }
}

/// Write the delay. Called only from `init()`, which is invoked once per
/// plugin instance at pipeline startup.
///
/// # Safety
/// Wasm is single-threaded; init() is not concurrent with process().
fn set_delay_ms(value: u64) {
    // SAFETY: single-threaded Wasm; init() is called before process().
    unsafe {
        DELAY_MS = value;
    }
}

/// Minimal JSON parser for `{"delay_ms": <u64>}`. Avoids pulling in serde to
/// keep the plugin binary small (goal: sub-100 KB per E-Density-1 tier).
fn extract_delay_ms(json: &str) -> Option<u64> {
    let key = "\"delay_ms\"";
    let idx = json.find(key)?;
    let after_key = &json[idx + key.len()..];
    let after_colon = after_key.trim_start().strip_prefix(':')?;
    let value_start = after_colon.trim_start();
    let end = value_start
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value_start.len());
    if end == 0 {
        return None;
    }
    value_start[..end].parse().ok()
}

struct DelayInjector;

impl exports::wafer::pipeline::lifecycle::Guest for DelayInjector {
    fn validate(config: NodeConfig) -> Option<String> {
        if config.config.is_empty() {
            return None;
        }
        // Empty JSON object is fine; anything else must have delay_ms.
        if config.config == "{}" || extract_delay_ms(&config.config).is_some() {
            return None;
        }
        Some(format!(
            "delay-injector: missing or invalid `delay_ms` in config: {}",
            config.config
        ))
    }

    fn init(config: NodeConfig) -> Result<(), ProcessError> {
        if let Some(v) = extract_delay_ms(&config.config) {
            set_delay_ms(v);
        }
        Ok(())
    }

    fn close() {}
}

impl exports::wafer::pipeline::transform::Guest for DelayInjector {
    fn process(input: Message) -> Result<OutputMessage, ProcessError> {
        // Read payload BEFORE sleeping so any host-side buffer lifecycle stays
        // synchronous with the message boundary. Otherwise a very long delay
        // could interact with buffer recycling in surprising ways.
        let payload = input.payload.read_all();

        std::thread::sleep(std::time::Duration::from_millis(delay_ms()));

        Ok(OutputMessage {
            id: input.id,
            timestamp: input.timestamp,
            source: input.source,
            content_type: input.content_type,
            metadata: input.metadata,
            payload,
        })
    }
}

export!(DelayInjector);
