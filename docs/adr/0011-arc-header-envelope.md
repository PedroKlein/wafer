# ADR-0011: Arc\<EnvelopeHeader\> + Bytes Payload + Lineage Runtime Envelope

- **Date**: 2026-07-06
- **Status**: Accepted
- **Parent RFC**: [RFC-003](../rfcs/RFC-003-node-types.md) §A3

## Context

Pipeline messages flow through bounded queues between nodes. Before this decision, `RuntimeEnvelope` was a flat struct where every field (id, timestamp, source, content-type, payload) was an owned allocation. Cloning a message — required for Filter pass-through, Router fan-out, and DLQ safety copies — allocated roughly 200–400 bytes of heap memory per clone. In a five-node pipeline processing tens of thousands of messages per second on a Raspberry Pi 4, these allocations dominated per-hop latency and garbage-collector-like allocation pressure in the global allocator.

The design constraint was: Filter and Router nodes borrow the envelope (they inspect but do not mutate it), yet the host must still be able to forward the message to one or more downstream queues after the Wasm call returns. The host therefore clones the envelope for each downstream edge. If cloning is expensive, the borrow-only signature buys nothing in practice — the allocation cost just moves from inside the Wasm call to outside it.

## Decision

We split `RuntimeEnvelope` into three fields with distinct sharing semantics:

```rust
pub struct RuntimeEnvelope {
    pub header: Arc<EnvelopeHeader>,   // immutable message identity, shared via refcount
    pub payload: Bytes,                // refcounted byte buffer, zero-copy from network sources
    pub(crate) lineage: Lineage,       // mutable per-hop tracking (parent_id, trace_id)
}
```

`EnvelopeHeader` holds immutable identity metadata (`id`, `timestamp`, `source`, `content_type`, `metadata` key-value pairs) behind an `Arc`. `payload` is a `bytes::Bytes` handle — refcounted and zero-copy from MQTT/network ingestion through the entire pipeline. `Lineage` is the only field that is deep-copied on clone; it contains two `Option<Box<str>>` fields (~32 bytes worst-case).

Clone cost: one `Arc` increment (~5 ns) + one `Bytes` increment (~5 ns) + `Lineage` copy (~32 bytes shallow) ≈ **~10 ns total**, down from ~300–400 ns with full-clone semantics.

The implementation lives in `crates/wafer-core/src/queue/envelope.rs`. `EnvelopeHeader` fields use `Box<str>` instead of `String` to eliminate the 8-byte capacity field per string — a space optimization meaningful when millions of envelopes are simultaneously in-flight on memory-constrained edge hardware.

## Consequences

- **Positive (RQ1 performance):** Near-free envelope clone is what makes Filter and Router borrow-only signatures actually cheap in practice. Without this, the host-side clone cost after each borrow-only Wasm invocation would dominate per-hop latency and undermine the RQ1 pass criterion of <50 µs per-hop on RPi 4. With ~10 ns clone cost, the envelope forwarding overhead is negligible compared to the Wasm call itself (~15–35 µs).

- **Positive (fan-out efficiency):** Router fan-out to N downstream ports costs N−1 clones at ~10 ns each plus one move. A 4-way router adds ~30 ns of envelope overhead — invisible next to the Wasm boundary crossing.

- **Positive (DLQ safety):** Transform nodes pre-clone the envelope before passing ownership into the Wasm call. If the call traps, the pre-clone is sent to the dead-letter queue. The Arc+Bytes design makes this safety copy virtually free (~10 ns), so enabling DLQ preservation imposes no measurable throughput penalty.

- **Positive (zero-copy network ingestion):** MQTT source nodes receive payloads as `Bytes` directly from the network stack (rumqttc provides `bytes::Bytes`). The payload flows through the entire pipeline without a single memcpy until a Transform node explicitly deserializes it inside Wasm.

- **Positive (allocation pressure):** In a 5-node pipeline, per-message allocations drop from ~1500 bytes (5 full clones) to ~64 bytes (lineage copies only). On the RPi 4 with 4 GB RAM, this reduces global allocator contention and improves cache locality.

- **Negative (Arc overhead on single-path):** For pipelines with no fan-out (linear chains), the Arc indirection adds one pointer dereference per header field access. This is negligible (~1 ns L1 cache hit) but non-zero.

- **Negative (metadata mutation requires Arc::make_mut):** Adding metadata after construction triggers a copy-on-write via `Arc::make_mut`. This is acceptable because metadata is typically set once at creation and never modified during pipeline traversal.

- **Trade-off (Lineage is the only deep copy):** Lineage must be mutable per-hop (each node may stamp its trace). Keeping it outside the Arc means it is the one field that incurs allocation on clone. The two `Option<Box<str>>` fields are small enough (~32 bytes) that this remains far cheaper than the pre-refactor full-clone approach.

- **Forecloses:** Cannot transparently add mutable metadata fields to the shared header without introducing copy-on-write semantics or a separate mutable-metadata sidecar.

- **Downstream requirement:** All code paths that produce fan-out (router dispatch, DLQ copies, broadcast topologies) must clone via the standard `Clone` impl and never manually reconstruct the envelope, to preserve Arc sharing invariants.

## See Also

- [RFC-003 — Node type architecture](../rfcs/RFC-003-node-types.md) — Amendment A3 is the parent decision. Defines the three-field envelope and its interaction with borrow-only vs. ownership-taking node signatures.
- [RFC-002 — Host-side runtime architecture](../rfcs/RFC-002-host-runtime.md) — Original `RuntimeEnvelope` definition that A3 amends. Defines the error policy, `InstancePre`, and `Store` lifecycle that surround envelope usage.
- [ADR-0002 — SPSC bounded queues](0002-spsc-bounded-queues.md) — Queues that carry `RuntimeEnvelope` instances between nodes; the bounded capacity and backpressure design assumes cheap message cloning for fan-out scenarios.
- `crates/wafer-core/src/queue/envelope.rs` — Implementation of `RuntimeEnvelope`, `EnvelopeHeader`, and `Lineage` with unit tests verifying Arc sharing and Bytes pointer equality across clones.
