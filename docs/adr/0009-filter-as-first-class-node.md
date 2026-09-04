# ADR-0009: Filter as First-Class Node Type

- **Date**: 2026-07-06
- **Status**: Accepted
- **Parent RFC**: [RFC-003](../rfcs/RFC-003-node-types.md)

## Context

Prior to Session 3 (RFC-003), WAFER had only Transform as a Wasm processing node. A transform that wanted to drop messages returned a "filter" variant from `process-outcome`, which meant every such node still took ownership of the envelope, allocated an output message, and then communicated "discard" through a return-type variant. This design conflated two semantically different operations — data transformation (produces new bytes) and predicate evaluation (inspects existing bytes, produces a boolean verdict). The host runtime could not distinguish the two, so it always paid the full ownership-transfer cost regardless of what the plugin actually needed.

The WIT contract in RFC-001 originally included `filter-node` as a separate world, but the host side lacked a distinct processing path. RFC-003 closed that gap by establishing Filter as a first-class node type with a dedicated borrow-only trait signature, an independent WIT interface, its own fuel budget tier, and an optimised host runner loop that forwards or drops with zero allocation.

## Decision

We introduce Filter as a distinct, first-class Wasm node type — separate from Transform — with a **borrow-only evaluation signature** (`evaluate(&self, input: &RuntimeEnvelope) -> Result<FilterOutcome>`) that returns a boolean verdict rather than an output message.

The WIT interface (`pipeline:node/filter@0.1.0`) exposes `evaluate: func(input: message) -> result<bool, process-error>`. On the host side the filter runner loop lends the envelope immutably to the Wasm guest via `borrow<buffer>` and, on a `true` verdict, forwards the **original** `RuntimeEnvelope` to the downstream queue by cloning only `Arc<EnvelopeHeader>` and `Bytes` refcounts (~10 ns total). On a `false` verdict the message is dropped in place. The guest never receives ownership; it never allocates output bytes.

## Consequences

- **Positive — zero-copy forwarding.** A filter pass-through costs two refcount bumps (`Arc` + `Bytes`) ≈ 10 ns instead of the ~300 B allocation that a Transform would incur. In a pipeline with multiple filters upstream of a transform, this eliminates per-message allocations proportional to filter count.

- **Positive: separate fuel policy.** Filter nodes have an independent fuel category. Runtime fuel defaults are `None`; the final protected evaluation config assigns 500,000 fuel units to Filter calls and 10,000,000 to Transform calls. This permits a tighter filter limit without coupling it to Transform work.

- **Positive — no DLQ pre-clone.** The host filter loop does not pre-clone the envelope before calling the guest (unlike Transform, which must clone for DLQ safety before yielding ownership). If the guest traps, the original envelope is still available in the host for error-policy handling. This saves one `Arc` bump per invocation in the happy path.

- **Positive — clear pipeline composition semantics.** Pipeline authors place Filter nodes upstream of Transform or Sink nodes to validate or gate messages. Transform is strict 1:1 — it cannot "skip" — so the filter-then-transform pattern is the only way to discard. This makes data-flow reasoning local: a Transform always emits, a Filter may drop, a Router may fan-out.

- **Positive — independent hot-swap.** Because filter and transform are separate `bindgen!` worlds (`filter_world`, `transform_world` in `crates/wafer-core/src/engine/bindings.rs`), each filter node has its own `InstancePre` and can be hot-swapped independently without affecting sibling transforms in the same pipeline.

- **Negative — two runner-loop implementations.** The host must maintain separate runner loops for Transform and Filter (and Router). This adds implementation surface (~100 extra lines per loop) compared to a single polymorphic loop. The trade-off is acceptable because the loops are small and the performance difference is measurable.

- **Negative — plugin author choice burden.** A plugin author must decide up-front whether their node is a Filter or a Transform. If requirements evolve (e.g., a filter that also enriches), the plugin must be rewritten as a Transform and the pipeline config updated. This is mitigated by clear documentation: if you produce new bytes, you are a Transform; if you produce a boolean, you are a Filter.

- **Forecloses — "filter-and-transform" combo nodes.** A single Wasm component cannot both filter and transform in one call. The pipeline must use two nodes. This is intentional: it preserves the zero-copy guarantee for filters and keeps trait signatures unambiguous.

- **Downstream requirement — router interplay.** When a Router fans out to multiple ports and one branch includes a Filter, the filter receives the same `Arc<EnvelopeHeader>` + `Bytes` envelope that the router forwarded via refcount bump. The filter's borrow-only contract means no allocation occurs until a downstream Transform takes ownership. This makes router→filter→transform chains allocate only once (at the transform).

## See Also

- [RFC-003](../rfcs/RFC-003-node-types.md) — the long-form decision establishing the five node categories, borrow-vs-ownership trait split, and fuel differentiation.
- [RFC-001](../rfcs/RFC-001-wit-contracts.md) — original WIT contracts defining the `filter` interface and `filter-node` world.
- [RFC-002](../rfcs/RFC-002-host-runtime.md) — host runtime architecture, including the RuntimeEnvelope design (amended by RFC-003 §A3 to use `Arc<EnvelopeHeader>` + `Bytes`).
- `wit/pipeline-node.wit` — the `filter` interface and `filter-node` world definition.
- `crates/wafer-core/src/engine/bindings.rs` — `filter_world` bindgen module and `WasmBindings::Filter` variant.
