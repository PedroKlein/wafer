# RFCs

Long-form design decisions for WAFER. RFCs are self-contained records with an
Abstract, Decision, Alternatives Considered, Related RFCs, and Implementation
Notes. They complement the short Nygard-format ADRs under [`../adr/`](../adr/).

The archive was migrated from `docs/decisions/` on 2026-07-18; each RFC
preserves its original decision-session date in the header block but adopts a
stable `RFC-NNN-<slug>` filename. Bit-for-bit copies of the original deleted
source documents are preserved under [`source-decisions/`](source-decisions/)
for provenance.

## Index

| RFC | Title | Status | Session date |
|-----|-------|--------|--------------|
| [RFC-001](RFC-001-wit-contracts.md) | WIT contracts & envelope design | Implemented (amended by RFC-002, RFC-003) | 2026-07-05 |
| [RFC-002](RFC-002-host-runtime.md) | Host-side runtime architecture | Implemented (amended by RFC-003, RFC-005) | 2026-07-06 |
| [RFC-003](RFC-003-node-types.md) | Node type architecture | Implemented (amends RFC-001, RFC-002) | 2026-07-06 |
| [RFC-004](RFC-004-config-schema.md) | Config schema & pipeline UX | Implemented | 2026-07-06 |
| [RFC-005](RFC-005-orchestrator.md) | Orchestrator & runtime simplification | Implemented (amends ADR-0003) | 2026-07-12 |
| [RFC-006](RFC-006-plugin-sdk.md) | Plugin rewrite & guest SDK | Implemented | 2026-07-12 |
| [RFC-007](RFC-007-performance-optimizations.md) | Performance optimizations | Implemented (amends RFC-002, RFC-004, RFC-005) | 2026-07-12 |
| [RFC-008](RFC-008-evaluation-harness.md) | Evaluation harness design | Accepted (implementation in progress) | 2026-07-12 |
| [RFC-009](RFC-009-implementation-architecture.md) | Implementation architecture | Implemented | 2026-07-12 |
| [RFC-010](RFC-010-io-integration.md) | Phase 4 — I/O integration (MQTT, HTTP, file) | Implemented | 2026-07-15 |
| [RFC-011](RFC-011-doc-refactor.md) | Documentation refactor execution plan | Superseded (retrospective archive) | 2026-07-18 |

## Amendments summary

Amendments are cross-links between RFCs where a later decision revised an
earlier one. The pattern is: the amending RFC lists the target on its
`Amends:` header line, and the amended RFC carries an `Amended by:` banner
that points back.

| Amender | Target | Nature of amendment |
|---------|--------|---------------------|
| RFC-003 §A1 | RFC-001 | Joiner WIT world removed; fan-in becomes implicit host topology (multi-producer mpsc). Three worlds remain: `transform-node`, `filter-node`, `router-node` (plus `inference-node` for wasi-nn plugins). |
| RFC-003 §A2 | RFC-001 | Transform return simplified from `result<process-outcome, process-error>` to `result<output-message, process-error>` — strict 1:1, no variant wrapper. |
| RFC-003 §A3 | RFC-002 | `RuntimeEnvelope` redesigned to `{ header: Arc<EnvelopeHeader>, payload: Bytes, lineage: Lineage }` so clone is near-free (refcount bumps) for borrow-only nodes. |
| RFC-005 §D6 | RFC-002, [ADR-0003](../adr/0003-hot-swap-mechanism.md) | Drain-and-flip 4-phase hot-swap replaced with a watch-channel between-messages model (`watch::Sender<Option<SwapPayload>>` per Wasm node). Each runner polls `swap_rx.has_changed()` between messages and applies the payload before the next `select!` iteration. |
| RFC-007 §W2 | RFC-002 | Adds `limits: StoreLimits` to `WaferState` for per-node memory containment (RQ2 scenario S4). |
| RFC-007 §W4 | RFC-004 | Adds `fuel = true/false` and `epoch = true/false` boolean toggles to `[engine]` for the four-way metering-overhead decomposition. |
| RFC-007 §W5 | RFC-005 | Epoch ticker moves from `tokio::spawn` to a dedicated `std::thread::spawn` OS thread — guarantees firing under runtime saturation. |
| RFC-008 §W1 | RFC-005 | `ProcessNode` trait abstraction generalised so a native Rust baseline can share the orchestrator/channels code paths. |
| RFC-008 §W2 | RFC-006 | `TestPipeline` generalised into a shared `PipelineBuilder` with pluggable I/O adapters (`MemorySource` / `BenchSource` / `MqttSource` × `CollectorSink` / `BenchSink` / `NullSink`). |
| RFC-008 §W3 | RFC-007 | Benchmark harness scope expanded — the "4–8 hours" estimate becomes a fully specified ~28-hour harness with HdrHistogram + `SwapTimeline` + Python analysis notebooks. |

## Reading order for new contributors

1. **RFC-001, RFC-002, RFC-003** — the WIT contracts, host runtime, and node
   types together define the shape of a WAFER pipeline. Read RFC-001 first,
   then RFC-002, then RFC-003 (which amends both).
2. **RFC-004** — the TOML config schema that operators actually write.
3. **RFC-005** — how the orchestrator and hot-swap actually run.
4. **RFC-006** — the guest-side plugin SDK and the plugin inventory.
5. **RFC-007** — performance optimisations layered on top of the above.
6. **RFC-008** — how we measure everything for the thesis evaluation.
7. **RFC-009** — the implementation architecture that bundles the above into
   the shipped crates.
8. **RFC-010** — the Phase-4 I/O integration (MQTT / HTTP / file) that made
   the runtime end-to-end usable.

For short, executive summaries of individual decisions, see the Nygard-format
ADRs under [`../adr/`](../adr/).
