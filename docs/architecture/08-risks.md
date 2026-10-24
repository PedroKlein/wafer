# 08. Risks and technical debt

This page lists the risks and known debt of the runtime architecture. Risks to
the evaluation measurements (MQTT support-path censoring, tested-grid
brackets, event alignment, coordinated omission, power proxies) are handled by
the method in [RFC-008](../rfcs/RFC-008-evaluation-harness.md#measurement-risks)
and the [result contract](../../eval/RESULT-CONTRACT.md).

## R1: the isolation boundary is narrower than "sandboxed"

Each Wasm node gets its own Store, memory limit, and metering, and the six
declared attack scenarios are contained. Outside that boundary:

- side channels, covert channels, and the native sources and sinks (trusted
  code) are not isolated;
- the memory limit covers linear memory and tables, not host-side WASI
  resources a guest creates; and
- fuel and epochs bound Wasm CPU time, not time a guest spends blocked in a
  host import.

## R2: an unconfigured pipeline runs unmetered

Fuel and epoch limits default to `None`. A config that omits `[engine.fuel]`
budgets and `epoch_deadline` lets an infinite-loop guest run unbounded. Every
ordinary config of the final evaluation sets both explicitly, and
`crates/wafer-config/tests/eval_configs_load.rs` checks the evaluation configs.

## R3: replacement telemetry is runner-local

A hot-swap response reports `replacement_adopted` and the first runner-local
outcome. A locally enqueued output can still be delayed or lost downstream;
only sink-side evidence (sequence, gap, throughput) shows delivery.

## R4: guest state does not survive replacement or recovery

A hot-swap, reconfigure, or recovery starts the guest in a new Store, and the
host never calls the guest `close()` export. A plugin that keeps state in
memory or relies on `close()` to flush loses that work. This is the stateless
replacement contract, not a defect.

## R5: fan-in has no ordering or fairness guarantee

Merged producers share one receiver; messages interleave in Tokio scheduling
order. A plugin that needs cross-branch order must impose it itself.

## Technical debt

- The compiled-component cache has a disk tier, but runtime startup builds a
  memory-only cache, and plugins compile one at a time at launch
  ([RFC-007](../rfcs/RFC-007-performance-optimizations.md) Decisions 1 and 10).
- `/metrics` has no rollback counter ([A20](../status/implementation-gaps.md)).
- Unused types remain in `wafer-core`: `AnyNode`, the `Node` wrapper in
  `node/kind.rs`, and the `BoundedQueue` wrapper, which only the throughput
  benchmark uses.

Current implementation state is in
[implementation status](../status/implementation-status.md).
