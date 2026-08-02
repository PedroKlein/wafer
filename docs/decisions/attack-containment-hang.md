# `cargo test --workspace` hang on `attack_containment.rs`

**Status:** Resolved  
**Date:** 2026-08-02  
**Context:** thesis-hardening plan, task T7

## Problem

`cargo test --workspace` (and `cargo test -p wafer-core --test attack_containment`)
hung indefinitely. The `infinite_loop_contained` test never terminated. Per-test
runs of the other 5 attack scenarios passed; only the infinite-loop scenario
(which relies on epoch interruption) failed.

## Bisect

```
$ cargo test -p wafer-core --test attack_containment -- --test-threads=1 --nocapture
running 6 tests
test buffer_overflow_contained ... ok           (< 1s)
test cross_read_contained ... ok                (< 1s)
test fs_access_contained ... ok                 (< 1s)
test infinite_loop_contained ...                ← HANGS HERE
```

The hang begins at `infinite_loop_contained` and never progresses. All trap-based
scenarios (S1, S2, S4, S5, S6) pass because they trap on memory/fault violations
which don't depend on epoch timing. S3 (infinite loop) is the only scenario that
requires the epoch interrupt to fire.

## Root Cause (test artefact, NOT a runtime deadlock)

Introduced by commit `6096dfd` ("core: F5.AC2 skip fuel/epoch setters when limit
is None").

Before that commit, `PluginTestHarness::new()` created an engine via
`WaferEngine::new()` (default `EngineConfig` with `epoch_deadline: None`), but
the `load_transform_with_memory_limit` method unconditionally called:
```rust
store.set_fuel(u64::MAX);
store.epoch_deadline_trap();
store.set_epoch_deadline(u64::MAX / 2);
```

The `epoch_deadline_trap()` call enabled Store-level epoch trapping even though the
Config-level `epoch_interruption` was technically unset (wasmtime silently allowed
this pre-6096dfd because `epoch_interruption` was always `true` in the engine's
wasmtime Config — the old code hard-coded `config.epoch_interruption(true)`).

Commit `6096dfd` correctly gated `config.epoch_interruption(...)` on whether
`epoch_deadline` is `Some`. With the default `None`, epoch interruption was
disabled at the Engine level. The Store-level guards (`epoch_deadline_trap`,
`set_epoch_deadline`) became no-ops. Consequence: the epoch ticker thread ran
but could never fire a trap; the infinite-loop plugin looped forever.

**Two compounding factors:**
1. `WaferEngine::new()` → `EngineConfig::default()` → `epoch_deadline: None` →
   `config.epoch_interruption(false)`.
2. `WasmTransformNode::new(...)` hardcodes `epoch_deadline: None` in its
   constructor, so the per-call `set_epoch_deadline` reset inside `process()` was
   also a no-op (guarded by `if let Some(n) = self.epoch_deadline`).

## Fix

Changed `PluginTestHarness::new()` to use a sandbox-safe default engine
configuration with `epoch_deadline: Some(NonZeroU64::new(100))` (100 ticks × 10 ms
= ~1 s timeout per guest call). Added `PluginTestHarness::with_engine_config()`
for callers that need custom metering (benchmarks, unlimited tests).

Additionally, after constructing `WasmTransformNode`, the harness now calls
`node.configure_runtime(...)` to propagate the epoch deadline into the node,
ensuring the per-call epoch reset inside `process()` fires.

**Files changed:**
- `crates/wafer-core/src/testing/harness.rs` — harness constructor and node
  wiring.

## Consequences

- `cargo test --workspace` completes in ~29 s (well under the 5 min AC).
- All 6 attack containment scenarios pass (S3 takes ~1.6 s due to epoch timeout).
- Benchmarks using `PluginTestHarness::new()` now have epoch-interruption enabled
  at 100 ticks (~1 s per call). This is generous enough for any well-behaved plugin
  in benchmark loops. If a benchmark ever needs truly unlimited execution time per
  call, it should use `PluginTestHarness::with_engine_config(EngineConfig { epoch_deadline: None, .. })`.
- No new runtime A-gap filed: this is a test-harness configuration issue, not a
  runtime deadlock.

## Verification

```
$ time cargo test --workspace 2>&1 | tail -3
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
real    0m29.229s
```

```
$ cargo test -p wafer-core --test attack_containment -- --nocapture 2>&1 | grep "test result"
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.23s
```
