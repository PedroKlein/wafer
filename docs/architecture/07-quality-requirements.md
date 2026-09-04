# 07. Quality requirements

These non-functional requirements define the current evaluation boundaries. `eval/canonical-matrix.json` is the executable experiment source; `eval/RESULT-CONTRACT.md` defines required evidence.

## RQ1: performance on an edge gateway

| ID | Requirement | Final method and criterion |
|---|---|---|
| NFR-PERF-1 | Bound the typed Wasm call cost. | E-Perf-4 reports run-level empty pass-through latency; median per-hop latency must be below 50 µs on Raspberry Pi 5. |
| NFR-PERF-2 | Compare matched Pipeline A at the target load. | E-Perf-1 uses 30 runs per SUT at 1,000 msg/s. Pooled loss must be at most 1 percent, mean achieved/offered at least 0.99, and median WAFER p95 divided by median eKuiper p95 at most 2.0. |
| NFR-PERF-3 | Measure the co-located gateway-capacity envelope. | E-Perf-10 uses 30 runs for every system-rate pair on `[1,000, 4,000, 8,000, 15,000, 16,000]` msg/s. Report delivery ceiling and normalized p99 knee with MQTT support-path censoring. Compare WAFER/eKuiper ceilings against 0.70 only when the ratio is identifiable. |
| NFR-PERF-4 | Bound memory growth with pipeline depth. | E-Perf-6 reports total RSS for five nodes and the run-level slope; limits are 150 MB total and 10 MB per added node. |
| NFR-PERF-5 | Compare architecture-specific isolation cost. | E-Perf-5 requires matched Raspberry Pi 5 and x86 Linux runs at the same source and method. It remains `PENDING` without both. |
| NFR-PERF-6 | Measure startup cache-state effects. | E-Perf-9 compares Linux filesystem page-cache cold/warm conditions. The runtime disk compiled-component cache is disabled, so no AOT-cache criterion is derived from this experiment. |

Pipeline A is `MQTT source -> threshold filter -> MQTT sink`. E-Perf-1 is not a capacity experiment. E-Perf-10 cannot assign an exact SUT ceiling beyond a delivery-bad MQTT loopback point.

## RQ2: fault containment

| ID | Requirement | Final method and criterion |
|---|---|---|
| NFR-ISO-1 | Contain all six attack scenarios. | The offending node may recover or fail, but the process and healthy nodes remain available. |
| NFR-ISO-2 | Limit healthy-branch impact. | E-Iso-7 uses independent source/sink populations and requires less than 1 percent branch-A throughput drop. |
| NFR-ISO-3 | Bound guest memory. | Per-store limits are 64 MiB for Transform and 16 MiB for Filter and Router unless configured otherwise. |
| NFR-ISO-4 | Interrupt infinite execution. | E-Iso-4 isolates epoch containment with its matrix-declared metering exception and records trap/recovery evidence. |

Runtime fuel and epoch limits default to `None`. Ordinary final WAFER configs enable both explicitly; attack-specific exceptions prevent another mechanism from preempting the fault under test.

## RQ3: stateless hot-swap disruption

| ID | Requirement | Final method and criterion |
|---|---|---|
| NFR-SWAP-1 | Bound repeated-swap pause. | E-Swap-1 reports sink-observed gaps and internal phases separately; p95 sink gap must be below 100 ms. |
| NFR-SWAP-2 | Preserve message accounting. | E-Swap-2 requires zero sequence loss and duplication. |
| NFR-SWAP-3 | Bound output disruption against restart comparators. | E-Swap-3 has 30 runs per strategy, one action at measured t=60, and 200 actual-t0-aligned 100 ms buckets over `[-10,+10)`. The WAFER criterion uses the upper bootstrap CI for median dip, which must be below 5 percent with zero loss and duplication. Restart dips are measured, not assumed. |
| NFR-SWAP-4 | Bound pause during a transient burst. | E-Swap-4 has 30 independent runs, source rates 1,000/2,000/1,000 over measured boundaries 55 and 65 seconds, and one stateless swap at 60 seconds. Across-run p95 sink gap must be below 100 ms with zero loss and duplication. |
| NFR-SWAP-5 | Recover from a process-time failure. | E-Swap-5 verifies bounded rollback and continuity within the configured canary window. |

A sink-observed gap, HTTP duration, and internal swap phases are separate measurements. No state-preservation claim is made.

## Evidence rules

- Complete process runs are the independent unit for amended N=30 experiments.
- Bootstrap confidence intervals and non-parametric effects use run-level values.
- Canonical analysis rejects incomplete, dirty, mixed-SHA, throttled, malformed, or unapproved batches.
- Raspberry Pi 5 PMIC telemetry is an internal-rail proxy, not total board or USB-C input power.
- Scout, v11-v17, targeted-pilot, laptop, and synthetic fixture data remain diagnostic and are not pooled with final evidence.

## Supporting documents

- [RFC-008 evaluation harness](../rfcs/RFC-008-evaluation-harness.md)
- [Result contract](../../eval/RESULT-CONTRACT.md)
- [Pi 5 experiment runbook](../eval/pi5-experiment-runbook.md)
- [Metering and cache ADR](../adr/0013-aot-cache-and-metering.md)
