# RFC-012: WASI 0.3 and Component Model Evolution

- **Status:** Async P2 and bounded outbound HTTP implemented; P3 PoC approved
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

Before WAFER's first release and canonical campaign, a separate Preview 3 PoC
will test native async functions and streams against the retained P2 path. The
PoC may lead to a versioned WIT migration; it is not production P3 support.

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

### D2.1: Frozen production P2 host experiment

This experiment is independent of the P3 prototype. It changes only the host
execution path for the existing `wafer:pipeline@0.1.0` worlds. The baseline is
the P1-T1 checkpoint; the candidate is the later P1-T5 checkpoint. Both SHAs
must be full 40-character commits, both worktrees must be clean when measured,
and both must use byte-identical WIT files, P2 plugin artifacts, configs, and
release-build inputs.

The workspace and lockfile pin Wasmtime 48.0.2 to
`e9f1ea232fd245aea338ab3eb7d73487ae75cab1`. At that revision the candidate API
surface is:

- `wasmtime_wasi::p2::add_to_linker_async` from
  `crates/wasi/src/p2/mod.rs`;
- `imports: { default: async }`, `exports: { default: async }`, and
  `require_store_data_send: true` in each host `bindgen!` invocation;
- generated `TransformNodePre`, `InferenceNodePre`, `FilterNodePre`, and
  `RouterNodePre` `instantiate_async` methods; and
- awaited generated lifecycle and processing calls. The generated method names
  remain `call_validate`, `call_init`, `call_process`, `call_evaluate`, and
  `call_route`; their return values become futures because the exports are
  generated as async.

`Config::async_support(true)` is not part of the candidate: it is deprecated
and has no effect at the pinned revision. `wasmtime_wasi_nn::wit::add_to_linker`
remains the inference import wiring; the inference world's instantiation,
lifecycle, and Transform export calls still use the async Component Model API.
No P3 feature or binding is enabled.

#### Frozen baseline/candidate inventory

| Path | Frozen synchronous baseline | Async candidate | Preserved boundary |
| --- | --- | --- | --- |
| Startup and linker | `WaferEngine::build_linker` registers `p2::add_to_linker_sync`; `pre_instantiate_{transform,inference,filter,router}` creates typed `*Pre` values. `launch_pipeline_timed` loads each component and creates one Store per loaded node. | Register `p2::add_to_linker_async`, generate async imports/exports, and keep the same typed `*Pre` split. | One engine, one Store owner and one loaded instance per node; the four existing worlds and capabilities do not change. |
| Initial instantiation | `load_transform_node`, `load_filter_node`, and `load_router_node` call synchronous `*Pre::instantiate`, then synchronous lifecycle methods. | Await `*Pre::instantiate_async`, `validate`, and `init` in the existing sequential launch future. | Store limits, fuel before start functions, relative epoch deadline, plugin version, config, and inference grant remain identical. |
| Lifecycle | `Wasm*Node::validate_and_init` resets configured fuel and epoch, calls `validate` then `init`, and flushes guest logs. The baseline Wasm runner does not invoke the guest `close` export; retirement drops the Store. | Await the same ordered calls. Do not add a new `close` behavior as part of this migration. | No lifecycle-policy change and no early-return cleanup change. |
| Processing | `WasmTransformNode::process`, `WasmFilterNode::evaluate`, and `WasmRouterNode::route` synchronously clear logs, reset metering, push one borrowed buffer, call the guest, flush logs, delete the buffer on success/error/trap, and map the result. | Make these methods async and await the generated export while retaining that exact setup/call/cleanup order. | Metadata, payload, lineage, retry count, logging, metering, and resource cleanup remain unchanged. |
| Runner scheduling | `spawn_wasm_runner` uses `JoinSet::spawn_blocking` plus `Handle::block_on`; each Transform, Filter, and Router call is wrapped in `tokio::task::block_in_place`. | Spawn each runner as an ordinary `JoinSet` task and await the node call directly. | `ProcessingGuard` spans the whole call. No per-message task, Store mutex, task abort, timeout race, or `select!` branch around a Wasm future. |
| Trap and timeout recovery | Each runner handles the completed error, then `recover_from_cached_pre` creates a fresh Store, reapplies capabilities, memory, fuel, and epoch configuration, synchronously instantiates the cached pre-instance, and reruns lifecycle. | Await fresh-Store instantiation and lifecycle before receiving another message. | A trapped or interrupted Store is never reused. |
| Reconfigure | `try_reconfigure` prepares a fresh Store and cached instance synchronously, swaps it in, runs lifecycle, and restores the complete prior Store/bindings/config on rejection. | Await preparation and lifecycle before committing or rolling back. | Reconfigure remains atomic and between messages. |
| Hot-swap preparation and adoption | `prepare_*_swap_timed` already calls `instantiate_async`, but the `*Pre` was built from the synchronous P2 linker; the runner applies the prepared Store between messages and runs synchronous lifecycle. | Use the async P2 linker and generated bindings for preparation and await lifecycle during between-message adoption. | Watch-channel signalling, capabilities, memory limits, canary rollback, and one active call remain unchanged. |
| Direct test/benchmark harness | `PluginTestHarness::load_transform_with_memory_limit` synchronously instantiates; `TransformHarness::process` calls the synchronous wrapper. | Provide the same async instantiation/call route needed by production-bound tests and benchmarks without exporting test-only production APIs. | The harness still exercises the production bindings and cached pre-instance. |
| Existing mixed async site | Hot-swap preparation uses `instantiate_async` while startup, recovery, reconfigure, and calls remain synchronous. | One async production path from linker through calls; no sync compatibility branch. | Existing plugin bytes and WIT ABI remain unchanged. |

The inventory is reconciled with these repository searches. Results from tests
or comments are retained in evidence but do not count as production sites:

```sh
DEVELOPER_DIR=/Library/Developer/CommandLineTools git grep -n -E \
  'add_to_linker_(sync|async)|spawn_blocking|block_in_place|\.instantiate(_async)?\(|call_(validate|init|process|evaluate|route)' \
  -- crates/wafer-core/src

WASMTIME_REPO=/Users/i572543/Dev/pi-repos/repos/github.com/bytecodealliance/wasmtime/main
DEVELOPER_DIR=/Library/Developer/CommandLineTools git -C "$WASMTIME_REPO" \
  grep -n -E 'pub fn add_to_linker_(async|sync)|require_store_data_send' \
  e9f1ea232fd245aea338ab3eb7d73487ae75cab1 -- \
  crates/wasi/src/p2 crates/wasmtime/src/runtime/component examples/wasip2-async/main.rs
```

#### Strict-simplicity gate

The candidate is strictly simpler only if all of the following are true:

1. production has zero Wasm-runner `spawn_blocking` calls and zero production
   `block_in_place` calls;
2. startup, recovery, reconfigure, hot-swap, lifecycle, and processing use one
   async linker/binding/call path, with no sync fallback, feature switch, or
   duplicate node wrapper;
3. each node still owns its Store directly and executes at most one guest call;
4. no Store mutex, per-message spawn, normal-path abort, cancellation timeout,
   or Wasm future in `select!` is introduced; and
5. the candidate removes the blocking bridge without adding an equivalent
   scheduling bridge elsewhere.

The P1-T5 receipt records the before/after production-site inventory and these
counts:

```text
blocking_bridge_sites = production spawn_blocking sites for Wasm runners
                      + production block_in_place sites around guest calls
production_p2_execution_paths = distinct sync or async linker-to-call paths
```

Strict simplicity requires candidate `blocking_bridge_sites = 0`, candidate
`production_p2_execution_paths = 1`, and both candidate values lower than the
baseline values where the baseline is nonzero. A source diff must also confirm
that no replacement bridge or dual mode was added. Failure of any item rejects
the candidate even if performance improves.

#### Correctness gate

Before any performance result is admissible, the mandatory prepared-artifact
matrix must pass without a missing fixture, ignored required test, or skip. It
covers Transform, Filter, Router, inference, Rust P2 plugins, the TinyGo P2
uppercase component, repeated success/error/trap cleanup, saturated workers,
active-call shutdown, fresh-Store recovery, reconfigure, hot-swap/rollback,
capabilities, memory limits, metering, logging, metadata, lineage, retry count,
and exactly-once forwarding. Formatting, clippy with warnings denied, and the
workspace test suite are also mandatory. The exact matrix is frozen in P1-T2
and executed in P1-T5; a failure leaves performance results diagnostic only.

#### Matched A/B workload

Three conditions use the existing in-process E-Perf-4 boundary: one
`bench-source`, one pass-through P2 Transform, and one `bench-sink`.

| Condition | Checked-in config | Payload |
| --- | --- | ---: |
| `120b` | `eval/configs/canonical/e-perf-4-120b.toml` | 120 bytes of `0x42` |
| `1kb` | `eval/configs/canonical/e-perf-4-1kb.toml` | 1,024 bytes of `0x42` |
| `100kb` | `eval/configs/canonical/e-perf-4-100kb.toml` | 102,400 bytes of `0x42` |

Every condition has 10 matched pairs: 20 runs per condition and 60 runs total.
Order is counterbalanced and alternating: odd pairs run `baseline,candidate`;
even pairs run `candidate,baseline`. Runs are not pooled across payload
conditions. A failed or incomplete run remains in place and does not count; its
replacement uses the next attempt number without changing the pair index or
arm order.

Controlled factors for every run are fixed as follows:

- `TOKIO_WORKER_THREADS=4`;
- release mode with `--locked` builds;
- queue capacity 1,024, resolved from the unchanged
  `EngineConfig::default_queue_capacity` because these configs omit an override;
- 1,000 messages/s, 30,000 warmup messages, 60,000 measured messages, and a
  120-second outer duration;
- 30-second sink warmup and a 60-second declared measurement window;
- Transform fuel 10,000,000, Filter/Router fuel 500,000, epoch deadline 100,
  and epoch tick 10 ms;
- the same machine, OS session, power mode, toolchain, config bytes, runtime
  arguments, pass-through artifact bytes, and no unrelated workload; and
- one condition completed before moving to the next condition.

The baseline and candidate run commands and the non-thesis artifact schema are
specified in [`eval/RESULT-CONTRACT.md`](../../eval/RESULT-CONTRACT.md#production-p2-async-ab-experiment).

#### Metrics and fail-closed decision

For run `i`, measured after warmup:

- `T_arm,i = measured_unique_messages / measurement_duration_seconds`;
- `L_arm,i = p95_ns` from the complete `latency.hdr`;
- `R_arm,i = max(rss_bytes)` from `memory.csv`.

For each condition, `T_arm` and `L_arm` are the medians of the 10 run values.
`R_arm` is the maximum observed run-level peak, preserving a fail-closed memory
gate. Deltas use the baseline denominator:

```text
throughput_delta_pct = 100 * (T_candidate - T_baseline) / T_baseline
p95_latency_delta_pct = 100 * (L_candidate - L_baseline) / L_baseline
rss_delta_bytes = R_candidate - R_baseline
rss_allowance_bytes = max(0.05 * R_baseline, 2 * 1024 * 1024)
```

| Correctness | Strict simplicity | Every condition: throughput | Every condition: p95 latency | Every condition: peak RSS | Decision |
| --- | --- | --- | --- | --- | --- |
| pass | pass | `throughput_delta_pct >= -5.0` | `p95_latency_delta_pct <= 5.0` | `rss_delta_bytes <= rss_allowance_bytes` | `adopt-async-p2` |
| fail | any | any | any | any | `retain-sync-p2` |
| pass | fail | any | any | any | `retain-sync-p2` |
| pass | pass | below budget in any condition | any | any | `retain-sync-p2` |
| pass | pass | pass | above budget in any condition | any | `retain-sync-p2` |
| pass | pass | pass | pass | above budget in any condition | `retain-sync-p2` |
| missing, malformed, dirty, unpaired, or fewer than 10 valid pairs | any | any | any | any | `retain-sync-p2` |

The matched experiment admitted all 30 required pairs and selected
`adopt-async-p2`. The retained implementation has zero production blocking
bridge sites and one production P2 execution path. Median throughput remained
within ±0.001% of the fixed 1,000 msg/s offered rate, while median p95 latency
decreased by 29.85%, 12.79%, and 7.71% for 120 B, 1 KiB, and 100 KiB payloads.
These macOS measurements support the implementation decision only and are not
canonical thesis evidence. The retained host migration is commits `fb129b4` and
`e7b9779`; decision tooling is `75ab337`.

### D3: Run a P3 PoC before the first release

The P3 experiment runs before the first release and canonical campaign, while a
breaking WIT change has no external compatibility cost and before measurements
would need to be repeated. It uses a separately versioned package or world and
leaves the verified `@0.1.0` P2 path intact until the adoption decision. It has
two stages:

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
grant. The contract, implementation, and H01–H20 verification checkpoints are
`eaffa91`, `40a730a`, and `f173151` respectively.

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
3. **Next:** build the isolated P3 Rust/HTTP/Go toolchain smoke test.
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
