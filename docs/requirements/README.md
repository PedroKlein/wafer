# Requirements

Functional and non-functional requirements for WAFER.

- `functional.md` — `FR-<area>-N` requirements (configuration, nodes, messaging,
  errors, hot-swap, control plane, plugin sandboxing), each with a statement and
  verification method.
- `non-functional.md` — NFR-PERF/ISO/SWAP requirements grouped by evaluation
  question (performance, fault containment, hot swap). Thresholds come from
  `verdict_rules` in `eval/canonical-matrix.json`; some requirements are
  report-only.
