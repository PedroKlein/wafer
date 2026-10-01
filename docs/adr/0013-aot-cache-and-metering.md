# ADR-0013: Compiled-component cache and per-node metering

- **Date:** 2026-07-12
- **Status:** Implemented with evaluation clarification dated 2026-09-04
- **Parent RFC:** [RFC-007](../rfcs/RFC-007-performance-optimizations.md)

## Context

WAFER needs independent CPU and memory limits for untrusted components and bounded hot-swap preparation. The evaluation also needs to separate fuel overhead, epoch overhead, and the unmetered Component Model boundary.

The runtime contains a content-addressed compiled-component cache module. Current initial pipeline loading and E-Perf-9 do not use the disk serialized-component tier. E-Perf-9 therefore measures Linux filesystem page-cache preparation, not an AOT-cache benefit.

## Decision

### Runtime metering defaults

`EngineConfig` uses optional fuel and epoch limits. The defaults are:

| Field | Runtime default |
|---|---|
| `engine.epoch_deadline` | `None` |
| `engine.epoch_tick_ms` | `10` ms |
| `engine.fuel.transform` | `None` |
| `engine.fuel.filter` | `None` |
| `engine.fuel.router` | `None` |
| `engine.memory.transform` | 64 MiB |
| `engine.memory.filter` | 16 MiB |
| `engine.memory.router` | 16 MiB |

Omitting a fuel value or `epoch_deadline` produces `None`; the runtime skips the corresponding Wasmtime mechanism. A positive integer produces `Some(NonZeroU64)`. Zero is rejected. No sentinel value represents disabled metering.

Fuel metering is engine-wide. It is on when any `[engine.fuel]` budget or any Wasm node's own `fuel` is set. In a metered engine every Wasm node gets a budget before each call: its own `fuel`, else its role's budget, else `u64::MAX` (a Store without fuel would trap on its first instruction). Fuel exhaustion and epoch interrupts are handled as timeouts by the node's `timed_out` error-policy action (ADR-0008).

### Final evaluation policy

Runtime defaults and evaluation policy are separate. Ordinary final WAFER leaves explicitly configure:

```toml
[engine]
epoch_deadline = 100
epoch_tick_ms = 10

[engine.fuel]
transform = 10000000
filter = 500000
router = 500000
```

The final matrix owns the exceptions. E-Perf-7 uses four parsed modes:

| Condition | Transform fuel | Epoch deadline |
|---|---|---|
| `neither` | omitted, `None` | omitted, `None` |
| `fuel-only` | `Some(10_000_000)` | omitted, `None` |
| `epoch-only` | omitted, `None` | `Some(100)` |
| `both` | `Some(10_000_000)` | `Some(100)` |

Attack experiments may omit fuel when fuel would preempt the mechanism under test. Those exceptions are declared in `eval/canonical-matrix.json` and verified against runtime provenance.

### Store limits

Each Wasm node has an independent `StoreLimits` instance with `trap_on_grow_failure(true)`. The defaults are 64 MiB for Transform and 16 MiB for Filter and Router. Per-node TOML overrides remain available. A memory-exhausting component cannot consume another node's linear memory.

### Epoch ticker

The epoch ticker runs on a named OS thread. It holds a weak engine reference and exits when the engine is dropped. This keeps epoch interruption available when Tokio worker threads are occupied by guest execution. The tick interval defaults to 10 ms, but no call deadline exists unless `epoch_deadline` is configured.

### Compiled-component cache

`crates/wafer-core/src/engine/cache.rs` implements an in-memory and disk content-addressed cache for compiled components. Its identity includes component bytes, platform, and Wasmtime version. Deserialization of a serialized component is unsafe and is restricted to artifacts written by WAFER in its cache directory.

The runtime builds the cache memory-only. Launch compiles every plugin through it, and hot-swap preparation compiles and links on tokio's blocking pool through the same cache, so a swap back to a binary already loaded is a memory hit; the swap response reports `compile_cache` as `memory_hit`, `disk_hit`, or `compiled`. The disk tier is never used by the runtime.

The presence of this module is an implementation fact, not evidence that a particular evaluation used it. Run metadata must state cache mode, hit state, artifact, and identity. Current E-Perf-9 records `mode="disabled"`, `hit=false`, and null artifact/identity.

## Consequences

- Production users opt into CPU metering explicitly; omission remains unlimited.
- Final WAFER measurements use a protected configuration unless the matrix declares an exception.
- E-Perf-7 measures true mechanism omission rather than large numeric budgets.
- Memory limits are independent of fuel and epoch settings.
- Cache performance claims require a run whose provenance records an enabled cache and a valid hit. E-Perf-9 cannot support that claim.
- Per-call fuel accounting remains the implemented model. Cross-message fuel budgets are outside scope.

## Verification

- Defaults: `crates/wafer-types/src/config/engine.rs`
- Parsed config tests: `crates/wafer-config/tests/eval_configs_load.rs`
- Runtime provenance: `crates/wafer-runtime/src/metadata.rs`
- Final policy and exceptions: `eval/canonical-matrix.json`
- Startup cache state: `eval/RESULT-CONTRACT.md`, `startup.json`

## Related documents

- [RFC-007](../rfcs/RFC-007-performance-optimizations.md)
- [RFC-008](../rfcs/RFC-008-evaluation-harness.md)
- [Config schema](../interfaces/config-schema.md)
