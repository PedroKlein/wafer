# Requirements

Functional and non-functional requirements for WAFER.

- `functional.md` — `FR-<area>-N` requirements (configuration, nodes, messaging,
  errors, hot-swap, control plane, plugin sandboxing), each with a statement and
  verification method.
- `non-functional.md` — NFR-PERF/ISO/SWAP requirements grouped by research
  question (RQ1 performance, RQ2 isolation, RQ3 hot-swap). Thresholds come from
  `verdict_rules` in `eval/canonical-matrix.json`; some requirements are
  report-only.
