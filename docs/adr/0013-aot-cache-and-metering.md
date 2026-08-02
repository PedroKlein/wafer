# ADR-0013: AOT Compilation Cache and Per-Node Metering (Fuel + Epoch + Limits)

- **Date**: 2026-07-12
- **Status**: Implemented — AOT cache, OS-thread epoch ticker, per-type fuel budgets, and per-node `StoreLimits` overrides (A8 closed 2026-07-20). See [`docs/status/implementation-gaps.md`](../status/implementation-gaps.md).
- **Parent RFC**: [RFC-007](../rfcs/RFC-007-performance-optimizations.md)

> **Implementation status.** The blake3-keyed two-tier AOT cache,
> `StoreLimits` wrapper, and named `wafer-epoch-ticker` OS thread are
> live. `[engine.fuel]` per-node-kind budgets (transform / filter /
> router) and `[engine.memory]` per-node-kind limits (default 64 MiB
> transform, 16 MiB filter/router) plus per-node `fuel` / `memory_limit`
> overrides are honored by the launcher and preserved across hot-swap
> (A8).

## Context

WAFER's thesis evaluation requires both fast hot-swap preparation (RQ3) and per-node resource containment (RQ2), while simultaneously providing a mechanism to decompose Wasm boundary overhead into its constituent costs (RQ1). Three tightly-coupled decisions emerge from these requirements:

1. Compiling Wasm components from source bytes is expensive on constrained hardware (approximately 30ms per component on RPi 4). Hot-swap preparation benefits enormously from avoiding re-compilation of unchanged plugins. Cold-start of multi-node pipelines also suffers from sequential compilation overhead.

2. RQ2 scenario S4 ("memory bomb") requires per-node memory containment — a misbehaving plugin growing its linear memory must not destabilise sibling nodes or the host process. The wasmtime `StoreLimits` mechanism provides zero-hot-path-overhead containment triggered only on rare `memory.grow` calls.

3. RQ1's overhead decomposition demands the ability to independently enable/disable fuel metering and epoch interruption, creating a four-way measurement matrix. The epoch ticker itself must fire reliably under full load, which a Tokio-spawned task cannot guarantee when all worker threads are executing Wasm.

## Decision

We implement three sub-decisions as a cohesive unit:

**AOT compilation cache.** A two-tier (in-memory + disk) content-addressed cache for pre-compiled `.cwasm` artifacts. The cache key is `{blake3_hex(wasm_bytes)}-{os}-{arch}-wt{wasmtime_major}.cwasm`, incorporating platform and engine version to prevent cross-platform or cross-version deserialization failures. The lookup path is: in-memory `HashMap<[u8; 32], Arc<Component>>` → disk deserialization → full compile + save to both tiers. Stale disk entries (wasmtime version mismatch) are evicted on deserialization failure. The in-memory map uses `foldhash` (cold-path only — no DoS resistance needed). Implementation lives in `crates/wafer-core/src/engine/cache.rs` (~150 LOC). Pattern validated by Flow-Like (blake3 + platform key, 244K/sec production) and Torvyn (SHA-256 variant).

**StoreLimits per-node.** Every Wasm node's `Store<WaferState>` is constructed with `StoreLimitsBuilder` configured to `trap_on_grow_failure(true)`. Default memory limit is 16 MiB (all node types); the RFC specifies per-type defaults of 64 MB for Transform and 16 MB for Filter/Router, overridable per-node in the TOML config. The default table element limit is 20,000. This has zero hot-path overhead — the limiter's `memory_growing` hook fires only on `memory.grow` instructions, which typically occur during guest startup and never on steady-state message processing. Implementation lives in `crates/wafer-core/src/engine/state.rs`.

**OS-thread epoch ticker.** The epoch incrementer runs on a dedicated OS thread (`std::thread::Builder::new().name("wafer-epoch-ticker")`) rather than a Tokio task. This guarantees epoch interrupts fire even when all Tokio worker threads are blocked inside Wasm execution — a scenario that is plausible on RPi 4 (4 cores, 4 Tokio workers, 4+ Wasm nodes). The thread holds a `Weak<Engine>` and self-terminates when the engine is dropped. Fuel and epoch are independently toggleable boolean flags in the `[engine]` TOML section (both default to enabled), enabling a four-way decomposition for RQ1 overhead measurement: (A) both enabled, (B) fuel-only, (C) epoch-only, (D) neither. Per-type fuel budgets: Transform 10M, Filter 500K, Router 500K instructions.

## Consequences

- **Positive: Hot-swap preparation drops from ~30ms to ~1-2ms on RPi 4** when the target plugin is cache-resident. The cache key guarantees correctness — a changed binary always recompiles.
- **Positive: Cold-start pipeline setup can compile plugins in parallel** (via `tokio::JoinSet`) because compiled components are `Arc<Component>` and immutable. Five-plugin cold start: ~150ms → ~40ms on 4 cores.
- **Positive: RQ2 S4 containment is structurally guaranteed.** A memory-bomb plugin traps immediately on `memory.grow` beyond its limit. Other nodes are unaffected — each has an independent `StoreLimits` instance.
- **Positive: Zero steady-state overhead for limits.** `memory.grow` is a guest startup operation, not a per-message operation. The limiter hook adds no latency to the message-processing hot path.
- **Positive: Four-way metering decomposition enables precise RQ1 attribution.** The thesis can report the exact overhead contribution of fuel tracking vs epoch interruption vs the pure WIT boundary cost, independently.
- **Positive: Epoch reliability under saturation.** The OS-thread model eliminates the race condition where all Tokio workers are blocked in Wasm and the ticker task starves — critical for the RQ2 S3 (infinite loop) containment guarantee.
- **Negative: `unsafe` deserialization required.** `Component::deserialize()` on disk cache entries is `unsafe` — malicious `.cwasm` files could violate memory safety. Mitigated by: the cache directory is written only by the WAFER process itself; external `.cwasm` files are never accepted.
- **Negative: Disk cache accumulates stale files.** When wasmtime is upgraded, all existing `.cwasm` files become invalid and are evicted on next load. No proactive garbage collection is implemented — stale files remain until accessed.
- **Negative: OS-thread epoch ticker is one more resource to manage.** It self-terminates via `Weak::upgrade` returning `None` when the engine drops, but adds a dedicated OS thread to the process for its lifetime.
- **Forecloses: JIT-style re-optimization.** The AOT cache model assumes a single compilation is final. Adaptive re-compilation (e.g., with profiling data) would require cache invalidation support not currently designed.
- **Forecloses: Per-message fuel accounting.** Fuel budgets are per-call (one Wasm invocation per message). Cross-message fuel accumulation or per-flow budgeting is not supported by this design.
- **Downstream requirement:** The evaluation harness (RFC-008) must configure the four metering combinations as distinct benchmark scenarios and report overhead per-configuration.

## See Also

- [RFC-007 — Performance Optimizations](../rfcs/RFC-007-performance-optimizations.md) — the long-form decision this ADR summarises (§D1, §D6, §D9, §C1).
- [ADR-0001](0001-wasmtime-runtime.md) — Wasmtime selection (provides fuel/epoch/Component Model).
- [RFC-001](../rfcs/RFC-001-wit-contracts.md) — `borrow<buffer>` design that makes filter calls cheap enough to skip fusion.
- [RFC-005](../rfcs/RFC-005-orchestrator.md) — builder + `JoinSet` spawn model that enables parallel compilation.
- `crates/wafer-core/src/engine/cache.rs` — AOT cache implementation.
- `crates/wafer-core/src/engine/state.rs` — `WaferState` with `StoreLimits`.
- `crates/wafer-core/src/engine/loader.rs` — `ensure_epoch_ticker()` using `std::thread::spawn`.
- `crates/wafer-types/src/config/engine.rs` — `EngineConfig`, `FuelBudgets` structs.
