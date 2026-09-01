# Benchmarks

Benchmark reports produced from the evaluation harness (`crates/wafer-loadgen`
and `eval/`). Each file documents one measurement: the workload, the hardware,
the method (N, statistics), the raw numbers, and the resulting per-phase
timing.

- `hot-swap.md` — watch-channel hot-swap latency decomposition
  (compile, instantiate, signal, ack, convergence).
- `ekuiper-comparator.md` — reproducible native eKuiper comparator configuration.
- `ekuiper-tail-diagnostic.md` — diagnostic explanation of the unmatched v11
  QoS 0 latency tail and the corrected small-N result.
