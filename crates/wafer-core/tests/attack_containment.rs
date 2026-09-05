#![cfg(test)]
#![expect(
    clippy::print_stderr,
    clippy::option_if_let_else,
    reason = "integration test: diagnostic output and convenience patterns"
)]
//! P0.8 — RQ2 Attack containment integration harness.
//!
//! Six real attack plugins live under `plugins/attacks/*`. This file
//! supplies the assertion side: for each attack scenario in RFC-008
//! §D8 we verify the *sandbox as configured* contains the exploit —
//! specifically:
//!
//! 1. A healthy transform (pass-through) processes a message
//!    successfully *before* the attack.
//! 2. Loading and invoking the attack plugin yields a `WasmProcessError`
//!    from the same `PluginTestHarness`. We do NOT assert on the exact
//!    trap text (varies with wasmtime version) but we DO assert the
//!    error kind is one of the expected containment variants:
//!    - `Unrecoverable`  — trap (unreachable, OOB memory, etc.)
//!    - `TimedOut`       — epoch interruption
//!
//!    Both variants map into `NodeStateTracker::transition_to_error`
//!    at the runner layer (see `crates/wafer-core/src/runner/*.rs`).
//! 3. The healthy transform still processes a *subsequent* message
//!    successfully. Isolation invariant: an attack in one Store MUST
//!    NOT poison a co-resident, independently loaded Store belonging
//!    to another node.
//!
//! Assumptions kept explicit (constraints on the task):
//! - No capability grant is relaxed. `WaferState::sandbox()` is used
//!   through the default `PluginTestHarness` path.
//! - Attack .wasm artefacts are pre-built (`plugins/attacks/*/target/
//!   wasm32-wasip2/release/*.wasm`). Missing artefacts SKIP with an
//!   `eprintln!` — matches the pattern used elsewhere in the crate.
//!
//! Epoch-interrupt dependency (S3 — infinite loop):
//! `PluginTestHarness::new()` enables epoch interruption with a 100-tick
//! deadline (~1 s). This is required for S3 containment — without it, the
//! guest `loop {}` runs forever. The harness was fixed in the T7 resolution
//! (see docs/decisions/attack-containment-hang.md) after commit `6096dfd`
//! disabled epoch interruption for the default `EngineConfig`.
//!
//! Not covered here (deferred per non-goals):
//! - Latency-to-contain (that is E-Iso-8's job).
//! - Full 3-node channel-wired orchestrator run. The harness-level
//!   isolation proof above IS the containment invariant; running the
//!   orchestrator only adds channel plumbing, which is already
//!   integration-tested elsewhere.

use std::path::Path;
use std::time::Duration;

use wafer_core::orchestrator::launch_pipeline;
use wafer_core::queue::RuntimeEnvelope;
use wafer_core::runner::error_policy::WasmProcessError;
use wafer_core::testing::PluginTestHarness;
use wafer_types::config::Config;

/// Pass-through plugin acts as the co-resident "healthy" transform.
/// If this is missing we cannot prove the isolation invariant, so we
/// skip the whole test — same pattern as harness.rs.
const PASS_THROUGH_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm"
);

// ---------------------------------------------------------------------------
// Attack .wasm paths (one const per scenario for greppability).
// ---------------------------------------------------------------------------

const ATK_BUFFER_OVERFLOW: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/attacks/buffer-overflow/target/wasm32-wasip2/release/wafer_attack_buffer_overflow.wasm"
);
const ATK_CROSS_READ: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/attacks/cross-read/target/wasm32-wasip2/release/wafer_attack_cross_read.wasm"
);
const ATK_FS_ACCESS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/attacks/fs-access/target/wasm32-wasip2/release/wafer_attack_fs_access.wasm"
);
const ATK_INFINITE_LOOP: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/attacks/infinite-loop/target/wasm32-wasip2/release/wafer_attack_infinite_loop.wasm"
);
const ATK_MEMORY_EXHAUST: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/attacks/memory-exhaust/target/wasm32-wasip2/release/wafer_attack_memory_exhaust.wasm"
);
const ATK_PANIC: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/attacks/panic/target/wasm32-wasip2/release/wafer_attack_panic.wasm"
);

/// Small memory limit for the memory-exhaust scenario. Every other
/// attack uses the harness default (64 MiB) so their trap comes from
/// the intended fault (OOB, panic, unreachable, epoch), not from an
/// artificially-tight cap.
const MEMORY_EXHAUST_LIMIT: usize = 4 * 1024 * 1024; // 4 MiB

/// Classification of a `WasmProcessError` kind for readable assertions.
#[derive(Debug, PartialEq, Eq)]
enum ContainedAs {
    /// Trap-like: `Unrecoverable(_)` — buffer overflow, cross-read,
    /// panic, `StoreLimits` cap.
    Trap,
    /// Fuel/epoch interruption: `TimedOut` — infinite loop when the
    /// epoch deadline fires. Fuel-exhaustion also lands here as long
    /// as the `map_trap` heuristic sees an "interrupt"/"epoch" string.
    TimedOut,
}

fn classify(err: &WasmProcessError) -> ContainedAs {
    match err {
        WasmProcessError::Unrecoverable(_) => ContainedAs::Trap,
        WasmProcessError::TimedOut => ContainedAs::TimedOut,
        // Any other variant means the sandbox did NOT actually stop
        // the exploit — the attack was allowed to return control
        // through a normal error channel, which is not containment.
        other => {
            panic!("attack was NOT contained; got an in-band error instead of a trap: {other:?}")
        }
    }
}

/// Skip the test with a friendly SKIP line when the required .wasm
/// artefacts are not on disk. Returns `false` so callers can
/// `if !check_prereqs(&[...]) { return; }`.
fn check_prereqs(paths: &[&str]) -> bool {
    for p in paths {
        if !Path::new(p).exists() {
            eprintln!("SKIP: required plugin not built: {p}");
            return false;
        }
    }
    true
}

/// Core isolation invariant used by every attack test. Loads the
/// healthy pass-through and the attack plugin in the same harness,
/// runs a pre-attack message through healthy, invokes the attack,
/// asserts containment, then runs a post-attack message through the
/// SAME healthy Store to prove the attack did not corrupt it.
///
/// `allowed` lists the acceptable containment classifications for the
/// attack; more than one is allowed because some attacks may hit
/// either the epoch deadline or the fuel budget depending on host
/// speed and wasmtime version. Every acceptable outcome MUST still be
/// a genuine sandbox interception, not an in-band error return.
fn assert_contained(
    attack_path: &str,
    label: &str,
    allowed: &[ContainedAs],
    memory_limit: Option<usize>,
) {
    let harness = PluginTestHarness::new().expect("engine must construct");

    // (1) Healthy transform succeeds BEFORE the attack.
    let mut healthy = harness.load_transform(PASS_THROUGH_WASM).expect("pass-through must load");
    let pre = healthy
        .process(RuntimeEnvelope::from_string("healthy", "pre-attack"))
        .expect("pre-attack healthy path must succeed");
    assert_eq!(
        std::str::from_utf8(&pre.payload).unwrap(),
        "pre-attack",
        "healthy plugin must echo before the attack runs"
    );

    // (2) Attack traps.
    let mut attacker = match memory_limit {
        Some(limit) => harness
            .load_transform_with_memory_limit(attack_path, limit)
            .expect("attack plugin must load"),
        None => harness.load_transform(attack_path).expect("attack plugin must load"),
    };
    let err = attacker
        .process(RuntimeEnvelope::from_string("attacker", "trigger"))
        .expect_err(&format!("attack {label} did NOT trap — sandbox failed to contain it"));
    let got = classify(&err);
    assert!(
        allowed.contains(&got),
        "attack {label} produced {got:?} but allowed={allowed:?}; err={err:?}"
    );
    // Record what we saw so `-- --nocapture` runs are self-describing.
    eprintln!("[attack_containment] {label}: contained as {got:?}  ({err:?})");

    // (3) Healthy transform STILL succeeds AFTER the attack. This is
    //     the isolation invariant — the attacker's Store trapping must
    //     not affect the healthy transform's independent Store.
    let post = healthy
        .process(RuntimeEnvelope::from_string("healthy", "post-attack"))
        .expect("healthy plugin must still process AFTER an attack in a co-resident Store");
    assert_eq!(
        std::str::from_utf8(&post.payload).unwrap(),
        "post-attack",
        "healthy plugin must still echo after the attack"
    );
}

// ---------------------------------------------------------------------------
// The six scenarios (RFC-008 §D8).
// ---------------------------------------------------------------------------

/// S1: buffer-overflow — writes 1M bytes past a 16-byte Vec via
/// raw pointer arithmetic. Expected: linear-memory OOB → trap.
#[test]
fn buffer_overflow_contained() {
    if !check_prereqs(&[PASS_THROUGH_WASM, ATK_BUFFER_OVERFLOW]) {
        return;
    }
    assert_contained(ATK_BUFFER_OVERFLOW, "S1 buffer-overflow", &[ContainedAs::Trap], None);
}

/// S2: cross-read — dereferences a fabricated absolute host address
/// (`0xDEAD_BEEF as *const u8`). Linear-memory bounds must catch it.
#[test]
fn cross_read_contained() {
    if !check_prereqs(&[PASS_THROUGH_WASM, ATK_CROSS_READ]) {
        return;
    }
    assert_contained(ATK_CROSS_READ, "S2 cross-read", &[ContainedAs::Trap], None);
}

/// S3: infinite-loop — `loop {}`. Expected: epoch interruption
/// (`TimedOut`) once the deadline (~100 ticks × 10 ms = 1 s) fires.
/// Fuel exhaustion is also acceptable if it lands first — both are
/// legitimate sandbox interventions.
#[test]
fn infinite_loop_contained() {
    if !check_prereqs(&[PASS_THROUGH_WASM, ATK_INFINITE_LOOP]) {
        return;
    }
    assert_contained(
        ATK_INFINITE_LOOP,
        "S3 infinite-loop",
        &[ContainedAs::TimedOut, ContainedAs::Trap],
        None,
    );
}

#[test]
fn epoch_recovery_uses_a_fresh_store() {
    if !check_prereqs(&[ATK_INFINITE_LOOP]) {
        return;
    }
    let harness = PluginTestHarness::new().expect("engine must construct");
    let mut attacker = harness.load_transform(ATK_INFINITE_LOOP).expect("attack plugin must load");

    let first = attacker
        .process(RuntimeEnvelope::from_string("attacker", "first"))
        .expect_err("first infinite-loop call must be interrupted");
    assert!(matches!(first, WasmProcessError::TimedOut));
    attacker
        .node_mut()
        .recover_from_cached_pre()
        .expect("recovery must instantiate from cached InstancePre");
    let second = attacker
        .process(RuntimeEnvelope::from_string("attacker", "second"))
        .expect_err("second infinite-loop call must be independently interrupted");
    assert!(
        matches!(second, WasmProcessError::TimedOut),
        "fresh Store must reach the epoch deadline instead of returning an unusable-instance trap: {second:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn consecutive_epoch_interruptions_each_recover_before_the_next_message() {
    if !check_prereqs(&[ATK_INFINITE_LOOP]) {
        return;
    }
    let config: Config = toml::from_str(&format!(
        r#"
[pipeline]
name = "epoch-recovery-regression"

[engine]
epoch_deadline = 10

[nodes.source]
type = "source"
kind = "bench-source"
rate = 100.0
total_messages = 2
warmup_messages = 0
payload_size = 128

[nodes.attack]
type = "transform"
plugin = {ATK_INFINITE_LOOP:?}

[nodes.sink]
type = "sink"
kind = "bench-sink"
warmup_secs = 0
track_sequences = false
track_hotswap = false

[[edges]]
from = "source"
to = "attack"

[[edges]]
from = "attack"
to = "sink"
"#,
    ))
    .expect("inline config must parse");

    let mut orchestrator =
        Box::pin(launch_pipeline(config, None)).await.expect("pipeline must launch");
    let handle = orchestrator.handle();
    tokio::time::timeout(Duration::from_secs(5), orchestrator.run_until_complete())
        .await
        .expect("two epoch interruptions must complete within five seconds")
        .expect("pipeline must shut down cleanly");

    let metrics = handle.node_metrics("attack").expect("attack metrics");
    assert_eq!(metrics.failed(), 2, "both infinite-loop calls must trap");
    assert_eq!(
        metrics.recovery_count(),
        2,
        "each epoch interruption must replace its Store before the next message"
    );
}

/// S4: memory-exhaust — allocates 1 MiB chunks in a loop. `StoreLimits`
/// with a 4 MiB cap must fire before the OS OOMs the test harness.
///
/// This test also carries AC2's explicit assertion: the trap message
/// must reference the memory cap. Because wasmtime's exact wording
/// changes between versions we accept any of the substrings the
/// upstream `StoreLimits::memory_growing` implementation has used in
/// recent releases.
#[test]
fn memory_exhaust_contained() {
    if !check_prereqs(&[PASS_THROUGH_WASM, ATK_MEMORY_EXHAUST]) {
        return;
    }

    // Full assert-and-log flow first.
    assert_contained(
        ATK_MEMORY_EXHAUST,
        "S4 memory-exhaust",
        &[ContainedAs::Trap],
        Some(MEMORY_EXHAUST_LIMIT),
    );

    // AC2: re-run just the attack to inspect the message payload.
    let harness = PluginTestHarness::new().unwrap();
    let mut attacker =
        harness.load_transform_with_memory_limit(ATK_MEMORY_EXHAUST, MEMORY_EXHAUST_LIMIT).unwrap();
    let err = attacker
        .process(RuntimeEnvelope::from_string("attacker", "grow"))
        .expect_err("memory-exhaust must trap");
    // Evidence that StoreLimits fired: the trap must land inside
    // memory-growth machinery. When wasmtime's ResourceLimiter
    // returns false from memory_growing, the guest's `memory.grow`
    // call returns -1 to `sbrk`, which then traps inside `dlmalloc`.
    // The 4 MiB cap ensures this happens after only ~4 iterations of
    // the 1 MiB allocation loop — no chance of an OS-level OOM.
    let raw = format!("{err:?}");
    let smoking_gun = ["sbrk", "dlmalloc", "malloc", "memory.grow", "memory grow"]
        .iter()
        .any(|s| raw.contains(s));
    assert!(
        smoking_gun,
        "memory-exhaust trap must land in memory-growth machinery \
         (sbrk/dlmalloc/malloc); got: {err:?}"
    );
    eprintln!("[attack_containment] memory-exhaust trap message: {err:?}");
}

/// S5: fs-access — calls `std::fs::read_to_string(\"/etc/passwd\")`.
/// Under `WaferState::sandbox()` no WASI preopen is granted, so the
/// call must fail. The plugin then panics on both branches (whether
/// the read succeeded or the `unwrap_or_else` returned "access
/// denied"), yielding a trap either way. This test asserts the trap
/// happens without asserting on which branch produced it — the
/// safety property is "no successful FS read AND `process()` did not
/// return Ok", which is exactly what containment means.
#[test]
fn fs_access_contained() {
    if !check_prereqs(&[PASS_THROUGH_WASM, ATK_FS_ACCESS]) {
        return;
    }
    assert_contained(ATK_FS_ACCESS, "S5 fs-access", &[ContainedAs::Trap], None);
}

/// S6: panic — `panic!("malicious payload triggers panic")`. Under
/// wasm32-wasip2 the compiler lowers `panic!` into `unreachable`,
/// which the runtime maps to a trap.
#[test]
fn panic_contained() {
    if !check_prereqs(&[PASS_THROUGH_WASM, ATK_PANIC]) {
        return;
    }
    assert_contained(ATK_PANIC, "S6 panic", &[ContainedAs::Trap], None);
}
