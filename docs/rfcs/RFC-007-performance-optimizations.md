# RFC-007: Performance Optimizations

- **Status:** Partially implemented — the compiled-component cache exists but is not wired into runtime startup; the epoch ticker, code-quality optimizations, per-type fuel, and `StoreLimits` are wired in the launcher, and criterion benchmarks run against the production Wasm path.
- **Original session date:** 2026-07-12
- **Amends:** RFC-002 (WaferState gains `limits: StoreLimits` field), RFC-004 (`[engine]` section gains fuel/epoch toggles)

> **Implementation notes.** The compiled-component cache implementation exists,
> but production startup constructs a memory-only cache. The epoch OS-thread
> ticker and code-quality optimizations are live. `[engine.fuel]` and
> `[engine.memory]` per-node-kind budgets are honored by the launcher
> (A8). `benches/throughput.rs` and `benches/hot_swap.rs` exercise the
> production `PluginTestHarness` and `prepare_transform_swap_timed`
> paths with an `assert_no_stub_backed_evidence` guard against the
> retired stub `TransformInstance` (A15).

## Abstract

This RFC establishes a performance optimization strategy for WAFER's thesis evaluation, balancing measurement quality against implementation complexity. It introduces a blake3-keyed AOT compilation cache for near-instant hot-swap preparation and cold-start, per-node `StoreLimits` for memory containment (RQ2 S4), independent fuel and epoch metering toggles for thesis overhead decomposition, parallel plugin compilation at startup, a correctness fix moving the epoch ticker to a dedicated OS thread, and several code-quality improvements (`Box<str>` immutable fields, `foldhash` for internal maps, RAII processing guards). Several theoretically beneficial optimizations (pooling allocator, filter fusion, host-native expression filters, `Sender::reserve`) are explicitly deferred with documented thesis-defense rationale, following a "measure first, optimize second" principle.

## Context

With the complete runtime architecture designed in Sessions 1–6, this session decided which performance optimizations to implement before the thesis evaluation and which to defer. The key constraint was maximizing evaluation quality without over-optimizing before measuring.

The post-refactor baseline already incorporates substantial performance decisions from prior sessions: `Bytes` payload with refcount-bump clone (~5ns), `Arc<EnvelopeHeader>` (~5ns clone), persistent Store per node (~77ns per-message overhead), `InstancePre` for hot-swap (~5µs instantiation), `borrow<buffer>` for zero-copy inspection, direct tokio mpsc channels (~100ns send+recv), and a typed nop call boundary measured at ~27ns on x86 (~50-80ns estimated on ARM64).

RQ1 targets are <50µs per WIT boundary crossing on RPi 4 and within 30% of eKuiper throughput (~12K msg/s on RPi 3B+). RQ2 requires per-node memory containment for attack scenario S4. RQ3 benefits from faster hot-swap preparation via AOT cache (compilation drops from ~30ms to ~1-2ms on RPi 4).

## Decisions

### Decision 1: AOT Compilation Cache — Implement Now

Implement a blake3-keyed disk cache for pre-compiled `.cwasm` component artifacts. Cache key: `{blake3_hex(wasm_bytes)}-{os}-{arch}-wt{wasmtime_major_version}.cwasm`. Two-tier architecture: in-memory `HashMap<[u8; 32], Arc<Component>>` plus disk `{cache_dir}/components/{cache_key}.cwasm`. Load path: in-memory → disk (`unsafe { Component::deserialize() }`) → compile + save to both tiers. Stale files removed on deserialization failure (wasmtime version mismatch). Pattern validated by Flow-Like (244K/sec production, blake3+platform key) and Torvyn (SHA-256 variant).

### Decision 2: Wasmtime Pooling Allocator — Defer to Future Work

Do NOT implement the pooling allocator. With WAFER's persistent Store model, instantiation happens only at startup, on unrecoverable errors (rare), and on hot-swap (already fast with InstancePre at ~5µs). Total savings across entire pipeline lifetime would be ~25µs — unmeasurable. The pooling allocator targets per-request models (Spin), not persistent-node models.

### Decision 3: BoundedQueue Wrapper — Removed by Architecture

No explicit removal needed. The `BoundedQueue`/`QueueSender`/`QueueReceiver` wrapper types are superseded by the builder design (RFC-005) which directly creates `mpsc::channel()` and distributes raw `Sender`/`Receiver` halves.

### Decision 4: Filter Chain Fusion — Skip Entirely

Do NOT implement filter chain fusion. Post-refactor filters cost ~0.5-1µs per hop; channel transit adds ~100-200ns — negligible vs Wasm call. Fusion would break per-node metrics, per-node hot-swap, and per-node error policy — all core thesis features. The thesis contribution IS per-node isolation; fusing filters contradicts this.

### Decision 5: Host-Native Expression Filters — Skip Entirely

Do NOT implement host-native expression evaluation. A Wasm filter inspecting metadata costs ~0.5-1µs — already competitive with eKuiper's Go-based SQL WHERE clause (~1M msg/s theoretical on single core, 80× the eKuiper comparison target). Bypassing Wasm undermines the thesis argument that Wasm isolation is viable for ALL pipeline stages.

### Decision 6: Fuel & Epoch — Keep Both, Independent Boolean Flags

Keep both fuel metering and epoch interruption, each independently configurable via boolean flags in `[engine]` (both default `true`). This enables four measurement configurations for thesis decomposition: (A) both enabled (production), (B) fuel-only, (C) epoch-only, (D) neither (pure Wasm boundary cost). Fuel provides deterministic instruction budgets; epoch provides wall-clock timeout for host-import hangs and WASI blocking. Per-type fuel budgets: transform 10M, filter 500K, router 500K.

### Decision 7: Sender::reserve — Skip

Do NOT adopt `Sender::reserve`. In WAFER's post-refactor loops, `sender.send(output).await` is outside `select!` — runs to completion after Wasm call. No cancel point intersects the send path. Adding `reserve` would be unnecessary complexity.

### Decision 8: Benchmark-First Strategy

Establish measurements before optimizing. Phase A: local development benchmarks (criterion, fast iteration). Phase B: evaluation-grade benchmarks (RPi 4, Jetson Orin — fixed CPU frequency, isolated cores, open-loop load gen, N=30-50 reps, 30s warmup exclusion, HdrHistogram, Mann-Whitney U, Bootstrap 95% CI). Phase C: cross-architecture validation on x86.

### Decision 9: Memory Limits Per-Node (StoreLimits) — Implement Now

Configure `StoreLimitsBuilder` with per-type memory limits on every Wasm node's Store. Required for RQ2 S4 attack containment. Per-type defaults: Transform 64MB, Filter/Router 16MB. Configurable per-node via override field. Zero hot-path overhead (triggered only on `memory.grow` calls — rare after startup).

### Decision 10: Compilation Parallelism — Implement Now

Compile multiple plugins in parallel at pipeline startup using `tokio::JoinSet`. Cold start (5 plugins, no cache): ~150ms sequential → ~40ms parallel (4 cores). Warm start (cache hit): ~10ms → ~3ms. Trivial implementation — no shared mutable state.

### Decision C1: Epoch Ticker as OS Thread — Correctness Fix

Change the epoch ticker from `tokio::spawn` to `std::thread::spawn`. If all Tokio worker threads are blocked executing Wasm simultaneously (high load, RPi 4 with 4 workers and 4+ nodes), the tokio-based ticker never fires — an infinite-loop plugin hangs forever. The OS thread fires regardless of Tokio saturation. Required for RQ2 S3 (infinite loop) containment.

### Decision C2: Box<str> for Immutable Envelope Fields

Use `Box<str>` instead of `String` for immutable fields in `EnvelopeHeader` (`id`, `source`, `content_type`, metadata keys/values). Primary value is code clarity (communicates immutability in the type system), with minor memory savings (no capacity word).

### Decision C3: foldhash for Internal HashMaps

Use `foldhash` as the default hasher for internal lookup maps (node-by-id, edge routing tables, cache keys). Internal maps use trusted keys; SipHash's DoS resistance wastes ~30% throughput on small keys. Applied to cold-path maps only — hot-path node loops use no HashMaps.

### Decision C4: RAII Guard for NodeState::Processing

Replace manual `set_processing(true)` / `set_processing(false)` with a Drop guard. An early `?` return or panic between manual pairs leaves the node permanently marked as processing — breaking drain detection. The RAII guard makes cleanup impossible to forget.

## Alternatives Considered

- **Pooling allocator (Decision 2):** Evaluated because Spin uses it successfully. Rejected because WAFER's persistent-Store model instantiates only a handful of times per pipeline lifetime — the pooling allocator's virtual-memory pre-allocation benefits are unmeasurable for our access pattern.
- **Filter chain fusion (Decision 4):** Evaluated as a standard stream-processing optimization (Hirzel 2014). Rejected because it directly contradicts WAFER's core contribution of per-node isolation, breaking per-node hot-swap, metrics, and error policy for <200ns savings per hop.
- **Host-native expression filters (Decision 5):** Evaluated to potentially match eKuiper's SQL WHERE performance. Rejected because bypassing Wasm for "simple" filters undermines the thesis argument that typed Wasm isolation is viable for ALL stages.
- **`Sender::reserve` pattern (Decision 7):** Evaluated per async-tokio skill recommendation. Rejected because WAFER's post-refactor node loops perform sends outside `select!` branches — no cancel hazard exists.
- **SHA-256 for cache key (Decision 1):** Torvyn uses SHA-256. Rejected in favor of blake3 (faster, 2× throughput on small inputs, hardware acceleration path). Cache integrity doesn't need cryptographic collision resistance.
- **DashMap for in-memory cache (Decision 1):** Flow-Like uses DashMap for concurrent access. WAFER uses a simple HashMap because cache is populated at build time (single-threaded) and read-only thereafter.
- **`Arc<AtomicBool>` vs `Weak<Engine>` for epoch thread shutdown (Decision C1):** Both options documented; implementation chooses based on ergonomics. `AtomicBool` is simpler and more explicit.

## Related RFCs

- **RFC-002** — provides the persistent Store model (D10) that makes the pooling allocator unnecessary; this RFC amends WaferState to add `limits: StoreLimits`.
- **RFC-004** — provides the `[engine]` config section; this RFC amends it with independent fuel/epoch toggle flags and per-type fuel budgets.
- **RFC-005** — provides the builder + `JoinSet` spawn architecture that Decision 10 (parallel compilation) plugs into; confirms the node loop structure that Decision C4 (RAII guard) protects.
- **RFC-001** — provides the `borrow<buffer>` zero-copy input that makes filter/router calls cheap (~0.5µs), supporting the decision to skip filter fusion.

## Implementation Notes

- **Decision 1 (compiled cache):** The two-tier implementation exists in `crates/wafer-core/src/engine/cache.rs`, but `launch_pipeline` constructs `WaferEngine::from_engine_config`, so the disk tier is not active in production startup. E-Perf-9 must not claim a compiled-cache hit until that wiring and artifact provenance exist.
- **Decision 6 (fuel/epoch flags):** Implemented in `crates/wafer-types/src/config/engine.rs` — `EngineConfig` struct with `fuel_enabled`, `epoch_enabled`, `epoch_tick_ms`, `epoch_deadline` fields and `FuelBudgets` sub-struct.
- **Decision 9 (StoreLimits):** Implemented via `StoreLimitsBuilder` in the Store constructor per-node, with `trap_on_grow_failure(true)`.
- **Decision C1 (epoch OS thread):** Implemented in the engine/orchestrator startup path using `std::thread::Builder::new().name("wafer-epoch-ticker")`.
- **Decision C2 (Box<str>):** Applied to `EnvelopeHeader` fields and `node_id` in `WaferState`.
- **Decision C3 (foldhash):** Applied to cold-path internal maps.
- **Decision C4 (RAII guard):** `ProcessingGuard` type used in node loops.
- **Decisions 2, 4, 5, 7 (deferred/skipped):** Not implemented; documented as future work with thesis-defense rationale.
