# pass-through-v2-panics — fault-injection plugin

**Do NOT run this in production.** It exists only for RFC-008 E-Swap-5, the
failed-swap-recovery experiment.

## What it does

Compiled to a `wasm32-wasip2` component that:

- exports the standard `pipeline:node/lifecycle` and
  `pipeline:node/transform` interfaces (surface-compatible with
  `pass-through-v1` and `pass-through-v2`), so a hot-swap request pointing
  at this binary is accepted by the runtime up to the moment the first
  message is processed;
- succeeds in `validate()` and `init()`;
- panics on the first call to `process()` with the message
  `pass-through-v2-panics 2.0.0-panic: intentional trap for E-Swap-5
  rollback test`.

The panic surfaces to the host as a Wasm trap, which drives the runtime's
A4 rollback logic: on trap, discard the failing instance, re-materialise
the previous version from its cached `InstancePre`, and continue serving
messages from where v1 left off.

## Metadata sentinel

The `Cargo.toml` version is `"0.2.0-panic"` and the panic message contains
the literal string `2.0.0-panic`. Both are intentional grep markers — a
reviewer skimming `eval/configs/*.toml` should be able to spot the fault
injection at a glance.

## Where it may appear

Only under `eval/configs/*` fixtures consumed by the E-Swap-5 harness. Any
other reference is a bug — CI should reject a `wafer-runtime` config that
loads this binary outside those fixtures.

## Related

- `docs/adr/0003-hot-swap-mechanism.md` — the A4 rollback contract.
- `docs/rfcs/RFC-008-evaluation-harness.md` — E-Swap-5 pass criterion.
- `plugins/pass-through-v2/` — the well-behaved counterpart used by every
  other E-Swap experiment.
