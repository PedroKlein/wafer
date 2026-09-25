# Benchmarks

Benchmark and comparator documentation for the evaluation harness (`crates/wafer-loadgen` and `eval/`).

Current method/reference documents:

- `hot-swap.md`: final hot-swap measurement boundaries and historical diagnostic context.
- `ekuiper-comparator.md`: native eKuiper comparator configuration.
- E-Backpressure is defined by `eval/RESULT-CONTRACT.md`: separate `slow`,
  `drop`, and `dead-letter` conditions use policy-specific equations and
  serialize dropped, dead-lettered, downstream-closed, DLQ-full, and DLQ-closed
  counters. There is no universal lossless criterion.

Explicitly historical or diagnostic documents:

- `rq-summary.md`: macOS shakedown observations; no final RQ verdict.
- `ekuiper-tail-diagnostic.md`: unmatched v11 QoS diagnosis and corrected small-N checks.
- `methodology-validation.md`: local E-Val-1 shakedown.
- `binary-sizes.md`: static diagnostic comparison at its recorded source.
- `rq2-attacks.md`: local containment checks, not the final N=30 evidence.

Only an approved, complete canonical batch may supply final numerical conclusions. Diagnostic files remain separate from final evidence.
