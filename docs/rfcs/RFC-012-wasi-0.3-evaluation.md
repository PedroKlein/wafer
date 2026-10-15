# RFC-012: WASI 0.3 and Component Model Evolution

- **Status:** Async P2 and bounded outbound HTTP implemented; P3 PoC not started
- **Assessment dates:** 2026-09-25 to 2026-09-26
- **Amends:** —
- **Amended by:** [ADR-0016](../adr/0016-outbound-wasi-http-capability.md)

## Abstract

WASI 0.3 adds native Component Model async functions, futures, and streams. These
features align with WAFER's asynchronous pipeline runtime, but the current WAFER
plugin contract, guest toolchains, cancellation rules, and hot-swap semantics are
built around synchronous WASI 0.2 components and one call per message.

WAFER retains its existing WASI 0.2 ABI while evaluating its successor. The
host now uses Wasmtime's asynchronous Preview 2 bindings without changing guest
WIT or plugin bytes, and processing nodes may receive a default-deny outbound
`wasi:http` grant for exact destinations.

The thesis evaluation ran on the P2 `wafer:pipeline@0.1.0` contract. A
Preview 3 PoC has not been started. If pursued, it uses a separately versioned
package, tests native async functions and streams against the retained P2 path,
and does not change the evaluation evidence. The PoC may lead to a versioned
WIT migration; it is not production P3 support.

## Current implementation

WAFER currently has the following runtime contract:

- Rust plugins target `wasm32-wasip2` and use `wit-bindgen` against `wit/`;
- the host registers `wasmtime_wasi::p2::add_to_linker_async` and the HTTP-only async P2 linker;
- WIT lifecycle, Transform, Filter, and Router exports remain synchronous for guests and are awaited by the host;
- Wasm runners execute as ordinary Tokio tasks without `spawn_blocking`, nested `block_on`, or `block_in_place` bridges;
- each node owns one persistent `Store` and permits one guest call at a time;
- an input payload is a call-scoped `borrow<buffer>` backed by host `Bytes`;
- fuel and epoch deadlines are reset before each guest call;
- active guest calls are never cancellation branches in `select!`;
- a trapped or interrupted Store is replaced rather than reused; and
- hot-swap occurs between messages, preserving configured capabilities and
  resource limits while discarding guest instance state; and
- optional outbound `wasi:http` access is enforced per request against an immutable exact-destination grant.

The workspace pins Wasmtime 48.0.2 at revision
`e9f1ea232fd245aea338ab3eb7d73487ae75cab1`. WASI 0.3 is a ratified
specification, but both the pinned revision and current Wasmtime main describe
their P3 WASI and HTTP embedder modules as experimental, incomplete, outside
semver guarantees, and not ready for production. WAFER therefore keeps P3 out
of production while using the PoC to measure the actual runtime and toolchain.

## Relevant WASI 0.3 capabilities

### Native async functions

WIT may declare `async func` imports and exports. The runtime coordinates one
event loop across composed components, allowing guest work to suspend without
the Preview 2 `pollable` and `start`/`finish` conventions.

For WAFER, native async is most relevant to plugins that call HTTP services,
wait on timers, use remote state, or invoke asynchronous device capabilities.
It does not justify concurrent calls into one stateful node instance by itself.

### Futures and streams

`future<T>` represents one asynchronously delivered owned value. `stream<T>`
represents an asynchronously transferred sequence with runtime-managed
backpressure. A stream operation can expose a separate completion future so a
consumer can observe terminal success or failure even when it stops reading
early.

Streams could amortize the Component Model call boundary across multiple
messages and support chunked payloads. They do not automatically provide
zero-copy transfer. Planned lazy-ABI and stream-splicing work may improve that
later.

### Concurrent component calls

Wasmtime exposes concurrent-store APIs including `Store::run_concurrent`,
`Func::call_concurrent`, `StreamReader`, and `FutureReader`. Adopting them would
change WAFER's one-task, one-Store, one-call invariant and complicate ordering,
fuel accounting, epoch deadlines, recovery, and hot-swap. They remain out of
scope until a measured workload demonstrates a need for same-instance
concurrency.

### Evolving WIT and composition tooling

Newer Component Model work includes `map<K,V>`, richer interface annotations,
component interposition, and planned lazy values, optional imports, cooperative
threads, and runtime instantiation. WAFER should track these features, but none
currently warrants changing the production plugin ABI.

## Direction

### D1: Preserve P2 until the versioned P3 decision

`wafer:pipeline@0.1.0` and existing `wasm32-wasip2` plugins remain the verified
baseline during the PoC. The experiment uses a separate package or world and
does not create a dual production runtime. If the adoption gates pass, WAFER
may move to a new package version before its first release; otherwise P2 ships
as the initial contract.

### D2: Adopt asynchronous WASI 0.2 host bindings

The host-only migration from synchronous P2 linker/bindgen calls to Wasmtime's
asynchronous P2 bindings was adopted. It removed the nested synchronous shim
and `block_in_place` path without changing guest WIT or plugin binaries.

The adoption evidence proves:

- existing P2 plugins continue to load and behave identically;
- no Wasm call is cancellation-dropped and then reused;
- fuel, epoch deadline, log cleanup, borrowed-buffer cleanup, metadata, and
  fresh-Store recovery invariants remain intact;
- saturated-worker epoch interruption and cooperative shutdown remain bounded;
- hot-swap still occurs only at a safe between-message boundary; and
- measured throughput, latency, and memory do not regress beyond an explicitly
  accepted bound.

All frozen correctness, simplicity, throughput, p95 latency, and RSS gates passed. The asynchronous P2 implementation is now the sole production linker-to-call path.

### D2.1: How the async P2 host was adopted

The migration changed only the host execution path for the existing
`wafer:pipeline@0.1.0` worlds. Before it, the host registered
`p2::add_to_linker_sync`, ran each Wasm runner through `spawn_blocking` plus
`Handle::block_on`, and wrapped every guest call in `block_in_place`. After it,
the host registers `wasmtime_wasi::p2::add_to_linker_async`, generates async
imports and exports in each `bindgen!` invocation, awaits instantiation,
lifecycle, and processing calls, and runs each runner as an ordinary Tokio
task. WIT files and plugin bytes did not change.

The change was accepted only if three gates passed:

- **Correctness.** The prepared-artifact test matrix (Transform, Filter,
  Router, inference, Rust and TinyGo P2 components, trap cleanup, saturated
  workers, shutdown, recovery, reconfigure, hot-swap and rollback, limits,
  metering, logging, metadata, lineage, retry count) passed with no skips.
- **Strict simplicity.** Zero production `spawn_blocking` or `block_in_place`
  bridges around guest calls, one linker-to-call path with no sync fallback,
  and no Store mutex, per-message spawn, or Wasm future in `select!`.
- **Performance.** On the in-process E-Perf-4 pass-through configs at 120 B,
  1 KiB, and 100 KiB, 10 counterbalanced baseline/candidate pairs per payload
  had to keep median throughput no more than 5 percent below the baseline,
  median p95 latency no more than 5 percent above it, and peak RSS no more
  than max(5 percent, 2 MiB) above it.

All three passed, and the async path replaced the synchronous one. The
measurements were taken on a development machine to decide the
implementation; they are not thesis evidence.

### D3: A possible P3 PoC

The thesis evaluation ran on the P2 `wafer:pipeline@0.1.0` contract. A P3
PoC, if pursued, uses a separately versioned package and does not change the
evaluation evidence. Run before the first release, it
lets a breaking WIT change land without external compatibility cost. It leaves
the verified `@0.1.0` P2 path intact until the adoption decision. It has two
stages:

1. a toolchain smoke test for one Rust component, P3 HTTP, and the maintained Go path; and
2. a streaming pass-through slice compared with the P2 message-at-a-time path.

The current Go proof uses TinyGo's WASI P2 target. Official P3 examples use
`componentize-go`, so the PoC must test TinyGo first and evaluate
`componentize-go` only if TinyGo cannot produce the versioned contract. A P3
migration cannot silently discard the bounded Go interoperability claim.

Async functions, futures, and streams carry owned values. The current
call-scoped `borrow<buffer>` message therefore cannot be moved into a P3 async
or streaming interface unchanged. The PoC must compare an owned buffer resource,
an owned byte list, or a separate payload stream rather than silently giving up
the existing low-copy Filter/Router behavior.

The prototype must answer:

- whether streams improve per-element throughput or boundary cost over bounded
  Tokio queues plus the borrowed-buffer resource;
- how owned stream elements interact with WAFER's current call-scoped
  `borrow<buffer>` optimization;
- where message ordering, lineage, retry, DLQ, and per-message metrics live;
- whether fuel and epoch limits apply per element, batch, or stream lifetime;
- how an active stream drains or terminates during shutdown and hot-swap;
- when an interrupted or cancelled Store must be discarded; and
- whether Component Model backpressure complements or duplicates WAFER's
  receiver-keyed bounded queues.

A pass-through prototype is sufficient. It must compare message-at-a-time and
streamed P3 variants against the retained P2 path and must not be represented as
production support. The comparison must cover 120 B, 1 KiB, and 100 KiB payloads,
representative pipeline depths, throughput, p50/p95/p99, RSS, ordering, loss,
duplication, startup, shutdown, trap recovery, and hot-swap. This measurement is
mandatory because current Component Model async task infrastructure can add
substantial overhead to otherwise synchronous calls; P3 is not presumed faster.

### D4: Add capabilities independently of P3

Outbound `wasi:http` is implemented independently of P3 under
[ADR-0016](../adr/0016-outbound-wasi-http-capability.md). It is default-deny,
scoped to exact `(scheme, canonical-host, effective-port)` destinations, and
enforced per request through a policy-controlled single-resolution connector.
Recovery, reconfigure, hot-swap, and rollback retain the loaded node's immutable
grant.

Runtime config and key-value proposals remain conditional:

- runtime config is useful if plugins need host-managed secrets or large
  independently managed configuration sets; and
- key-value storage is useful only if WAFER deliberately introduces opt-in
  state persistence across hot-swap, which changes the current stateless-swap
  contract.

Native Sources and Sinks remain the preferred owners of MQTT, HTTP ingress, and
other transport credentials.

### D5: Treat interposition as diagnostic tooling first

Component interposition can wrap imports or exports with record/replay,
tracing, policy, or fault-injection components. WAFER may evaluate tools such as
`splicer` for reproducible plugin diagnostics. Interposed components are not
initially equivalent to independently scheduled WAFER nodes because their
resource limits, metrics, and hot-swap boundaries are not represented in the
DAG.

## Adoption gates

A production P3 path requires all of the following:

1. the selected Wasmtime P3 embedder API is documented as production-ready;
2. Rust guest tooling builds the required world without release-candidate
   dependencies;
3. the maintained Go path can build and execute the corresponding contract, or
   the new world is explicitly Rust-only and does not weaken the existing
   polyglot claim;
4. stream cancellation, shutdown, trap recovery, and Store disposal have
   subprocess tests with outer timeouts;
5. hot-swap has a defined and tested stream quiescence boundary;
6. capability grants remain link-time default-deny and survive recovery and
   replacement without expansion;
7. the prototype shows a concrete operational or measured benefit over the P2
   message-at-a-time path; and
8. evaluation evidence from the P2 path is not relabeled as P3 evidence.

## Non-goals

This PoC does not:

- replace the current plugin ABI before the adoption decision;
- promise same-instance concurrent message processing;
- treat `stream<T>` as proof of zero-copy transfer;
- move native transport adapters into guest components by default;
- add state migration between plugin versions;
- change current thesis results or release-readiness claims; or
- adopt experimental Component Model features solely because the pinned
  Wasmtime revision contains them.

## Implementation status and next order

1. **Complete:** adopt asynchronous P2 host bindings after matched correctness, simplicity, and performance gates.
2. **Complete:** implement and adversarially verify bounded outbound P2 `wasi:http`.
3. **If pursued:** build the isolated P3 Rust/HTTP/Go toolchain smoke test.
4. **Next if the smoke passes:** build one P3 streaming pass-through slice.
5. Compare P2, P3 message-at-a-time, and P3 streaming behavior under fixed conditions.
6. Before release, decide whether to migrate to a new WIT package version or ship P2 as the initial contract.

The P3 work requires its own plan and frozen acceptance criteria.

## Alternatives considered

### Replace all worlds with WASI 0.3 immediately

Rejected before the PoC. It combines host runtime, guest toolchain, WIT ABI,
polyglot support, hot-swap, and evaluation changes without first establishing a
benefit. A versioned migration before the first release remains available if the
PoC passes every adoption gate.

### Use same-instance concurrent calls immediately

Rejected. Parallel component calls would weaken WAFER's local ownership model
and make message order, state mutation, metering, and recovery harder to prove.
Multiple isolated node instances remain the simpler concurrency mechanism.

### Replace metadata pairs with `map<string, string>`

Deferred. The current representation is ordered and permits duplicate keys. A
map should be adopted only in a future major contract after metadata semantics
are explicitly changed to unique and unordered.

### Wait for Component Model 1.0 before any work

Rejected. Async P2 bindings can improve the host now, and a bounded P3 prototype
can provide useful evidence without committing the production ABI.

## References

- [WASI 0.3 announcement](https://bytecodealliance.org/articles/WASI-0.3)
- [WASI 0.3.0 release notes](https://github.com/WebAssembly/WASI/releases/tag/v0.3.0)
- [WASI releases and proposal status](https://wasi.dev/releases)
- [Component Model features adopted by WASI](https://github.com/WebAssembly/WASI/blob/main/docs/ComponentModelFeatures.md)
- [The road to Component Model 1.0](https://bytecodealliance.org/articles/the-road-to-component-model-1-0)
- [Component interposition and `splicer`](https://bytecodealliance.org/articles/how-wasm-components-enable-pluggable-middleware)
- [Current Wasmtime P3 warning](https://github.com/bytecodealliance/wasmtime/blob/main/crates/wasi/src/p3/mod.rs)
- [Current Wasmtime P3 HTTP warning](https://github.com/bytecodealliance/wasmtime/blob/main/crates/wasi-http/src/p3/mod.rs)
- [componentize-go P3 HTTP example](https://github.com/bytecodealliance/componentize-go/tree/main/examples/wasip3/export_wasi_http_handler)
- `Cargo.toml` and `Cargo.lock` — pinned Wasmtime source and revision
- `wit/` — current production plugin contracts
- `crates/wafer-core/src/engine/bindings.rs` — current async P2 host bindings
- `crates/wafer-core/src/engine/loader.rs` — current P2 WASI and HTTP linkers
- `crates/wafer-core/src/engine/http.rs` — exact-destination outbound HTTP enforcement
- `crates/wafer-core/src/node/wasm.rs` — per-call Store, lifecycle, and resource invariants
- `crates/wafer-core/src/runner/` — ordinary Tokio task execution for Wasm nodes
