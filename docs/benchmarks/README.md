# Benchmarks

Benchmark and comparator documentation for the evaluation harness (`crates/wafer-loadgen` and `eval/`).

Method and reference documents:

- `hot-swap.md`: final hot-swap measurement boundaries.
- `ekuiper-comparator.md`: native eKuiper comparator configuration.
- E-Backpressure is defined by `eval/RESULT-CONTRACT.md`: separate `slow`,
  `drop`, and `dead-letter` conditions use policy-specific equations and
  serialize dropped, dead-lettered, downstream-closed, DLQ-full, and DLQ-closed
  counters. There is no universal lossless criterion.

Final numbers come only from the batches approved in [`eval/final-batches.json`](../../eval/final-batches.json) and are reported in the thesis. Diagnostic results stay separate from them.
