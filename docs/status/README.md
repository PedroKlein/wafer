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

- [`docs/history/plans/thesis-hardening.md`](../history/plans/thesis-hardening.md) —
  **9/9 done (closed 2026-08-02)**. Landed: A17 process-time hot-swap
  rollback (T1) + B1/B2/M1/M2 polish, `cargo test --workspace` hang
  fix (T7), A19 runtime-side memory sampler (T4), doc freshness sweep
  (T12), legacy shakedown metadata unification (T8), thesis-grade PDF
  figure pipeline (T9), notebook↔RQ traceability (T11), aarch64-linux
  cross-arch CI (T10).
- [`docs/status/rpi5-canonical-transition.md`](rpi5-canonical-transition.md) —
  active pre-measurement decision record for Raspberry Pi 5 4 GB, native
  eKuiper 2.1.0, and canonical CPU allocation.
- [`docs/history/plans/canonical-runs.md`](../history/plans/canonical-runs.md) —
  historical Pi 4 + Jetson plan. Its Pi 4 and Docker assumptions are
  superseded by the Pi 5 transition record; its completed ARM64 build work
  remains applicable.
