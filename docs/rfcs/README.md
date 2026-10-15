# RFCs

Long-form design decisions for WAFER. RFCs are self-contained records with an
Abstract, Decision, Alternatives Considered, Related RFCs, and Implementation
Notes. They complement the short Nygard-format ADRs under [`../adr/`](../adr/).
RFCs are point-in-time records: a later change appears as a banner at the top
of the RFC, and the design text below it is kept. Current behaviour is defined
by the code, the WIT files under `wit/`, and the
[architecture views](../architecture/README.md).

## Index

| RFC | Title | Status | Session date |
|-----|-------|--------|--------------|
| [RFC-001](RFC-001-wit-contracts.md) | WIT Contracts & Envelope Design | Implemented | 2026-07-05 |
| [RFC-002](RFC-002-host-runtime.md) | Host-Side Runtime Architecture | Implemented | 2026-07-06 |
| [RFC-003](RFC-003-node-types.md) | Node Type Architecture | Implemented | 2026-07-06 |
| [RFC-004](RFC-004-config-schema.md) | Config File Schema & Pipeline UX | Implemented | 2026-07-06 |
| [RFC-005](RFC-005-orchestrator.md) | Orchestrator & Runtime Simplification | Implemented | 2026-07-12 |
| [RFC-006](RFC-006-plugin-sdk.md) | Plugin Rewrite & Guest SDK | Implemented | 2026-07-12 |
| [RFC-007](RFC-007-performance-optimizations.md) | Performance Optimizations | Partially implemented | 2026-07-12 |
| [RFC-008](RFC-008-evaluation-harness.md) | Evaluation harness design | Implemented | 2026-07-12 |
| [RFC-009](RFC-009-implementation-architecture.md) | Implementation Architecture — Module Structure & Crate Boundaries | Implemented | 2026-07-12 |
| [RFC-010](RFC-010-io-integration.md) | I/O Integration & First End-to-End Pipeline | Implemented | 2026-07-15 |
| [RFC-012](RFC-012-wasi-0.3-evaluation.md) | WASI 0.3 and Component Model Evolution | Async P2 and bounded outbound HTTP implemented; P3 PoC not started | 2026-09-25 |

Titles, statuses, and dates are copied from each RFC's own header; the RFC's
Status line carries the detail. Amendment relations are listed below.

## Amendments summary

Amendments are places where a later RFC revised an earlier one. Not every RFC
header records them; this table is the complete list.

| Amender | Target | Nature of amendment |
|---------|--------|---------------------|
| RFC-003 §A1 | RFC-001 | Joiner removed; fan-in becomes implicit host topology. |
| RFC-003 §A2 | RFC-001 | Transform return simplified from `result<process-outcome, process-error>` to `result<output-message, process-error>` — strict 1:1, no variant wrapper. |
| RFC-003 §A3 | RFC-002 | `RuntimeEnvelope` redesigned to `{ header: Arc<EnvelopeHeader>, payload: Bytes, lineage: Lineage }` so clone is near-free (refcount bumps) for borrow-only nodes. |
| RFC-005 §D6 | RFC-002, [ADR-0003](../adr/0003-hot-swap-mechanism.md) | Drain-and-flip 4-phase hot-swap replaced with a watch-channel between-messages model (`watch::Sender<Option<SwapPayload>>` per transform, filter and router node). Each runner checks the watch slot between messages and applies the payload before dequeuing the next message; the swap signal also wakes an idle runner. |
| RFC-007 D9 | RFC-002 | Adds `limits: StoreLimits` to `WaferState` for per-node memory containment (RQ2 scenario S4). |
| RFC-007 D6 | RFC-004 | Adds per-category `[engine.fuel]` budgets and makes fuel and epoch metering independently switchable for the four-way metering-overhead decomposition; each is off when its values are omitted (the proposed boolean toggles were not implemented). |
| RFC-007 C1 | RFC-005 | Epoch ticker moves from `tokio::spawn` to a dedicated `std::thread::spawn` OS thread, which keeps firing under runtime saturation. |
| RFC-008 D5 | RFC-005 | `ProcessNode` trait abstraction generalised so a native Rust baseline can share the orchestrator/channels code paths. |
| RFC-008 D15 | RFC-006 | Proposed generalising `TestPipeline` into a shared `PipelineBuilder` with pluggable I/O adapters. Neither was built; tests use `PluginTestHarness` and the in-memory `ChannelSource` / `ChannelSink`. |

## Reading order for new contributors

1. **RFC-001, RFC-002, RFC-003**: the WIT contracts, host runtime, and node
   types together define the shape of a WAFER pipeline. Read RFC-001 first,
   then RFC-002, then RFC-003 (which amends both).
2. **RFC-004**: the TOML config schema that operators actually write.
3. **RFC-005**: how the orchestrator and hot-swap actually run.
4. **RFC-006**: the guest-side plugin SDK and the plugin inventory.
5. **RFC-007**: performance optimisations layered on top of the above.
6. **RFC-008**: how we measure everything for the thesis evaluation.
7. **RFC-009**: the implementation architecture that bundles the above into
   the shipped crates.
8. **RFC-010**: the I/O integration (MQTT / HTTP / file) that made
   the runtime end-to-end usable.
9. **RFC-012**: the Component Model direction: adopted async P2 host bindings,
   bounded outbound HTTP, and a possible P3 streaming PoC that has not been
   started.

For short, executive summaries of individual decisions, see the Nygard-format
ADRs under [`../adr/`](../adr/).
