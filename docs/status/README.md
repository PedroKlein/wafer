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
backlog for closing gaps is `plan_tasks --plan-name runtime-migration`.
