# ADR-0017: Open-Loop Load Generation and Latency Recording

- **Date**: 2026-09-28
- **Status**: Accepted
- **Parent RFC**: [RFC-008](../rfcs/RFC-008-evaluation-harness.md)

## Context

The RQ1 and RQ3 latency numbers come from two recording paths:

- the external MQTT path, where `wafer-loadgen publish` offers messages to a
  broker, the pipeline under test consumes and republishes them, and
  `wafer-loadgen subscribe` records what arrives; and
- the in-process path, where `BenchSource` feeds the pipeline and
  `BenchSink` records at its end.

Both paths must produce latencies that include the time a message waited
because the system (or the load generator itself) fell behind. A closed-loop
or "send when ready" generator hides that wait: when the system stalls, the
generator stalls with it and the delayed messages are timestamped late, so
the stall never appears in the histogram (coordinated omission, Tene 2012).
The recorders also have to survive long runs, clock adjustments, and being
stopped by a signal without silently producing a clean-looking result.

Earlier versions of the harness did not meet this. The publisher slept with
the Tokio timer until each target time and stamped the payload with the time
it actually sent the message. The in-process and external histograms used a
1 µs to 10 s range; `BenchSink` dropped samples above 10 s and the subscriber
clamped them at 10 s without counting them.

## Decision

### Open-loop schedule, stamped with the scheduled time

Message *n* has a target offset fixed by the load shape (steady, burst, ramp,
or hotswap-trigger for the publisher; steady or a burst schedule for
`BenchSource`), independent of whether earlier messages were on time. The
timestamp a message carries is its scheduled send time, not the time it
left:

- The publisher computes `ts` as the wall-clock time at measurement start
  plus the message's scheduled offset. If it falls behind (a full client
  queue, a slow broker), overdue messages go out back to back and their
  delay counts as latency.
- `BenchSource` fixes its schedule origin on the first poll and stamps
  `intended_ns` (scheduled time) and `emit_ns` (when the message actually
  left the source) into the envelope's host-only `BenchStamps`, which Wasm
  guests never see.

Latency is always arrival time minus scheduled time.

### Pacing on an OS thread

Target times are released by a dedicated OS thread (`loadgen-pacer` in the
publisher, `bench-pacer` in `BenchSource`) that sleeps until each due time
and hands the tick to the async sender through a bounded channel. The Tokio
timer rounds every deadline up to the next millisecond, which would add up to
1 ms of lag to every message and release high rates in per-millisecond
bursts. Because the target time comes from the schedule, not from when the
sender picks up the tick, the pacer may run ahead of a blocked sender without
changing any timestamp.

### Source lag is measured, not hidden

How late each message was actually handed off is recorded separately:

- the publisher records the lag after each successful enqueue into a
  histogram and reports it as `source_lag_ns` (count, p50, p99, p999, max) in
  `publisher-summary.json`, together with `deadline_misses`, `intended`,
  `rejected`, and `enqueued` counts;
- `BenchSink` splits end-to-end latency into `service.hdr` (arrival minus
  emit) and `source-lag.hdr` (emit minus scheduled), next to `latency.hdr`
  (arrival minus scheduled).

A run whose source lag approaches its latency is limited by the generator,
and that is visible in the artifacts.

### One histogram range, with clamps counted

Every latency histogram uses the constants in
`crates/wafer-types/src/latency.rs`: 1 µs to 1 h at 3 significant digits. A
sample below zero or above 1 h is recorded at the bound and counted
(`LatencyClamps`). `BenchSink` writes the counts to
`measurement-window.json` (`latency_clamps`); the subscriber writes
`negative_latency_count` and `above_highest_latency_count` to
`subscriber-metadata.json`. A latency longer than an hour means a broken run,
so it is counted rather than dropped.

### Clock handling

In-process timestamps come from a monotonic clock anchored to the wall clock
once per process (`monotonic_unix_ns`), so a wall-clock step cannot change an
in-process latency; `BenchSink` still records how far the wall clock moved
away from it (`wall_clock_step_ns`). The external path subtracts the
publisher's wall-clock schedule from the subscriber's wall-clock receive
time, so a step there would shift every later sample. The subscriber tracks
the gap between its wall clock and its monotonic clock from its own start and
counts a change larger than 100 ms as a step (`clock_steps`). NTP clients
slew small offsets and only step large ones, so a real step is above that
tolerance while a thread delayed between the two clock reads stays below it.

The result verifier (`eval/scripts/verify-result-contract.py`, sharing its
window checks with the canonical runner through
`eval/scripts/lib/latency_evidence.py`) rejects a run with any clamp or clock
step.

### Artifacts and partial runs

The subscriber writes `latency.hdr` (a bare V2 histogram), `sequence.csv`
(gaps and duplicates), optional interval and event-bucket files, and
`subscriber-metadata.json` last, so its presence means the other artifacts
are complete. When a bounded side artifact overflows or fails its checks,
recording continues, the failed file is left out, the metadata is marked
`status: partial` with `partial_reasons`, and the subscriber exits non-zero.
`BenchSink` writes its primary artifacts first, lists any export failure in
`export-errors.json`, and writes every file through a temporary file and a
rename.

The publisher and subscriber handle SIGTERM and SIGINT from the start of the
run: the first signal stops the run and flushes artifacts (the exit reason
records the signal), and a second one exits at once. The verifier rejects
partial subscriber runs and publisher runs stopped by a signal.
`wafer-loadgen hdr-summary` reads both `latency.hdr` encodings (the
`BenchSink` interval log and the subscriber's bare histogram) so the analysis
does not depend on the Python HDR library.

## Consequences

### Positive

- Latency includes queueing caused by a stalled pipeline or a stalled
  generator; coordinated omission cannot hide a stall.
- Generator saturation is diagnosable from `source_lag_ns` and
  `source-lag.hdr` instead of being folded silently into the result.
- In-process and end-to-end latencies share one range and precision, so they
  are directly comparable.
- No sample is silently dropped or clamped; clamps, clock steps, partial
  artifacts, and signal-stopped runs are all recorded and rejected by the
  verifier.
- A run stopped by a signal still leaves its evidence on disk.

### Negative

- The external path depends on the publisher and subscriber wall clocks
  agreeing; the design detects steps but cannot correct them, so an affected
  run is discarded rather than repaired.
- One pacer thread per generator adds an OS thread outside the Tokio runtime.
- A latency above 1 h is recorded as 1 h; the true value is lost (the run is
  rejected anyway).

### Neutral

- The payload `ts` field and the `intended_ns` bench stamp mean "scheduled time";
  readers of older traces must not interpret them as actual send times.

## Alternatives Considered

- **Stamp the actual send time.** The previous publisher did this after
  sleeping until each target. It hides any delay between the scheduled time
  and the send, which is exactly the coordinated-omission error the harness
  must avoid. Replaced by scheduled-time stamps plus a separate source-lag
  measurement.
- **Pace with the Tokio timer.** Rejected because of its millisecond
  rounding, which adds lag and bursts at high rates.
- **1 µs to 10 s histogram range.** The earlier range dropped or silently
  clamped long samples. Replaced by a 1 h bound with counted clamps.

## See Also

- [RFC-008 — Evaluation Harness Design](../rfcs/RFC-008-evaluation-harness.md)
- [`eval/RESULT-CONTRACT.md`](../../eval/RESULT-CONTRACT.md) — artifact fields and verifier rules.
- `crates/wafer-loadgen/src/publish.rs`, `crates/wafer-loadgen/src/recorder.rs`, `crates/wafer-loadgen/src/sub.rs`
- `crates/wafer-core/src/node/source/bench.rs`, `crates/wafer-core/src/node/sink/bench.rs`
- `crates/wafer-types/src/latency.rs`
