# Specifications

This directory is reserved for future feature-level specifications that are too
narrow or implementation-specific for the main documentation tree.

At the moment, there are no active specs here. Agents should treat the current
project documentation under `docs/` as authoritative.

## Relationship to Current Documentation

| Document | Purpose | Use for |
|----------|---------|---------|
| [`docs/architecture/`](../docs/architecture/) | System explanation and architectural views | Understanding how WAFER fits together |
| [`docs/rfcs/`](../docs/rfcs/) | Long-form decision archive | Design rationale and historical decisions |
| [`docs/adr/`](../docs/adr/) | Short Architecture Decision Records | Executive summaries of key decisions |
| [`docs/interfaces/`](../docs/interfaces/) | API / WIT / config references | Exact external contracts |
| [`docs/operations/`](../docs/operations/) | How-to guides | Running, configuring, and operating WAFER |
| [`docs/status/`](../docs/status/) | Factual implementation and drift status | What is built, what is not wired, evaluation progress |
| [`ROADMAP.md`](../ROADMAP.md) | Aspirational backlog | Future work and runtime-migration priorities |

The retired `docs/SPEC.md` and `docs/MVP.md` were deleted during the 2026-07-18
doc-refactor. Their content was redistributed into the directories above.

## Creating Specs

Use this directory only when a feature needs a focused implementation contract
that does not belong in an RFC, ADR, operation guide, or status ledger.

A spec should include:

```markdown
# <Feature Name>

## Summary
One-paragraph description of the feature.

## Current authoritative docs
- Link to relevant RFC / ADR / architecture / status entries.

## Context
Why this feature is needed.

## Requirements
- R1: ...
- R2: ...

## Design
Implementation approach and boundaries.

## Validation
How to prove the feature works.

## Open Questions
- Q1: ...
```

## Current Specs

| Spec | Status |
|------|--------|
| *(none yet)* | — |
