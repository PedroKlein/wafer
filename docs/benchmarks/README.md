# Benchmarks

Benchmark reports produced from the evaluation harness (`crates/wafer-loadgen`
and `eval/`). Each file documents one measurement: the workload, the hardware,
the method (N, statistics), the raw numbers, and the resulting per-phase
timing.

- `hot-swap.md` — watch-channel hot-swap latency decomposition
  (compile, instantiate, signal, ack, convergence).
