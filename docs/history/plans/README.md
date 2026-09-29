# Historical planning scratchpads

> **Archived, not current.** Planning scratchpads from the implementation phase.
> Kept as a historical record only. Do not use them for decisions, commands,
> paths or numbers. Current source: [docs/README.md](../../README.md).

These files are archived planning notes from the thesis implementation
phase. They are frozen documents that capture the state of the plans at
the point they closed, not living guidance.

If you want to understand what problems the runtime worked through
during development, read them. If you want to know how the runtime
behaves today, read `docs/architecture/` and `docs/operations/` instead.

## Contents

- `thesis-hardening.md` — closed 2026-08-02. The final push to bring
  the runtime up to the shape the thesis defends.
- `canonical-runs.md` — pre-final Raspberry Pi 4 and Jetson experiment
  preflight and execution plan.
- `eval-followups.md` — follow-up work identified during the initial
  evaluation shakedown.

## Why they live in the repository at all

They document decisions that shaped the code and are referenced from
several `docs/status/` and `docs/adr/` files. Deleting them would break
those breadcrumbs.
