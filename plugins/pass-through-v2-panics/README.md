# pass-through-v2-panics — fault-injection plugin

**Do not run this in production.** It is the failing replacement component for
E-Swap-5, the failed-replacement experiment, and for the process-time rollback
tests.

## What it does

Compiled to a `wasm32-wasip2` component that:

- implements the `transform-node` world, like `pass-through-v1` and
  `pass-through-v2`, so the runtime accepts a hot-swap to it;
- succeeds in `validate()` and `init()`;
- panics on the first call to `process()` with the message
  `pass-through-v2-panics 2.0.0-panic: intentional trap for E-Swap-5
  rollback test`.

The panic reaches the host as a Wasm trap inside the Transform canary window.
The runner discards the failing instance, restores the previous version from
its retained `InstancePre`, and keeps serving messages.

## Sentinel

The `Cargo.toml` version is `2.0.0-panic` and the panic message contains the
same string, so a reviewer can grep for the fault injection.

## Where it is used

- `eval/scripts/lib/canonical_runner.py` uses it as the replacement in every
  E-Swap-5 event.
- `crates/wafer-core/tests/hotswap_process_time_rollback.rs` and
  `crates/wafer-core/src/testing/harness.rs` test the rollback and the trap.

Nothing in CI prevents another config from loading it.

## Related

- `docs/adr/0003-hot-swap-mechanism.md`: hot-swap and rollback.
- `docs/benchmarks/hot-swap.md`: the E-Swap-5 measurement.
- `plugins/pass-through-v2/`: the well-behaved counterpart used by the other
  E-Swap experiments.
