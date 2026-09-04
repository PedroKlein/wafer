# Non-functional requirements

This reference lists the current quantitative requirements. The narrative explanation is in `docs/architecture/07-quality-requirements.md`; the executable definitions are in `eval/canonical-matrix.json`.

## RQ1: performance

| ID | Statement | Criterion | Experiment |
|---|---|---|---|
| NFR-PERF-1 | Typed Wasm boundary cost is bounded. | Median empty pass-through hop < 50 µs on Raspberry Pi 5. | E-Perf-4 |
| NFR-PERF-2 | Matched target-load latency and delivery are bounded against eKuiper. | At 1,000 msg/s: pooled loss <= 1 percent, mean achieved/offered >= 0.99, median WAFER p95 / median eKuiper p95 <= 2.0. | E-Perf-1 |
| NFR-PERF-3 | Gateway capacity is measured on one common grid. | Report delivery ceiling and normalized p99 knee for all systems on `[1,000, 4,000, 8,000, 15,000, 16,000]` msg/s. WAFER/eKuiper competitive ratio >= 0.70 only when identifiable after MQTT censoring. | E-Perf-10 |
| NFR-PERF-4 | Pipeline memory growth is bounded. | Five-node RSS < 150 MB and incremental slope < 10 MB/node. | E-Perf-6 |
| NFR-PERF-5 | Isolation-cost ratios are portable across architectures. | `PENDING` until matched Raspberry Pi 5 and x86 Linux blocks exist. | E-Perf-5 |
| NFR-PERF-6 | Linux filesystem page-cache startup effect is reported honestly. | Cold/warm phase estimates with disk compiled-component cache disabled. No AOT-cache claim. | E-Perf-9 |

E-Perf-1 is a matched operating point, not capacity. Pipeline A is `MQTT source -> threshold filter -> MQTT sink`. E-Perf-10 reports support censoring when MQTT loopback is delivery-bad.

## RQ2: isolation

| ID | Statement | Criterion | Experiment |
|---|---|---|---|
| NFR-ISO-1 | Adversarial components are contained. | All six attacks trap without a process-wide failure. | E-Iso-1 through E-Iso-6 |
| NFR-ISO-2 | A fault does not materially affect an independently sourced healthy branch. | Branch-A throughput drop < 1 percent. | E-Iso-7 |
| NFR-ISO-3 | Guest memory is bounded per store. | 64 MiB Transform and 16 MiB Filter/Router defaults unless overridden. | E-Iso-5 |
| NFR-ISO-4 | Infinite execution is interrupted by the declared mechanism. | Trap and recovery evidence matches the matrix-declared epoch stimulus. | E-Iso-4, E-Iso-8 |

## RQ3: stateless hot-swap

| ID | Statement | Criterion | Experiment |
|---|---|---|---|
| NFR-SWAP-1 | Repeated stateless swap pause is bounded. | p95 sink-observed output gap < 100 ms. | E-Swap-1 |
| NFR-SWAP-2 | Repeated swaps preserve sequence accounting. | Zero loss and duplication. | E-Swap-2 |
| NFR-SWAP-3 | Event-aligned output disruption is bounded. | For 30 WAFER hot-swap runs, upper bootstrap CI for median dip < 5 percent with zero loss and duplication; restart strategies are measured comparators. | E-Swap-3 |
| NFR-SWAP-4 | One swap remains bounded during a true transient burst. | Across-run p95 sink gap < 100 ms with zero loss and duplication over 30 independent 1,000/2,000/1,000 msg/s runs. | E-Swap-4 |
| NFR-SWAP-5 | A process-time failure rolls back within the canary policy. | Pipeline continues and sequence evidence remains lossless. | E-Swap-5 |

## Measurement rules

- Amended N=30 experiments use complete runs as independent units.
- Fuel and epoch limits default to `None`; final WAFER configs enable both explicitly except for declared ablations and attack stimuli.
- PMIC values are Raspberry Pi 5 internal-rail proxy measurements, not total board power.
- Diagnostic and final evidence are never pooled.
