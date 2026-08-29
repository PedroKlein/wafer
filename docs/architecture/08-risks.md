# 08. Risks & Technical Debt

Risks that could invalidate a thesis claim, block the evaluation, or
require material rework. Each risk carries a likelihood × impact
estimate, the current mitigation, and the residual (post-mitigation)
exposure. Ordered roughly by product-of-likelihood-and-impact.

## R1 — Wasm boundary overhead exceeds the 30 %-of-eKuiper threshold on Raspberry Pi 5

**Likelihood:** Medium — literature reports Wasm hop overheads from
6 % (Lyu 2022) to 13.4 % (Sledge 2020) on x86; ARM numbers are noisier
and typically higher. Our pass criterion is aggressive.
**Impact:** High — RQ1 miss undermines the "viable performance"
half of the thesis contribution.
**Mitigation:** Envelope shape (`Arc<EnvelopeHeader>` + `Bytes` +
`Lineage`) makes clone ~10 ns. Filter/Router borrow-only signatures
avoid payload copy entirely. AOT cache brings cold-start prepare down
from ~30 ms to ~2 ms. Fuel + epoch are independently toggleable so
overhead can be attributed per mechanism (four-config decomposition).
See ADR-0011, ADR-0007, ADR-0013.
**Residual:** ResourceTable push/delete cost (~50 ns each) and
Canonical-ABI lift of `list<u8>` outputs are irreducible today.
If RQ1 misses the 30 % threshold, thesis narrative should shift to
per-hop absolute latency framing rather than throughput ratio.

## R2 — Attack containment fails in an unmeasured way

**Likelihood:** Low — the six attack scenarios (S1–S6) were designed
against `StoreLimits`, epoch OS-thread ticker, and WASI capability
scoping; those are enforced by wasmtime, not by our code.
**Impact:** High — RQ2 miss undermines the isolation half of the
contribution.
**Mitigation:** All six scenarios are exercised by the
`plugins/attacks/*` set, wired into `TestPipeline` integration tests
with explicit assertions on the offending node's `NodeState` and on
the pipeline-level state.
**Residual:** Side-channel and timing attacks are explicitly out of
scope (see Scope Qualifier in the `wafer-project` skill). The claim
is memory containment + capability scoping, not information-flow
control.

## R3 — Hot-swap loses messages under pathological backpressure

**Likelihood:** Medium — the runner selects between the input `mpsc`
receiver and the `watch::Sender<Option<SwapPayload>>`; a producer
saturating the input queue at the exact instant of swap could in
principle overflow retry buffers.
**Impact:** High — RQ3 requires zero message loss.
**Mitigation:** Retry buffer entries in flight at swap time are
flushed to DLQ with `DlqReason::HotSwapDrain` — not lost, just
diverted. `wafer-loadgen` tags every emitted message with a monotonic
sequence, and the eval harness asserts every sequence is either
sink-delivered or DLQ-recorded.
**Residual:** DLQ full during a swap under saturation would surface
as `DlqReason::QueueFull`; the eval harness catches this as a
"delivery is not to the primary sink" rather than a lost message.
Interpretation of DLQ-during-swap as acceptable (not-lost) is a
methodological choice — flagged in the thesis's threats-to-validity
section.

## R4 — Coordinated omission and warmup drift in the measurement harness

**Likelihood:** Medium — well-known pitfall in latency benchmarking
(Gil Tene 2012).
**Impact:** Medium — measurement artefacts that survive peer review
would invalidate NFR-PERF numbers even if the runtime meets the
target.
**Mitigation:** Open-loop `wafer-loadgen` (fixed emission rate, no
back-off on receiver stall). HdrHistogram at the sink side. 30 s
warmup exclusion, ADF stationarity verification (RFC-008 §warmup).
Bootstrap CI95 on all reported quantiles.
**Residual:** The eval harness is still under construction (RFC-008,
status: Accepted). Any missing knob (e.g. no SCHED_FIFO on the RPi)
would silently inflate variance.

## R5 — Single-process constraint limits production adoption

**Likelihood:** N/A — this is a scope choice, not a delivery risk.
**Impact:** Medium — the thesis viability claim is about edge
gateways, not cluster deployments. Reviewers may push back that the
constraint is arbitrary.
**Mitigation:** Positioning matrix and comparator narrative
(architecture/09) frame the single-process invariant as
*differentiator vs Azure IoT Operations* rather than a limitation.
Explicitly documented in ADR-0001 (wasmtime choice), ADR-0004
(native sources/sinks), and the `wafer-project` skill.
**Residual:** Production users needing multi-node deployment must
adopt a different runtime. Thesis makes no claim about horizontal
scalability.

## R6 — Component-Model tooling churn (wit-bindgen / wasmtime)

**Likelihood:** High — wasmtime is pinned to a git commit
(post-41.0.3) because wasmtime-wasi-nn 41.0.3 on crates.io has a bug
with the ort crate API. WIT bindgen is under active revision.
**Impact:** Low — build breakage, occasional API migration effort.
**Mitigation:** All three components (`wasmtime`, `wasmtime-wasi`,
`wasmtime-wasi-nn`) are pinned in the workspace `Cargo.toml`. Plugin
`crate-type = ["cdylib"]` and `wasm32-wasip2` target are stable at the
Component-Model level.
**Residual:** Bumping to wasmtime 42.x when it lands will require a
one-day migration; documented in `docs/operations/dependencies.md`.

## R7 — AOT cache invalidation misses across wasmtime versions

**Likelihood:** Low.
**Impact:** Medium — a stale cache would produce a component that
fails at instantiate time and never reaches the guest.
**Mitigation:** The cache key includes `blake3(wasm_bytes || platform
|| wasmtime_version || config_flags)`. Any change in any input
produces a different key. See ADR-0013.
**Residual:** If wasmtime's compiled-module format changes without a
version bump (unusual), stale entries would slip through. The AOT
loader validates the compiled module before use and falls back to
recompilation on validation error.

## R8 — Evaluation hardware access

**Likelihood:** Medium — the primary Raspberry Pi 5 unit and the optional Jetson Orin
are single instances at the author's site. A hardware failure would
extend the evaluation timeline.
**Impact:** Medium — RQ1 / RQ2 / RQ3 measurements are pinned to those
targets.
**Mitigation:** All experiments run in a container-free
`wafer-runtime` binary that can be cross-compiled to a replacement
device without code changes. Bench scripts are checked into
`eval/` and are hardware-agnostic.
**Residual:** Timeline slip only; no impact on the thesis claim.
