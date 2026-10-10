# Benchmarks

Benchmark and comparator documentation for the evaluation harness (`crates/wafer-loadgen` and `eval/`).

Current method/reference documents:

- `hot-swap.md`: final hot-swap measurement boundaries.
- `ekuiper-comparator.md`: native eKuiper comparator configuration.
- `ekuiper-profile-diagnostic.md`: diagnostic eKuiper tail-profiling batch
  (GC trace and `/proc` sampling); never thesis evidence.
- E-Backpressure is defined by `eval/RESULT-CONTRACT.md`: separate `slow`,
  `drop`, and `dead-letter` conditions use policy-specific equations and
  serialize dropped, dead-lettered, downstream-closed, DLQ-full, and DLQ-closed
  counters. There is no universal lossless criterion.

The macOS shakedown and pilot pages (RQ summary, RQ2 attacks, E-Val-1
methodology validation, binary sizes and the v11 eKuiper tail diagnosis) are
archived in [`docs/history/benchmarks/`](../history/benchmarks/).

Only an approved, complete canonical batch may supply final numerical conclusions. Diagnostic files remain separate from final evidence.
