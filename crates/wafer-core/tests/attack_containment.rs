#![cfg(test)]
#![expect(clippy::print_stderr, reason = "integration test: diagnostic output")]
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
//!    from the same `PluginTestHarness`, and the error names the
//!    mechanism the scenario expects: the exact wasmtime trap code
//!    (out-of-bounds access, epoch interrupt, `unreachable`) or, for
//!    the filesystem attack, the guest's own report that WASI refused
//!    the read. Every attack plugin returns a "NOT CONTAINED" error if
//!    its access goes through, so a sandbox that let the access happen
//!    fails the test instead of passing on a later panic.
//! 3. The healthy transform still processes a *subsequent* message
//!    successfully. Isolation invariant: an attack in one Store MUST
//!    NOT poison a co-resident, independently loaded Store belonging
//!    to another node.
//!
//! Assumptions kept explicit (constraints on the task):
//! - No capability grant is relaxed. `WaferState::sandbox()` is used
//!   through the default `PluginTestHarness` path.
//! - Attack .wasm artefacts are pre-built (`plugins/attacks/*/target/
//!   wasm32-wasip2/release/*.wasm`). A missing artefact fails the test:
//!   a skipped containment check would read as a passing one.
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

use serde_json::json;
use wafer_core::orchestrator::launch_pipeline;
use wafer_core::queue::RuntimeEnvelope;
use wafer_core::runner::error_policy::WasmProcessError;
use wafer_core::testing::PluginTestHarness;
use wafer_types::config::Config;

/// Pass-through plugin acts as the co-resident "healthy" transform.
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

/// How the sandbox stopped an attack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Containment {
    /// wasmtime trapped the call with this code.
    Trap(wasmtime::Trap),
    /// `StoreLimits` refused a `memory.grow` and trapped the call.
    MemoryLimit,
    /// The guest reported that WASI refused the filesystem read.
    FsDenied,
}

impl Containment {
    const fn label(self) -> &'static str {
        match self {
            Self::Trap(_) => "trap",
            Self::MemoryLimit => "memory_limit",
            Self::FsDenied => "fs_denied",
        }
    }
}

const FS_DENIED_MARKER: &str = "fs access denied as expected";
/// Error text of wasmtime's `StoreLimits` when `trap_on_grow_failure` is set.
const MEMORY_LIMIT_MARKER: &str = "forcing trap when growing memory";

/// Anything other than a coded trap or the guest's own denial report means
/// the sandbox did not stop the attack, including the attack plugins'
/// "NOT CONTAINED" error returned after an access that went through.
fn classify(err: &WasmProcessError) -> Containment {
    match err {
        WasmProcessError::Trapped { code: Some(code), .. } => Containment::Trap(*code),
        WasmProcessError::Trapped { code: None, message }
            if message.contains(MEMORY_LIMIT_MARKER) =>
        {
            Containment::MemoryLimit
        }
        WasmProcessError::Unrecoverable(message) if message.contains(FS_DENIED_MARKER) => {
            Containment::FsDenied
        }
        other => panic!("attack was NOT contained: {other:?}"),
    }
}

fn require_plugins(paths: &[&str]) {
    for p in paths {
        assert!(
            Path::new(p).is_file(),
            "required plugin not built: {p} (run `mise run //plugins:build-plugins`)"
        );
    }
}

/// Core isolation invariant used by every attack test. Loads the
/// healthy pass-through and the attack plugin in the same harness,
/// runs a pre-attack message through healthy, invokes the attack,
/// asserts it was stopped by `expected`, then runs a post-attack
/// message through the SAME healthy Store to prove the attack did not
/// corrupt it. Returns the attack's error for the receipt.
async fn assert_contained(
    attack_path: &str,
    label: &str,
    expected: Containment,
    memory_limit: Option<usize>,
) -> WasmProcessError {
    require_plugins(&[PASS_THROUGH_WASM, attack_path]);
    let harness = PluginTestHarness::new().expect("engine must construct");

    // (1) Healthy transform succeeds BEFORE the attack.
    let mut healthy =
        harness.load_transform(PASS_THROUGH_WASM).await.expect("pass-through must load");
    let pre = healthy
        .process(RuntimeEnvelope::from_string("healthy", "pre-attack"))
        .await
        .expect("pre-attack healthy path must succeed");
    assert_eq!(
        std::str::from_utf8(&pre.payload).unwrap(),
        "pre-attack",
        "healthy plugin must echo before the attack runs"
    );

    // (2) Attack is stopped by the expected mechanism.
    let mut attacker = match memory_limit {
        Some(limit) => harness
            .load_transform_with_memory_limit(attack_path, limit)
            .await
            .expect("attack plugin must load"),
        None => harness.load_transform(attack_path).await.expect("attack plugin must load"),
    };
    let err = attacker
        .process(RuntimeEnvelope::from_string("attacker", "trigger"))
        .await
        .expect_err(&format!("attack {label} returned Ok: sandbox failed to contain it"));
    let got = classify(&err);
    assert_eq!(got, expected, "attack {label} was stopped by the wrong mechanism; err={err:?}");
    // Record what we saw so `-- --nocapture` runs are self-describing.
    eprintln!("[attack_containment] {label}: contained as {got:?}  ({err:?})");

    // (3) Healthy transform STILL succeeds AFTER the attack. This is
    //     the isolation invariant — the attacker's Store trapping must
    //     not affect the healthy transform's independent Store.
    let post = healthy
        .process(RuntimeEnvelope::from_string("healthy", "post-attack"))
        .await
        .expect("healthy plugin must still process AFTER an attack in a co-resident Store");
    assert_eq!(
        std::str::from_utf8(&post.payload).unwrap(),
        "post-attack",
        "healthy plugin must still echo after the attack"
    );
    err
}

#[derive(Clone, Copy)]
struct MandatoryScenario {
    scenario_id: &'static str,
    label: &'static str,
    attack_path: &'static str,
    expected: Containment,
    memory_limit: Option<usize>,
    outcome: &'static str,
}

fn mandatory_scenarios() -> [MandatoryScenario; 6] {
    [
        MandatoryScenario {
            scenario_id: "S1",
            label: "S1 buffer-overflow",
            attack_path: ATK_BUFFER_OVERFLOW,
            expected: Containment::Trap(wasmtime::Trap::MemoryOutOfBounds),
            memory_limit: None,
            outcome: "buffer-overflow-trap",
        },
        MandatoryScenario {
            scenario_id: "S2",
            label: "S2 cross-read",
            attack_path: ATK_CROSS_READ,
            expected: Containment::Trap(wasmtime::Trap::MemoryOutOfBounds),
            memory_limit: None,
            outcome: "cross-read-trap",
        },
        MandatoryScenario {
            scenario_id: "S3",
            label: "S3 infinite-loop",
            attack_path: ATK_INFINITE_LOOP,
            expected: Containment::Trap(wasmtime::Trap::Interrupt),
            memory_limit: None,
            outcome: "epoch-timeout",
        },
        MandatoryScenario {
            scenario_id: "S4",
            label: "S4 memory-exhaust",
            attack_path: ATK_MEMORY_EXHAUST,
            expected: Containment::MemoryLimit,
            memory_limit: Some(MEMORY_EXHAUST_LIMIT),
            outcome: "memory-limit-trap",
        },
        MandatoryScenario {
            scenario_id: "S5",
            label: "S5 fs-access",
            attack_path: ATK_FS_ACCESS,
            expected: Containment::FsDenied,
            memory_limit: None,
            outcome: "fs-read-denied",
        },
        MandatoryScenario {
            scenario_id: "S6",
            label: "S6 panic",
            attack_path: ATK_PANIC,
            expected: Containment::Trap(wasmtime::Trap::UnreachableCodeReached),
            memory_limit: None,
            outcome: "guest-panic-trap",
        },
    ]
}

fn scenario(scenario_id: &str) -> MandatoryScenario {
    mandatory_scenarios()
        .into_iter()
        .find(|scenario| scenario.scenario_id == scenario_id)
        .expect("scenario is defined")
}

async fn run_scenario(scenario_id: &str) -> WasmProcessError {
    let s = scenario(scenario_id);
    assert_contained(s.attack_path, s.label, s.expected, s.memory_limit).await
}

#[tokio::test]
#[ignore = "run via eval/scripts/run-attack-evidence.py"]
async fn mandatory_attack_evidence_receipt() {
    let output = std::env::var("WAFER_ATTACK_EVIDENCE_OUTPUT")
        .expect("mandatory attack evidence output path must be provided");
    let mut scenarios = Vec::new();
    for scenario in mandatory_scenarios() {
        let err = assert_contained(
            scenario.attack_path,
            scenario.label,
            scenario.expected,
            scenario.memory_limit,
        )
        .await;
        let trap_code = match scenario.expected {
            Containment::Trap(code) => Some(format!("{code:?}")),
            Containment::MemoryLimit | Containment::FsDenied => None,
        };
        scenarios.push(json!({
            "scenario_id": scenario.scenario_id,
            "executed": true,
            "healthy_before": true,
            "healthy_after": true,
            "outcome": scenario.outcome,
            "contained_as": scenario.expected.label(),
            "trap_code": trap_code,
            "error": format!("{err:?}"),
        }));
    }

    let unique_outcomes = scenarios
        .iter()
        .map(|scenario| scenario["outcome"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();

    std::fs::write(
        output,
        serde_json::to_vec_pretty(&json!({
            "healthy_reference": {
                "executed": true,
                "path": PASS_THROUGH_WASM,
            },
            "scenarios": scenarios,
            "unique_outcomes": unique_outcomes,
        }))
        .expect("mandatory attack evidence receipt must serialize"),
    )
    .expect("mandatory attack evidence receipt must write");
}

// ---------------------------------------------------------------------------
// The six scenarios (RFC-008 §D8).
// ---------------------------------------------------------------------------

/// S1: buffer-overflow — writes one byte just past the end of linear
/// memory. Expected: out-of-bounds trap.
#[tokio::test]
async fn buffer_overflow_contained() {
    run_scenario("S1").await;
}

/// S2: cross-read — reads a fabricated absolute host address
/// (`0xDEAD_BEEF`). Linear-memory bounds must catch it.
#[tokio::test]
async fn cross_read_contained() {
    run_scenario("S2").await;
}

/// S3: infinite-loop — `loop {}`. Expected: epoch interruption once the
/// harness deadline (~100 ticks × 10 ms = 1 s) fires.
#[tokio::test]
async fn infinite_loop_contained() {
    run_scenario("S3").await;
}

#[tokio::test]
async fn epoch_recovery_uses_a_fresh_store() {
    require_plugins(&[ATK_INFINITE_LOOP]);
    let harness = PluginTestHarness::new().expect("engine must construct");
    let mut attacker =
        harness.load_transform(ATK_INFINITE_LOOP).await.expect("attack plugin must load");

    let first = attacker
        .process(RuntimeEnvelope::from_string("attacker", "first"))
        .await
        .expect_err("first infinite-loop call must be interrupted");
    assert!(
        matches!(first, WasmProcessError::Trapped { code: Some(wasmtime::Trap::Interrupt), .. }),
        "{first:?}"
    );
    attacker
        .node_mut()
        .recover_from_cached_pre()
        .await
        .expect("recovery must instantiate from cached InstancePre");
    let second = attacker
        .process(RuntimeEnvelope::from_string("attacker", "second"))
        .await
        .expect_err("second infinite-loop call must be independently interrupted");
    assert!(
        matches!(second, WasmProcessError::Trapped { code: Some(wasmtime::Trap::Interrupt), .. }),
        "fresh Store must reach the epoch deadline instead of returning an unusable-instance trap: {second:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn consecutive_epoch_interruptions_each_recover_before_the_next_message() {
    require_plugins(&[ATK_INFINITE_LOOP]);
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fuel_exhaustion_follows_the_timed_out_policy() {
    require_plugins(&[ATK_INFINITE_LOOP]);
    let dir = tempfile::tempdir().expect("tempdir");
    let dlq_path = dir.path().join("dlq.jsonl");
    let config: Config = toml::from_str(&format!(
        r#"
[pipeline]
name = "fuel-exhaustion-regression"

[engine.fuel]
transform = 1000000

[error_policy]
timed_out = "dlq"

[dead_letter]
kind = "file"
path = {dlq_path:?}

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
    tokio::time::timeout(Duration::from_secs(10), orchestrator.run_until_complete())
        .await
        .expect("two fuel exhaustions must complete within ten seconds")
        .expect("pipeline must shut down cleanly");

    let metrics = handle.node_metrics("attack").expect("attack metrics");
    assert_eq!(metrics.failed(), 2, "both infinite-loop calls must run out of fuel");
    assert_eq!(metrics.recovery_count(), 2, "each fuel trap must replace its Store");

    let records = std::fs::read_to_string(&dlq_path).expect("timed_out = dlq must write records");
    let records = records
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("DLQ record is JSON"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2, "{records:?}");
    for record in records {
        assert_eq!(record["error_category"], "timed_out", "{record}");
        let message = record["error_message"].as_str().expect("error message");
        assert!(message.contains("all fuel consumed"), "trap code missing: {message}");
    }
}

/// S4: memory-exhaust — allocates 1 MiB chunks in a loop. `StoreLimits`
/// with a 4 MiB cap must refuse the growth and trap the call before the
/// OS OOMs the test harness.
#[tokio::test]
async fn memory_exhaust_contained() {
    run_scenario("S4").await;
}

/// S5: fs-access — calls `std::fs::read_to_string("/etc/passwd")`.
/// Under `WaferState::sandbox()` no WASI preopen is granted, so the call
/// must fail and the guest must report the denial.
#[tokio::test]
async fn fs_access_contained() {
    run_scenario("S5").await;
}

/// S6: panic — `panic!("malicious payload triggers panic")`. Under
/// wasm32-wasip2 the compiler lowers `panic!` into `unreachable`,
/// which the runtime maps to a trap.
#[tokio::test]
async fn panic_contained() {
    run_scenario("S6").await;
}
