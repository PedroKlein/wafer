# 07. Quality requirements

WAFER's quality goals follow its three research questions. This page maps each
goal to the architectural mechanisms that serve it. The measurable criteria are
listed once in [non-functional requirements](../requirements/non-functional.md),
with thresholds declared in `verdict_rules` of `eval/canonical-matrix.json`.
The measurement method is in [RFC-008](../rfcs/RFC-008-evaluation-harness.md)
and the [result contract](../../eval/RESULT-CONTRACT.md).

| Quality goal | Architectural mechanisms | Requirements |
|---|---|---|
| Low per-message cost on an edge gateway (RQ1) | Wasm only on processing nodes, with native sources and sinks. A persistent Store per node. Call-scoped `borrow<buffer>` input and the refcounted `Arc<EnvelopeHeader>` + `Bytes` envelope. One bounded channel per destination with end-to-end backpressure. Relaxed atomic counters as the only always-on measurement. | NFR-PERF-1 to NFR-PERF-6 |
| Fault containment (RQ2) | One Store per node with `StoreLimits` (defaults 64 MiB for Transform, 16 MiB for Filter and Router). Fuel and epoch interruption, each off unless configured, with the epoch ticker on its own OS thread. A trapped or interrupted Store is replaced from the cached `InstancePre`, never reused. WASI gets no preopened directories, and stdio, environment, inference, and outbound HTTP only by explicit grant. | NFR-ISO-1 to NFR-ISO-4 |
| Live, stateless replacement (RQ3) | Watch-channel swap between messages, with compile and link off the worker pool. `validate()` and `init()` run before adoption and v1 is kept on failure. Transform canary rollback. Pending retries go to the DLQ instead of being replayed against v2. | NFR-SWAP-1 to NFR-SWAP-5 |
| Accountable overload | A per-edge overflow policy (`slow`, `drop`, `dead-letter`) with a counter for every disposition, and a DLQ envelope that records the reason. | E-Backpressure |

The final evaluation enables fuel and epoch limits explicitly in every ordinary
WAFER config. Only the metering ablation (E-Perf-7) and the attack stimuli that
must not be pre-empted by another mechanism differ.
