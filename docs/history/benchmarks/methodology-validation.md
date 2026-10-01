# E-Val-1: Methodology validation

> **Archived, not current.** Shakedown E-Val-1 results.
> Kept as a historical record only. Do not use it for decisions, commands,
> paths or numbers. Current source: [the Pi 5 runbook](../../eval/pi5-experiment-runbook.md).

<!-- historical-diagnostic-file -->

**Purpose.** Prove the measurement rig captures a known-magnitude latency.
Injects a 50 ms delay via a Wasm plugin; asserts recorded p99 lands within
[45, 55] ms. A p99 outside that window means the rig is lying about tail
latency and every downstream RQ1 / RQ2 / RQ3 number is suspect.

**Status.** ✅ Honesty gate holds on macOS shakedown.

## Shakedown result (2026-07-22, macOS)

| Metric                         | Value                          |
|--------------------------------|--------------------------------|
| Host                           | shakedown-macos                |
| Runs                           | 5                              |
| Runs passing gate              | 5 / 5                          |
| Injected delay                 | 50 ms                          |
| Observed p50 range (across runs) | 52.10 – 52.26 ms             |
| Observed p99 range (across runs) | 52.99 – 53.35 ms             |
| Overhead (observed p50 − 50)   | ~2.2 ms                        |
| Honesty invariant              | PASS (p99 ∈ [45, 55])          |

Evidence: `eval/results/e-val-1/shakedown-macos-2026-07-22T16-19-29Z/`.

These are pre-fix diagnostic numbers: they predate the open-loop schedule
(latency from each message's scheduled send time, origin at the source's first
poll), the 1 µs to 1 h histogram range, and the pinned release profile (thin
LTO, one codegen unit). They are not comparable with runs of the current build.

## Overhead attribution

The ~2.2 ms gap between injected 50 ms and observed p50 52.2 ms decomposes
into three sources:

1. **`std::thread::sleep` granularity on wasip2.** Lowers to
   `wasi:clocks/monotonic-clock.subscribe-duration` + `wasi:io/poll.poll`;
   the WASI adapter cannot guarantee wake-up better than the host's OS
   scheduler minimum (macOS: ~1 ms typical).
2. **Message-passing overhead.** BenchSource → transform channel →
   transform → sink channel → BenchSink. Each hop is bounded MPSC send
   + recv; ~10-50 µs each on macOS.
3. **BenchSink measurement point.** Latency reference is
   the `intended_ns` stamp (source's scheduled publish time). If the source's
   actual publish lags its scheduled time, that lag rolls into the observed
   latency. The sink records the lag on its own in `source-lag.hdr`.

The 2.2 ms overhead is **stable across runs** (range: 52.10 – 52.26 ms p50,
0.16 ms wide) — this is the honesty invariant working: whatever the rig
adds, it adds consistently.

## Config

`eval/configs/pipeline-c-with-delay.toml`:

- **Source rate:** 10 msg/s. Sits well below sink capacity (1 s / 50 ms =
  20 msg/s) so the queue never back-pressures. Recorded p99 reflects only
  injected delay, not queue-wait.
- **Total messages:** 300 (250 recorded post-warmup).
- **Delay:** 50 ms via `plugins/delay-injector/` (wraps `std::thread::sleep`).
- **Sink:** BenchSink with `track_sequences = true`, no hot-swap.

## Pi tuning notes

For the canonical Pi run:

1. **Rate/delay ratio must hold.** Source rate < 1000/delay_ms always.
   If canonical Pi tightens the window (e.g. [48, 52]), the rate ceiling
   scales the same way — nothing to change here.
2. **Epoch deadline.** The runtime default is unmetered
   (`EngineConfig.epoch_deadline` unset; ticks every `epoch_tick_ms = 10`
   ms). A configured deadline counts those ticks; a 50 ms sleep needs ~5
   of them, so any deadline of a second or more leaves this experiment
   unaffected. The sleep itself is a host call (`wasi:io/poll`), and time
   spent blocked in a host import is not interrupted by epochs.
3. **CPU affinity / isolcpus.** macOS shakedown numbers include OS jitter.
   On Pi, `isolcpus` + `taskset` on the runtime process are expected to
   narrow p99 further (macOS observed range was 0.36 ms wide; expect
   ~50 – 200 µs on Pi with CPU pinning).

## How to reproduce

```sh
# On the Pi, after deploy and preflight (see docs/eval/pi5-experiment-runbook.md)
./eval/scripts/run-rpi5-validation.sh
# Expected: p99 inside the 45-55 ms honesty window
```

The canonical `e-val-1` condition repeats the same check inside every
campaign batch.

Any FAIL means the rig is measuring dishonestly — investigate before
trusting other numbers. See `docs/status/implementation-gaps.md` A16 for
one such investigation (WASI async panic that would have gone undetected
without this shakedown).

## Related

- **RFC-008 §D9** — measurement infrastructure design.
- **crates/wafer-core/tests/wasi_async_runner.rs** — CI regression test for
  the same runtime path using a median injected-delay check; the canonical
  Raspberry Pi gate retains the p99 invariant.
- **docs/status/canonical-readiness.md** — E-Val-1 readiness row.
- **plans/evaluation-infrastructure/plan.json** — P3.1 task.
