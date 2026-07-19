# Phase 0 — Discussion Sessions Plan

> Each session produces **decisions only** — no implementation.  
> Implementation happens after all discussions are resolved, with full awareness of every change.

---

## Session 1: WIT Contracts & Envelope Design ✅ DONE

**Status:** Complete (2025-07-05)  
**Output:** `docs/decisions/2025-07-05-wit-contracts-envelope-design.md`  
**Decisions made:** 18 (8 original + 10 follow-ups)

Covered: envelope shape (two asymmetric types), process-result return type, router/joiner interfaces, payload type (list<u8> + content-type), metadata mutability, value passing vs resource handles (borrow<buffer> input, list<u8> output), filter specialization, package structure (4 packages), error categories, init config shape, validate() retention, filter forwarding, splitting scope, logging imports, buffer lifetime, content-type location, inference node world.

Also resolved topics originally planned for Session 2: config format (string/JSON), init signature, close() naming, error type from init, plugin-specific vs opaque config.

---

## Session 2: Host-Side Runtime Architecture ✅ DONE

**Status:** Complete (2025-07-06)  
**Output:** `docs/decisions/2025-07-06-host-runtime-architecture.md`  
**Decisions made:** 10

Covered: RuntimeEnvelope redesign (`bytes::Bytes` payload, `Vec<(String, String)>` metadata, lineage fields), buffer resource implementation (WaferBuffer in wasmtime ResourceTable, call-scoped lifecycle), queue format (unchanged mpsc with Bytes refcount-bump sends), error policy engine (per-node TOML config, 5-category mapping, bounded VecDeque retry with backoff), hot-swap buffer safety proof (call-scoped = no issue), filter zero-copy forwarding (Bytes clone through queue), lineage tracking (parent_id + trace_id on envelope), WaferState expansion (log_buffer + node_id), InstancePre adoption for hot-swap, Store lifecycle confirmation (persistent, cancel-safe by design).

Also resolved: `Arc<[u8]>` vs `bytes::Bytes` (Bytes wins — already in dep tree, zero-copy from MQTT, sub-slicing), per-call vs persistent Store (persistent wins — 77ns vs 5µs per message, cancel-safety proven via select! pattern in loops.rs), retry buffer design (VecDeque with priority, bounded, flushed to DLQ on hot-swap), error policy TOML schema (composition with overflow_policy clarified).

---

## Session 3: Node Type Architecture (Host Side) ✅ DONE

**Status:** Complete (2025-07-06)  
**Output:** `docs/decisions/2025-07-06-node-type-architecture.md`  
**Decisions made:** 12 + 3 amendments to Sessions 1 & 2

Covered: Node type set (4 types: Transform, Filter, Router, Merge), operator taxonomy (two categories: data-transformation vs flow-control), AnyNode restructure (struct with inner NodeKind enum), trait objects retained (vtable irrelevant vs Wasm cost), Source/Sink remain open traits, lifecycle states (+Recovering), instance type unification (separate bindgen modules with `with:` sharing, WasmBindings enum), native async fn in traits (RPITIT), Filter/Router borrow-only signatures, Transform strict 1:1, Merge as host-native topology, fuel budget differentiation, DLQ pre-clone elimination for borrow-only nodes, router fan-out last-port-move optimization.

**Amendments to previous sessions:**
- **Session 1 (A1):** Removed Joiner WIT world entirely — Merge is host-native topology, Join is out of scope (stateful).
- **Session 1 (A2):** Transform return simplified from `result<process-outcome, process-error>` to `result<output-message, process-error>` — strict 1:1, no variant wrapper.
- **Session 2 (A3):** RuntimeEnvelope redesigned: `{ header: Arc<EnvelopeHeader>, payload: Bytes, lineage: Lineage }` — makes clone near-free for borrow-only nodes (Arc refcount vs string copies).

---

## Session 4: Config File Schema & Pipeline UX ✅ DONE

**Status:** Complete (2025-07-06)  
**Output:** `docs/decisions/2025-07-06-config-schema-pipeline-ux.md`  
**Decisions made:** 14

Covered: Map-keyed node schema (`[nodes.NAME]` with serde tag dispatch), single `plugin` field (auto-detect local vs OCI), implicit merge topology (validated at build time — only Transform/Sink may have multiple inputs), error policy cascade (`[error_policy]` pipeline defaults + per-node `[nodes.X.error_policy]` override), fuel+epoch in `[engine]` with per-type defaults and per-node override, edge simplification (removed `to_port`, renamed `from_port` to `port`), DagConfig removal (single `Config` type), top-level structure (7 sections + nodes + edges, api/metrics kept with env var overrides), capabilities as nested `[nodes.X.capabilities]` table (extensible), two-phase validation (serde + accumulated semantic errors), aggressive defaults (7-line minimal config), source/sink kind enum + flat fields + sub-tables for TLS/auth, DLQ reuses sink kind pattern, plugin config as TOML table serialized to JSON at load.

**Key schema changes from current:**
- Removed: `DagConfig`, `NodeType::Joiner`, `to_port`, `source_type`/`sink_type` strings, `NodeConfig` internal struct, `config: toml::Value` untyped bag, `swappable` field
- Added: `NodeType::Filter`, `[error_policy]` section, `[engine.fuel]` per-type, `[nodes.X.capabilities]` table, `[nodes.X.config]` TOML table (→ JSON), `[nodes.X.error_policy]` override, per-node `fuel` field
- Changed: `[[nodes]]` array → `[nodes.NAME]` map, `plugin_path`/`oci` → single `plugin`, `from_port` → `port`

---

## Session 5: Orchestrator & Runtime Simplification ✅ DONE

**Status:** Complete (2025-07-12)  
**Output:** `docs/decisions/2025-07-12-orchestrator-runtime-simplification.md`  
**Decisions made:** 13

Covered: Task-per-node model confirmed (no central scheduler), builder redesign (map iteration, receiver-keyed queue wiring, multi-sender merge), three per-type runner loops (transform/filter/router), ErrorPolicyExecutor as owned struct (retry buffer + backoff + DLQ dispatch), InstancePre per-node for recovery (~5µs re-instantiation), hot-swap via watch channel (ownership transfer, zero-mutex hot path), Recovering state machine (re-instantiate on unrecoverable), DLQ envelope format (structured with full context for message accounting), config diff (plugin vs config-only warm swap), unconditional metrics (always-on atomics, feature-gate only exposition), graceful shutdown sequence (ordered: sources stop → drain → retry flush → DLQ → close), orchestrator role (setup + watch + teardown only), evaluation baseline stack (3-layer decomposition: single-flow / native-with-channels / WAFER).

**Key architectural changes:**
- Removed: `Mutex<HashMap<String, Arc<Mutex<AnyNode>>>>` (double-lock), `RunState` mutex wrapper, `RoutingController`, `run_joiner_loop`, `DagConfig`, `ProcessResult::Filter` in transform, `NodeAssembler` behind Mutex
- Added: `ErrorPolicyExecutor` struct, `RetryBuffer` (bounded VecDeque), `DlqEnvelope` struct, `watch::channel` per Wasm node, `NodeMetrics` (unconditional atomics), `Recovering` state + re-instantiation, config diff (plugin vs config changes), ordered shutdown with retry flush
- Amended: ADR-0003 drain phase simplified (watch between messages = no separate drain), `RoutingController` removed (no buffering needed)

**Cross-session impacts:**
- Session 2 D4 (error policy): runtime `ErrorPolicyExecutor` now specified (was "implementation detail")
- ADR-0003: Drain phase simplified — no explicit drain, swap happens between messages naturally
- ADR-0002: Confirmed mpsc for merge (multi-sender, single receiver)

**Context for future sessions:**
- Session 6: Plugins must target 3 worlds (transform-node, filter-node, router-node). No joiner plugin.
- Session 7: BoundedQueue wrapper can be removed (direct tokio::mpsc). InstancePre cache upgrading to shared HashMap is trivial. Filter chain fusion is possible since swap is per-node.
- Session 8: Unconditional `NodeMetrics` + `swap_count` enable all 15 experiments without feature flags. 3-layer baseline stack (single-flow / native-with-channels / WAFER) decomposes overhead.

---

## Session 6: Plugin Rewrite & Guest SDK ✅ DONE

**Status:** Complete (2025-07-12)  
**Output:** `docs/decisions/2025-07-12-plugin-rewrite-guest-sdk.md`  
**Decisions made:** 17

Covered: Plugin inventory (12 Rust + 2 polyglot + 6 attacks = 20 total), state storage pattern (RefCell via thread_local!, no unsafe in user code), guest SDK design (wafer-plugin crate: output_from, error helpers, state macros, parse_config, no proc macros), workspace structure (separate plugins/ workspace with .cargo/config.toml for wasm32-wasip2, polyglot in go/ and python/ subdirs), build tooling (Makefile + wasm-opt post-processing, no cargo-component needed), testing pyramid (L1 unit native, L2 PluginTestHarness, L3 TestPipeline E2E), content router (reads payload JSON, configurable route field), naming convention (wafer-{descriptive-name}), attack plugins (interface now, implement during eval), transform DX (output_from(&input, payload)), filter DX (explicit evaluate() impl), error categorization DX (constructor functions with decision-guide docs), wasi-nn integration (inference-node world, load_by_name for hot-swap), polyglot plugins (Go TinyGo + Python componentize-py), complex plugins (all 4: Cayenne decoder, EWMA anomaly, vibration FFT, quality rules), SDK scope (internal path dep, not for crates.io), binary size strategy (simple plugins no-serde ~5KB, complex with serde ~150-300KB, wasm-opt -Os).

**Key narrative decisions:**
- Simple telemetry plugins exist for eKuiper comparison (Pipeline A baseline)
- Complex plugins (EWMA, vibration, Cayenne, quality-rules) justify the architecture — they demonstrate WHY you need Wasm isolation + hot-swap
- Polyglot plugins (Go + Python) prove Component Model's language-agnostic composition
- Binary size: 5KB-300KB vs 50-200MB containers = 10,000× smaller per-stage isolation
- Build priority: SDK → simple plugins → harness → complex plugins → polyglot → attacks

**Context for future sessions:**
- Session 7: Plugin binary sizes affect AOT cache strategy. Simple plugins (~5KB) compile in <1ms. Complex plugins (~300KB) may take longer.
- Session 8: E2E TestPipeline exercises the full runtime loops. Attack plugins map directly to E-Iso-1 through E-Iso-6. PluginTestHarness provides the measurement boundary for per-hop overhead (RQ1a).

---

## Session 7: Performance Optimizations (Scope & Priority) ✅ DONE

**Status:** Complete (2025-07-12)  
**Output:** `docs/decisions/2025-07-12-performance-optimizations.md`  
**Decisions made:** 10 primary + 4 code quality (C1-C4) + 3 amendments

Covered: AOT compilation cache (blake3+platform keyed .cwasm, two-tier memory+disk, ~150 lines), pooling allocator (skip — persistent Store makes it irrelevant), BoundedQueue removal (already implied by Session 5), filter chain fusion (skip — contradicts per-node isolation contribution), host-native expression filters (skip — undermines Wasm viability argument), fuel+epoch independent boolean flags (4 measurement configs for thesis decomposition), Sender::reserve (skip — no cancel hazard on send path), benchmark-first strategy (local Phase A → RPi Phase B → Jetson Phase C), StoreLimits per-node (required for RQ2 S4), parallel compilation (JoinSet at startup).

**Code quality decisions:**
- C1: Epoch ticker → OS thread (correctness bug fix — tokio saturation prevents epoch firing)
- C2: Box<str> for immutable EnvelopeHeader fields (communicates immutability)
- C3: foldhash for internal HashMaps (30% faster than SipHash on trusted keys)
- C4: RAII guard for NodeState::Processing (prevents forgotten cleanup on early return)

**Amendments to previous sessions:**
- Session 2: Add `limits: StoreLimits` to WaferState
- Session 4: Add `fuel = true/false` and `epoch = true/false` to `[engine]`
- Session 5: Epoch ticker implementation → std::thread::spawn (not tokio::spawn)

**Key thesis-relevant outcomes:**
- 4 metering configurations enable stacked overhead decomposition figure
- AOT cache makes hot-swap prepare ~2ms (vs ~30ms on RPi 4) — improves RQ3 numbers
- StoreLimits + fuel + epoch + OS-thread ticker = complete RQ2 attack containment story
- Documented future wasmtime roadmap (MMU-based epochs → ~0% overhead) for thesis defense
- Total "do now" effort: ~10-13 hours
- Explicitly deferred: pooling allocator, filter fusion, host-native filters, buffer pools, Sender::reserve

**Context for Session 8:**
- Benchmarks have 4 fuel/epoch configs (A: both, B: fuel-only, C: epoch-only, D: neither)
- AOT cache means benchmark can distinguish cold-start vs warm-start
- StoreLimits configured per-type (Transform 64MB, Filter/Router 16MB)
- Local benchmarks first (your machine), then RPi 4, then Jetson
- Epoch ticker guaranteed to fire under saturation (OS thread)

### Context from Sessions 3 & 5 (SUPERSEDES some Session 2 questions)
- **Envelope optimization: RESOLVED.** `Arc<EnvelopeHeader>` wraps all immutable fields. Clone is near-free (refcount bumps). No need for `Arc<str>` on individual fields.
- **Clone-on-error: RESOLVED.** Transform DLQ copy = Arc bump (~10ns). Filter/Router = no clone at all (borrow). No further optimization needed.
- **Fuel differentiation**: Transform 10M, Filter/Router 500K (Session 3 D10). Per-node overridable.
- **Zero-copy paths proven**: Filter pass-through = 0 allocations. Router fan-out = N refcount bumps. Transform = 2 copies (unavoidable Wasm boundary).
- **Merge = zero cost**: No Wasm, no task, just mpsc multi-sender topology.
- **BoundedQueue wrapper removable**: Session 5 confirmed direct tokio::mpsc is sufficient.
- **InstancePre cache**: Per-node Arc<InstancePre> is baseline. Shared cache (HashMap keyed by SHA-256) is trivial upgrade for Session 7 if startup time matters.
- **Unconditional metrics**: Always-on atomics mean benchmarks work without feature flags.
- Remaining performance unknowns: ResourceTable push/delete cost (~50ns — measure on RPi4), Canonical ABI lift cost for `list<u8>` output, actual fuel consumption for real plugins.

### Context from Session 2
- Bytes payload eliminates the biggest allocation hotspot (Vec<u8> clone per queue send).
- InstancePre eliminates hot-swap instantiation bottleneck (8.85ms → ~5µs).
- Remaining performance unknowns: ResourceTable push/delete cost (~50ns — measure on RPi4), Canonical ABI lift cost for `list<u8>` output, metadata string clone cost in filter forwarding.
- wasmtime call overhead for typed nop: ~27ns (PR #10643). This is the floor for RQ1a — actual overhead will add ResourceTable ops + Canonical ABI + payload copy.

### Documents to Explore

**This repo:**
- `crates/wafer-core/src/queue/envelope.rs` — Current alloc patterns
- `crates/wafer-core/src/queue/bounded.rs` — BoundedQueue wrapper
- `crates/wafer-core/src/engine/loader.rs` — Current compilation (no cache)
- `crates/wafer-core/benches/{throughput.rs,hot_swap.rs,metrics.rs}`
- `docs/benchmarks/hot-swap.md` — Existing measurements (~9ms prepare on Apple Silicon)

**tcc-doc:**
- `research/analysis/evaluation-plan.md` — Full methodology: N=30, open-loop, HdrHistogram
- `research/analysis/thesis-statement-v3.md` §RQ1 (all sub-questions with pass criteria)
- `findings/source-code-comparators.md` §1.5 (Torvyn Treiber stack buffer pool)
- `findings/source-code-comparators.md` §1.9 (Torvyn AOT cache: SHA-256 keyed)
- `findings/source-code-comparators.md` §1.11 (Torvyn performance: 410ns/element)
- `findings/source-code-comparators.md` §2.4 (Flow-Like AOT: blake3 + platform key + inject)
- `findings/synthesis-findings.md` §2 (quantitative evidence: all overhead numbers from literature)

**Obsidian vault:**
- `TCC/systems/lmaxDisruptorHighPerformance2011.md` — Ring buffer: 52ns/hop, cache-line design
- `TCC/systems/wasmComponentModelZeroCopy2024.md` — Copy overhead at Canonical ABI level
- `TCC/papers/kakatiCrossArchitectureEvaluationWebAssembly2024.md` — Cross-arch Wasm perf (x86/ARM/RISC-V)
- `TCC/papers/Gadepalli2020.md` — Sledge: 13.4% x86, 6.74% ARM overhead baseline
- `TCC/papers/Lyu2022.md` — 10-sandbox chain = 6% overhead
- `TCC/papers/lumosPerformanceWasm2025.md` — Wasm performance measurement methodology
- `TCC/papers/performanceContainersEdge2025.md` — Container vs Wasm on edge performance
- `TCC/papers/wagnerEnergyConsumptionPerformance2023.md` — Energy + performance on RPi
- `TCC/systems/teneHowNotMeasureLatency2012.md` — Gil Tene: coordinated omission, measurement methodology
- `TCC/software/torvyn2026.md` §Performance — Mock invoker numbers, copy accounting
- `TCC/papers/marcelinoRoadrunnerAcceleratingData2025.md` — Roadrunner: 69× with zero-copy ring buffer
- `TCC/papers/ahnPerformanceCharacterizationUsing2023.md` — Wasm performance characterization

**Skills to load:**
- `rust-best-practices` — Zero-alloc hot path, SmallVec, Arc<str>, Cow, buffer reuse
- `async-tokio` — Cancel-safe sends, Sender::reserve, backpressure patterns
- `wasm-specialist` — Store lifecycle, pre-compilation, fuel/epoch metering

---

## Session 8: Evaluation Harness Design ✅ DONE

**Status:** Complete (2025-07-12)  
**Output:** `docs/decisions/2025-07-12-evaluation-harness-design.md`  
**Decisions made:** 15 + 6 additional experiments

Covered: Per-hop overhead measurement boundary (includes fuel/epoch reset + Wasm call, excludes channel ops), measurement instrumentation (unconditional inline Instant::now() + optional HdrHistogram), open-loop load generator (dual: in-process BenchSource + external wafer-loadgen binary), HdrHistogram integration (sink-side recording, 3 sig digits, 1µs–10s range, SequenceTracker for gap/duplicate detection), native Rust baseline (same crate, ProcessNode trait, same orchestrator/channels), eKuiper comparison setup (same broker, same loadgen, same measurement methodology), hot-swap measurement (version markers + SwapTimeline phase decomposition + throughput time-series), attack scenario measurement (TestPipeline pass/fail + parallel-branch topology for E-Iso-7), statistical analysis (UV-managed Python notebooks, scipy Mann-Whitney + Bootstrap CI + Cliff's Delta), reproducibility artifacts (eval/ directory, Makefile automation, Zenodo-style raw data publishing), warmup detection (30s exclusion + ADF stationarity verification), throughput saturation (ramp profile + 2× p99 threshold definition), cross-architecture validation (same binary, ratio reporting), memory measurement (/proc/self/statm at 1Hz, incremental per-node attribution), shared pipeline builder (pluggable I/O adapters: MemorySource/BenchSource/MqttSource × CollectorSink/BenchSink/NullSink).

**Additional experiments added (beyond original 16):**
- E-Perf-7: Metering overhead decomposition (4 fuel/epoch configs)
- E-Perf-8: Pipeline depth scaling (1→10 hops, latency vs depth)
- E-Perf-9: AOT cache cold-start vs warm-start
- E-Iso-8: Recovery time (trap → Recovering → Running)
- E-Density-1: Binary size comparison (Wasm vs containers)
- E-Backpressure: Queue depth time-series during burst

**Total thesis deliverables:** 22 experiments, 15 figures, 7 tables.

**Amendments to previous sessions:**
- Session 5: ProcessNode trait abstraction for native baseline support
- Session 6: TestPipeline generalized to shared PipelineBuilder with pluggable I/O
- Session 7 D8: Benchmark harness fully specified (was "4-8 hours" estimate, now detailed ~28 hours)

**Implementation priority:** BenchSource+BenchSink → wafer-loadgen → native baseline → SwapTimeline → Python notebooks → eval/ automation → MemoryRecorder → eKuiper config

**Key architectural patterns adopted:**
- Tremor's bench connector pattern (measurement as pipeline component)
- lightbench's ProducerConsumer model (rate control + sequence tracking)
- Zenodo replication dataset pattern (Rust records → CSV/HDR → Python analysis)
- Tinto 2025 hardware controls (pinned freq, affinity, SCHED_FIFO)

---

## Session Order & Dependencies

```
Session 1: WIT Contracts ─────────────── ✅ DONE (2025-07-05)
                                         │
Session 2: Host Runtime Architecture ─── ✅ DONE (2025-07-06)
                                         │
Session 3: Node Architecture ──────────── ✅ DONE (2025-07-06) [amends S1 + S2]
                                         │
Session 4: Config Schema ────────────────── ✅ DONE (2025-07-06)
                                         │
Session 5: Orchestrator & Hot-Swap ──────── ✅ DONE (2025-07-12) [amends ADR-0003]
                                         │
Session 6: Plugin Rewrite & Guest SDK ───── ✅ DONE (2025-07-12)
                                         │
Session 7: Performance Optimizations ────── ✅ DONE (2025-07-12) [amends S2 + S4 + S5]
                                         │
Session 8: Evaluation Harness Design ────── ✅ DONE (2025-07-12) [amends S5 + S6 + S7]
```

**ALL 8 DISCUSSION SESSIONS COMPLETE.**  
Phase 0 design is finished. Implementation can begin.

---

## Implementation Phase

With all architectural decisions locked across 8 sessions, implementation proceeds with full awareness of the complete design. The decision documents serve as the specification:

| Document | Scope |
|----------|-------|
| `2025-07-05-wit-contracts-envelope-design.md` | WIT packages, worlds, type definitions |
| `2025-07-06-host-runtime-architecture.md` | WaferState, ResourceTable, Store lifecycle |
| `2025-07-06-node-type-architecture.md` | 4 node types, trait hierarchy, copy semantics |
| `2025-07-06-config-schema-pipeline-ux.md` | TOML schema, validation, defaults |
| `2025-07-12-orchestrator-runtime-simplification.md` | Builder, runner loops, hot-swap, shutdown |
| `2025-07-12-plugin-rewrite-guest-sdk.md` | 20 plugins, SDK, workspace, testing pyramid |
| `2025-07-12-performance-optimizations.md` | AOT cache, metering, StoreLimits, code quality |
| `2025-07-12-evaluation-harness-design.md` | Measurement infra, load gen, analysis, reproducibility |
