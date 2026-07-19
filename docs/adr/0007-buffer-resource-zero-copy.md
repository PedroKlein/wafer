# ADR-0007: borrow\<buffer\> Zero-Copy Input

- **Date**: 2026-07-05
- **Status**: Accepted
- **Parent RFC**: [RFC-001](../rfcs/RFC-001-wit-contracts.md)

## Context

WAFER pipeline nodes receive messages at the WebAssembly Component Model boundary. The payload is the dominant allocation cost: telemetry messages range 100B–1KB (UC1 JSON) to 784B (UC2 MNIST). In a naïve design every boundary crossing copies the payload into component linear memory whether or not the guest reads it. Filter nodes that evaluate metadata alone, and router nodes that examine only a few bytes, would pay the full copy cost for zero benefit.

The Component Model provides the `borrow<T>` qualifier on resource handles: the host retains ownership and the guest receives a read-only reference that is valid only for the duration of the call. This maps directly to Rust's single-owner `Bytes` refcounting, where sharing is a ~5 ns Arc bump rather than a memcpy.

The prior MVP (`pipeline:transform@0.1.0`) used a symmetric `list<u8>` for both input and output, which required copying the payload into the component heap before the guest could inspect it.

## Decision

We pass the input payload as a `borrow<buffer>` resource handle in the WIT `message` record. The `buffer` resource exposes three read-only methods — `size() → u64`, `read(offset, len) → list<u8>`, and `read-all() → list<u8>` — and is defined in `pipeline:types@0.1.0`. On the host side, `WaferBuffer` (in `crates/wafer-core/src/engine/buffer.rs`) wraps a `bytes::Bytes` value. Creating this wrapper from the `RuntimeEnvelope` payload is an Arc refcount bump — no allocation or copy. The buffer handle is pushed into the per-Store `ResourceTable` immediately before the guest call and is dropped immediately after, ensuring call-scoped lifetime with deterministic cleanup.

Output remains `list<u8>` on `output-message`: the component owns the bytes it produces, and the host adopts them into a new `Bytes` via the Canonical ABI lift.

## Consequences

- **Positive — filter/router zero-copy:** A filter that examines only `content-type` or `metadata` never calls `read()`, so zero payload bytes cross the Wasm boundary. The host forwards the same underlying `Bytes` by cloning the refcount, not the data.

- **Positive — read-on-demand granularity:** A router that needs only the first 16 bytes (e.g. a JSON type-tag) calls `read(0, 16)` instead of `read-all()`, paying for exactly the bytes it inspects.

- **Positive — uniform lifetime management:** Call-scoped creation/destruction means no resource leaks across pipeline ticks. The `ResourceTable::push` / implicit drop pattern in `crates/wafer-core/src/node/wasm.rs` guarantees the handle cannot outlive the invocation.

- **Positive — cheap fan-out:** When a router sends the message to multiple output ports, the host clones the `Bytes` (Arc bump) per port. No payload duplication regardless of fan-out factor.

- **Positive — composable with envelope immutability:** Because the buffer is read-only from the guest's perspective, `Arc<EnvelopeHeader>` plus shared `Bytes` forms a near-free-clone envelope (RFC-003 §A3 amendment). Borrow-only nodes (filter, router) never trigger a deep clone.

- **Negative — two copies for transforms:** A transform must call `read-all()` (host→guest copy) then produce `list<u8>` output (guest→host lift). This is an irreducible cost of the Wasm sandbox boundary — the component has its own linear memory.

- **Negative — ResourceTable overhead per call:** Each invocation does one `ResourceTable::push` and one implicit `drop`. This adds ~50–100 ns per message. On RPi 4 at the target rate of 10 000 msg/s this is ≤1 ms/s total — well within budget.

- **Forecloses — mutable input buffers:** Guests cannot write back into the host-owned buffer. Any in-place transformation pattern (e.g. decryption without reallocation) is impossible; the guest must always produce a new `list<u8>`.

- **Downstream requirement — plugin SDK must abstract:** Guest-side helpers (`wafer-plugin` crate's `from_input()` / `parse_config()`) must hide the `buffer.read-all()` call so plugin authors deal in `&[u8]` or deserialized types, not raw resource handles.

## See Also

- [RFC-001](../rfcs/RFC-001-wit-contracts.md) — the full WIT contract design session; Decision 7 specifies the asymmetric payload boundary.
- [RFC-002](../rfcs/RFC-002-host-runtime.md) — defines `RuntimeEnvelope` with `Bytes` payload on the host side.
- [RFC-003](../rfcs/RFC-003-node-types.md) — §A3 amends the envelope to `Arc<EnvelopeHeader> + Bytes`, enabling the near-free clone that makes borrow-only forwarding viable.
- `wit/pipeline-types.wit` — the `buffer` resource definition with `size`, `read`, `read-all` methods.
- `crates/wafer-core/src/engine/buffer.rs` — host-side `WaferBuffer` implementation wrapping `Bytes`.
- `crates/wafer-core/src/engine/bindings.rs` — `HostTypes` impl mapping `Resource<WaferBuffer>` methods to `WaferBuffer` calls.
- `crates/wafer-core/src/node/wasm.rs` — `build_wit_message()` showing the push-into-ResourceTable pattern.
