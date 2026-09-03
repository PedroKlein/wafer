# RQ2 — Attack Containment Evidence

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

1. **The attack DOES trap.** The plugin's `process()` call must return
   `WasmProcessError::Unrecoverable` or `TimedOut` — never a normal
   in-band return. Anything else means the exploit succeeded.
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
   `crates/wafer-core/src/runner/transform.rs:112-124` and P0.11).

## Scenarios

| ID | Plugin | Exploit | Expected trap kind |
|----|--------|---------|--------------------|
| S1 | `buffer-overflow` | Raw-pointer write 1 M bytes past a 16-byte `Vec` | `Unrecoverable` — linear-memory OOB |
| S2 | `cross-read` | `unsafe { *(0xDEAD_BEEF as *const u8) }` | `Unrecoverable` — linear-memory OOB |
| S3 | `infinite-loop` | `loop {}` | `TimedOut` (epoch) or `Unrecoverable` (fuel) |
| S4 | `memory-exhaust` | Allocate 1 MiB chunks until `StoreLimits` blocks | `Unrecoverable` — trap in `sbrk`/`dlmalloc` |
| S5 | `fs-access` | `std::fs::read_to_string("/etc/passwd")` then `panic!` | `Unrecoverable` — no WASI preopen granted, plugin panics either way |
| S6 | `panic` | `panic!("malicious payload triggers panic")` | `Unrecoverable` — panic lowers to `unreachable` under wasm32-wasip2 |

## Pass criteria

For each scenario the test asserts, in order:

1. **Pre-attack:** healthy pass-through processes `"pre-attack"` and
   echoes it back byte-for-byte.
2. **Attack:** `attacker.process(_)` returns `Err(err)` where
   `classify(&err) ∈ allowed` (see the per-scenario `allowed` list in
   `assert_contained`). Any `Ok(_)` return, and any `WasmProcessError`
   kind outside `{Unrecoverable, TimedOut}` (for example a
   `ProcessingFailed(_)` returned normally by the guest), fails the
   test — those would mean the attack was *not* contained.
3. **Post-attack:** the same healthy transform processes `"post-attack"`
   and echoes it back. This is the isolation invariant.

### Additional evidence for S4 (memory-exhaust)

Because S4 targets the `wasmtime::ResourceLimiter` layer specifically,
the test also asserts the trap backtrace lands inside memory-growth
machinery — one of `sbrk`, `dlmalloc`, `malloc`, `memory.grow`,
`memory grow` — with the store-memory cap set to **4 MiB**. Since the
plugin allocates 1 MiB per iteration, `StoreLimits` fires after ~4
iterations, well before any OS-level OOM could occur. The tight cap
is what gives us confidence the wasmtime limiter fired *before* the
kernel would have killed the test binary.

## Cross-cutting sandbox properties this test asserts

- **Capability isolation (S5).** `WaferState::sandbox()` grants no
  WASI preopens. `std::fs::read_to_string` therefore fails at the
  WASI boundary; the plugin's `panic!` on both success and failure
  branches ensures the test observes a trap regardless of the wasm
  runtime's exact FS-error-mapping behaviour.
- **CPU quota (S3).** `WaferEngine`'s epoch ticker (default
  `epoch_tick_ms = 10 ms`, `epoch_deadline = 100 ticks`) preempts an
  infinite loop within ~1 s. Fuel exhaustion (`DEFAULT_FUEL_LIMIT =
  10 000 000`) may fire first depending on host speed; both are
  legitimate. The test accepts both classifications.
- **Memory quota (S4).** `Store::limiter` bound to
  `WaferState::limits_mut()` enforces the per-store cap regardless
  of guest allocator (dlmalloc in std wasm32-wasip2).
- **Isolated linear memory (S1, S2).** Each Store owns its own
  linear memory region. OOB writes trap; OOB reads to a fabricated
  host address trap. Neither can escape to another Store.
- **Unreachable-as-abort (S6).** Rust's `panic!` under
  `wasm32-wasip2` lowers to `unreachable`, which wasmtime treats as
  a trap.

## Reproduction

```bash
# Build the attack plugins (once):
cd plugins && for a in attacks/*; do (cd $a && cargo build --release); done && cd -

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
