# Status

Current implementation and evaluation state — factual reports, no roadmap.

- `implementation-status.md` — what is built, what is tested, per-plugin and
  per-crate coverage. Replaces the legacy `docs/MVP.md`.
- `implementation-gaps.md` — documented behaviour that the runtime does not
  yet implement (drift ledger). Every RFC / ADR / architecture chapter with
  an aspirational banner points here.
- `migration-audit.md` — row-per-decision audit proving that the deleted
  `docs/decisions/*` files were either preserved in RFCs/ADRs/status docs or
  explicitly mapped to known implementation gaps.
- `evaluation-progress.md` — progress against the thesis evaluation plan
  (`tcc-doc/research/analysis/evaluation-plan.md`).

Aspirational items live in the repo-root `ROADMAP.md`; the executable
backlog for closing gaps is tracked in the plan_tasks system.

## Active plans

Run `plan_tasks --list-plans` for the full inventory. Current focus:

- [`plans/thesis-hardening.md`](../../plans/thesis-hardening.md) —
  **9/9 done (closed 2026-08-02)**. Landed: A17 process-time hot-swap
  rollback (T1) + B1/B2/M1/M2 polish, `cargo test --workspace` hang
  fix (T7), A19 runtime-side memory sampler (T4), doc freshness sweep
  (T12), legacy shakedown metadata unification (T8), thesis-grade PDF
  figure pipeline (T9), notebook↔RQ traceability (T11), aarch64-linux
  cross-arch CI (T10).
- [`plans/canonical-runs.md`](../../plans/canonical-runs.md) —
  Pi 4 + Jetson preflight and canonical-run execution against the
  RFC-008 evaluation harness. C1 (aarch64 cross-compile spike) and
  C2 (cross-build loadgen + waferctl) closed 2026-08-02 via
  `mise run cross-build-pi` (docker linux/arm64). Next: C3 plugin
  portability check, then Pi hardware preflight.
