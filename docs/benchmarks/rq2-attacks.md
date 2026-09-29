# RQ2 — Attack Containment Evidence

> **Archived, not current.** macOS shakedown attack-containment evidence.
> Kept as a historical record only. Do not use it for decisions, commands,
> paths or numbers. Current source: [canonical readiness](../status/canonical-readiness.md).

<!-- historical-diagnostic-file -->

**Status:** informational (shakedown-macos). Canonical evidence pending
device runs. See `docs/status/canonical-readiness.md` for the promotion
gate.

**Provenance:**
- Test file: `crates/wafer-core/tests/attack_containment.rs`
- Attack plugins: `plugins/attacks/{buffer-overflow, cross-read, fs-access, infinite-loop, memory-exhaust, panic}/`
- Harness: `crates/wafer-core/src/testing/harness.rs::PluginTestHarness`
- Design spec: `docs/rfcs/RFC-008-evaluation-harness.md` §Decision 8 (E-Iso-1..6).

## What this document covers

RFC-008 §D8 defines eight fault-injection experiments. **E-Iso-1..6**
are the six adversarial-plugin scenarios below — automated correctness
tests, not latency benchmarks. **E-Iso-7** (parallel-branch topology)
and **E-Iso-8** (recovery latency) sit outside this file's scope; see
the parent plan tasks P4.\* and P0.4 respectively.

The containment invariant proved here has three parts:

1. **The attack is stopped by the expected mechanism.** The plugin's
   `process()` call must fail with the wasmtime trap code the scenario
   names (or, for S4 and S5, the memory-limit trap and the guest's
   denial report). Each attack plugin returns a `NOT CONTAINED` error
   when its access goes through, so an attack that is not stopped
   fails the test instead of passing on a later panic.
2. **The healthy neighbour KEEPS processing.** A co-resident
   pass-through transform, loaded in the same `PluginTestHarness`
   (same wasmtime `Engine`, independent `Store`), must still process
   a message *after* the attack traps. This is the sandbox isolation
   invariant: one node's fault must not poison another's.
3. **The runner would transition Error → Recovering.** We do not
   spin up the full orchestrator here (no channels, no metrics
   registry); we assert on the `WasmProcessError` variant, which is
   the exact signal the three runners (`transform/filter/router`)
   consume to drive `NodeStateTracker::transition_to_error` and,
   subsequently, `transition_recovering_to_running_timed()` (see
   `run_transform_loop` in `crates/wafer-core/src/runner/transform.rs`
   and P0.11). For S3 the runner applies the `timed_out` action and
   replaces the interrupted instance; the other scenarios take the
   recovery path.

## Scenarios

| ID | Plugin | Exploit | Expected containment |
|----|--------|---------|----------------------|
| S1 | `buffer-overflow` | Volatile write one byte past the end of linear memory | Trap `MemoryOutOfBounds` |
| S2 | `cross-read` | Volatile read from the fabricated address `0xDEAD_BEEF` | Trap `MemoryOutOfBounds` |
| S3 | `infinite-loop` | `loop {}` | Trap `Interrupt` (epoch deadline) |
| S4 | `memory-exhaust` | Allocate 1 MiB chunks until `StoreLimits` blocks | `StoreLimits` trap on the refused `memory.grow` (no trap code) |
| S5 | `fs-access` | `std::fs::read_to_string("/etc/passwd")` | Read fails (no WASI preopen); guest reports the denial as `unrecoverable` |
| S6 | `panic` | `panic!("malicious payload triggers panic")` | Trap `UnreachableCodeReached` (panic aborts under wasm32-wasip2) |

## Pass criteria

For each scenario the test asserts, in order:

1. **Pre-attack:** healthy pass-through processes `"pre-attack"` and
   echoes it back byte-for-byte.
2. **Attack:** `attacker.process(_)` returns `Err(err)` where
   `classify(&err)` equals the scenario's `expected` containment in
   `mandatory_scenarios()`. Any `Ok(_)` return, a trap with a
   different code, or any other `WasmProcessError` (such as the
   plugins' `NOT CONTAINED` `ProcessingFailed(_)`) fails the test.
3. **Post-attack:** the same healthy transform processes `"post-attack"`
   and echoes it back. This is the isolation invariant.

`mise run mandatory-attack-evidence` writes the same six checks as a
receipt. Each scenario records its `outcome` (`buffer-overflow-trap`,
`cross-read-trap`, `epoch-timeout`, `memory-limit-trap`,
`fs-read-denied`, `guest-panic-trap`), `contained_as` (`trap`,
`memory_limit` or `fs_denied`) and `trap_code` (the wasmtime trap code
for `trap`, null for S4 and S5), plus the healthy reference before and
after it.

### Additional evidence for S4 (memory-exhaust)

S4 targets the `wasmtime::ResourceLimiter` layer specifically. The
harness store sets `trap_on_grow_failure`, so the refused growth
surfaces as wasmtime's "forcing trap when growing memory" error rather
than as an allocator abort, and the test requires that error. The
store-memory cap is **4 MiB** and the plugin allocates 1 MiB per
iteration, so the limiter fires after a few iterations, well before
any OS-level OOM could occur.

## Cross-cutting sandbox properties this test asserts

- **Capability isolation (S5).** `WaferState::sandbox()` grants no
  WASI preopens. `std::fs::read_to_string` therefore fails at the
  WASI boundary. The plugin reports the denial as an `unrecoverable`
  error carrying `fs access denied as expected`; a successful read
  returns `NOT CONTAINED` instead.
- **CPU quota (S3).** The harness sets `epoch_deadline = 100 ticks` at
  the default `epoch_tick_ms = 10 ms`, so an infinite loop is preempted
  within ~1 s. The harness does not meter fuel, so the test expects the
  `Interrupt` trap. In a pipeline, fuel exhaustion is handled like an
  epoch interrupt (the `timed_out` action).
- **Memory quota (S4).** `Store::limiter` bound to
  `WaferState::limits_mut()` enforces the per-store cap regardless
  of guest allocator (dlmalloc in std wasm32-wasip2).
- **Isolated linear memory (S1, S2).** Each Store owns its own
  linear memory region. OOB writes trap; OOB reads to a fabricated
  host address trap. Neither can escape to another Store.
- **Unreachable-as-abort (S6).** Rust's `panic!` under
  `wasm32-wasip2` lowers to `unreachable`, which wasmtime treats as
  a trap.

What these properties do not cover:

- The memory cap bounds guest linear memory and tables. Host-side WASI
  resources a guest creates (for example resource-table entries) are not
  counted against it.
- Epochs and fuel bound time spent executing Wasm. A guest blocked
  inside a host import is not interrupted by either.

## Pipeline runs (E-Iso-1..8)

The canonical runs load the same plugins into a full pipeline and judge
containment from the attack node's row in `per_node_metrics.csv`
(`eval/scripts/lib/containment.py`). The run passes only when the column
below counted the attack and the node recorded nothing else: no other trap
kind, no other guest error, and no message passed downstream.

| Experiment / condition | Attack node | Counter that must stop it |
|---|---|---|
| E-Iso-1 `buffer-overflow`, E-Iso-2 `cross-read` | `attack` | `traps_memory_out_of_bounds` |
| E-Iso-3 `fs-access` | `attack` | `guest_unrecoverable` (the plugin's denial report) |
| E-Iso-4 `infinite-loop` | `attack` | `traps_interrupt` |
| E-Iso-5 `memory-exhaust` | `attack` | `traps_memory_limit` |
| E-Iso-6 `panic`, E-Iso-8 `panic-recovery` | `attack` | `traps_unreachable` |
| E-Iso-7 `panic-attack` / `epoch-loop-attack` | `branch_b` | `traps_unreachable` / `traps_interrupt` |

E-Iso-3 is the one scenario the host does not see: with no preopened
directory, wasi-libc fails the open inside the guest, so the evidence is the
guest's own `unrecoverable` report. The plugin reports a successful read
as `processing_failed`, which fails the verdict.

`traps_total` and the `traps_*` columns count only calls the host aborted;
errors the guest returned are in the `guest_*` columns, and
`attempts_failed` counts both. `containment.json` records `contained` as
null for conditions without an attack, such as the E-Iso-7 control run.

## Reproduction

```bash
# Build the attack plugins (once):
mise run //plugins:build-plugins

# Run all six scenarios:
cargo test -p wafer-core --test attack_containment

# Just S4, with the verbose trap details:
RUST_LOG=info cargo test -p wafer-core --test attack_containment memory_exhaust -- --nocapture
```

## Constraints that must NOT be relaxed

Per the plan (`P0.8 constraints`):

- No capability grant may be widened to make an attack succeed.
- `StoreLimits` numeric caps in the harness (`memory_limit`) are
  configurable *upwards* for realistic-workload benchmarks but never
  loosened to bypass an attack.
- Attack plugins remain under `plugins/attacks/` and are never listed
  in any config under `eval/configs/` or shipped in a production
  wafer-runtime manifest.

## Follow-ups (out of scope for P0.8)

- **E-Iso-7 — independent parallel branches (plan task P4.7).** Wires
  separate matched BenchSource → transform → BenchSink paths for the healthy
  and fault populations so a fault branch cannot throttle the measured branch
  through shared-source backpressure. This file's harness-level proof is a strictly
  weaker claim (isolation-in-principle); E-Iso-7 supplies the
  quantitative version (isolation-in-practice).
- **E-Iso-8 — recovery latency (plan task P4.8).** Measures the time
  from trap to first successful message after re-instantiation from
  the cached `InstancePre`. Requires the recovery-duration histogram
  landed by P0.11 (see `docs/status/implementation-gaps.md` §A7).
