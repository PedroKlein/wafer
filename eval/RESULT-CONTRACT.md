# Result-directory contract

Every WAFER experiment run produces one self-contained raw leaf beneath the
selected results layout. Repository-local tests use
`eval/results/<experiment-id>/<host-tag>-<UTC-timestamp>/`; v6 campaign runs use
`<results-root>/raw/<experiment-id>/<host-tag>-<batch-id>/...`. This document is
the authoritative manifest for both layouts.

## Path convention

Repository-local tests retain the historical raw root:

```
eval/results/
└── <experiment-id>/                    ← e.g. e-perf-4, e-swap-1, dry-run
    └── <host-tag>-<UTC-timestamp>/     ← e.g. shakedown-macos-2026-07-21T14-30-15Z
        ├── config.toml
        ├── metadata.json
        ├── stdout.log
        ├── latency.hdr
        ├── throughput.csv
        ├── memory.csv
        ├── per_node_metrics.csv
        ├── queue-depth.csv             ← E-Backpressure
        ├── backpressure.json           ← E-Backpressure
        ├── interval-latency.json       ← bounded producer fragment for timed runs
        ├── interval-metrics.json       ← composed one-second summaries for timed runs
        ├── sequence.csv                ← where applicable
        ├── swap_requests.json          ← API-side hot-swap timing
        ├── swap_timeline.json          ← sink-observed output transitions
        ├── hotswap-analysis.json       ← matched internal/output-gap evidence
        ├── branch-a/                   ← E-Iso-7 only
        │   ├── latency.hdr
        │   ├── throughput.csv
        │   ├── sequence.csv
        │   └── measurement-window.json
        └── branch-b/                   ← E-Iso-7 only; same branch-local files
```

Prospective v6 runs pass an explicit results root. The approved volume layout is:

```
<results-root>/
├── raw/<experiment-id>/<host-tag>-<batch-id>/...
├── manifests/canonical-batches/<host-tag>-<batch-id>/...
├── manifests/aliases/<experiment-id>/<host-tag>-<batch-id>/...
├── derived/
└── reports/
```

The runner accepts `--results-root PATH` or `WAFER_RESULTS_ROOT`; a CLI value wins.
No platform path is inferred. `/mnt/wafer-results` on Pi/Jetson and
`/Volumes/WAF_RESULTS` on macOS are documented operator mount paths. An explicit
root must already exist and be a mounted filesystem before the runner creates any
managed directory. Paths containing spaces are supported. Stored paths are relative
to the volume root.

Generated path segments use a conservative exFAT-safe ASCII set. The layout rejects
reserved DOS names, separators, control characters, trailing dots or spaces,
case-fold/Unicode-normalization collisions, symlinks, and hardlinked evidence.
Temporary files used for atomic receipts live beside their destination; an existing
immutable destination or a cross-device rename fails instead of falling back to a
copy.

Raw attempts are additive. A failed or interrupted attempt remains in place and the
next attempt uses the next numeric suffix. A passed terminal receipt makes the leaf
immutable. Analysis resolves raw inputs through the same results root and may create
outputs only below `derived/` or `reports/`; output traversal or overlap with raw is
rejected.

Shared canonical views do not create another raw tree. A small alias receipt under
`manifests/aliases/` records the volume-relative source leaf, source status digest,
and shared sample identity. Consumers validate and dereference that receipt to the
single source leaf. Copies, symlinks, and hardlinks are not alias mechanisms.
Historical repository-local result trees remain unchanged.

**Host tags** (explicit whitelist — the harness rejects anything else):

- `shakedown-macos` — MacBook laptop shakedown.
- `rpi5` — Raspberry Pi 5 4 GB canonical run.
- `rpi4` — retained for existing Raspberry Pi 4 result directories; not the active canonical target.
- `jetson` — Jetson Orin Nano inference validation.
- `x86` — x86 workstation cross-architecture validation.

**Timestamp format**: `YYYY-MM-DDTHH-MM-SSZ` — UTC, colons replaced with
hyphens so the path is `mv`-safe on every filesystem.

## Manifest

The result-directory contract is split into **core artefacts**
(mandatory on every run) and **per-experiment optional artefacts**
(present only when the experiment's semantics require them). See
`docs/decisions/eval-result-contract-scope.md` for the option A vs B
rationale.

### Core artefacts (mandatory)

Every `run-experiment.sh` invocation produces these regardless of
experiment:

| File | Producer | Description |
| --- | --- | --- |
| `config.toml` | `run-experiment.sh` copies the input file | Verbatim copy of the pipeline config used for this run. |
| `metadata.json` | `run-experiment.sh` + wafer-runtime provenance merge | Run-provenance JSON — see [metadata.json schema](#metadatajson-schema). |
| `runtime-provenance.json` | `wafer-runtime` (F2) | Runtime-owned provenance sidecar (wasmtime, plugin hashes, config sha256, rustc, kernel, runtime sha). Merged into `metadata.json` by `_write_metadata`. |
| `stdout.log` | `run-experiment.sh` tees runtime output | Complete stdout+stderr of `wafer-runtime` including trap traces and hot-swap events. |

### Per-experiment optional artefacts

Present only when the experiment's script writes them; consumers
(notebooks, canonical-run analysis) MUST guard on `os.path.exists`
before reading. The matrix below is authoritative:

| File | Experiments | Producer | Description |
| --- | --- | --- | --- |
| `latency.hdr` | Every experiment with a BenchSink or `wafer-loadgen subscribe` (E-Val-1, E-Perf-1..10, E-Backpressure, E-Iso-*, E-Swap-*) | `BenchSink` (in-process) or `wafer-loadgen subscribe` (E2E) | HdrHistogram V2 latency in nanoseconds. In-process: per-hop source-to-sink. E2E: MQTT publish → MQTT consume with intended-publish timestamp (avoids coordinated omission). |
| `interval-latency.json` | Timed runs with latency output | `BenchSink` or `wafer-loadgen subscribe` | Bounded producer fragment containing one-second latency histograms reduced to p50/p95/p99 and event/unique/duplicate counts. This is an implementation input to the composer, not a replacement for `latency.hdr`. |
| `interval-metrics.json` | Timed runs with latency or throughput output | `canonical_runner.py` or `interval_metrics.py` | Bounded one-second rows combining latency, throughput, CPU, RSS, PMIC internal-rail proxy, temperature, and queue observations. Missing observations use an explicit `unavailable` status and reason. |
| `throughput.csv` | Same as `latency.hdr` | `BenchSink` or `wafer-loadgen subscribe` | Periodic throughput samples: `timestamp_ns,messages_per_sec,total_messages`. |
| `sequence.csv` | Loadgen with sequence tracking, or `BenchSink.track_sequences = true` (E-Perf-1..3, E-Perf-8, E-Perf-10, E-Backpressure, E-Swap-*) | `wafer-loadgen subscribe` / `BenchSink` | Producer-specific sequence accounting. `BenchSink` writes one summary row (`total_expected,total_received,gap_ranges,gap_msgs,duplicates_count`); for `BenchSource` input, its `bench.warmup` marker makes this the exact post-warmup population. `wafer-loadgen subscribe` writes long-form gap/duplicate events (`event_type,seq_start,seq_end,count`), with zero data rows when lossless; its totals live in `subscriber-metadata.json`. |
| `memory.csv` | E-Perf-6, E-Perf-7, E-Perf-8, E-Backpressure (and any run with `WAFER_BENCH_OUTPUT_DIR` set) | `wafer-runtime` via `MemoryRecorder::sample_loop` (1 Hz, cross-platform via `memory-stats` crate). Flush on graceful shutdown. | 1 Hz process-RSS timeline of `wafer-runtime`: `elapsed_ms,rss_bytes`. Runtime-owned since A19 closure (thesis-hardening T4). Legacy harness `ps` polling removed. |
| `memory-clock.json` | Runs with `memory.csv` | `wafer-runtime` | Pairs the memory recorder's monotonic zero with one Unix-epoch anchor for cross-process interval composition. |
| `per_node_metrics.csv` | All experiments (emitted on graceful shutdown when `WAFER_BENCH_OUTPUT_DIR` set) | `wafer-runtime` via `PipelineOrchestrator::export_per_node_metrics` | One row per pipeline node: `node_id,messages_in,messages_out,traps_total,error_state_seconds,recovery_count`. Runtime-owned since A19 closure (thesis-hardening T4). |
| `queue-depth.csv` | E-Backpressure, when `WAFER_QUEUE_DEPTH_OUTPUT` is set | `wafer-runtime` via `QueueDepthRecorder` | Bounded internal Tokio queue samples at 10 ms intervals: `elapsed_ns,queue,depth,capacity,accepted,dequeued,processed`. Counters are internal pipeline observations; broker backlog is excluded. Collection is capped at 131,072 rows and reports truncation in the runtime log. |
| `queue-depth-clock.json` | Runs with `queue-depth.csv` | `wafer-runtime` | Pairs the queue recorder's monotonic zero with one Unix-epoch anchor for cross-process interval composition. |
| `backpressure.json` | E-Backpressure | `canonical_runner.py` | Queue threshold crossing and recovery, offered/accepted/processed/drained rates in messages per second, loss/duplication accounting, and peak-RSS bound. A high offered rate without measured occupancy is classified `not-saturated`. |
| `recovery.csv` | E-Iso-8 and any run with recovery events | `wafer-runtime` via `PipelineOrchestrator::export_per_node_metrics` | Exact recovery samples: `node_id,sample_index,duration_ns`. Retains nanosecond precision for trap-to-running percentiles. |
| `measurement-window.json` | Canonical Pi 5 runs | `BenchSink` for in-process runs with post-warmup output; canonical harness for external MQTT subscribers and zero-output containment runs | Exact `started_ns` and `finished_ns` bounds for excluding warmup and teardown from PMIC energy integration. |
| `pi-telemetry.csv` | Canonical Pi 5 runs | `eval/scripts/lib/pi_telemetry.py` | Timestamped temperature, CPU frequency, governor, throttling state, and summed PMIC internal-rail proxy watts. |
| `pmic-rails.csv` | Canonical Pi 5 runs | `eval/scripts/lib/pi_telemetry.py` | Long-form named PMIC rail voltage/current/power samples. Rails without both voltage and current are not included. |
| `power-boundary.json` | Canonical Pi 5 runs | `eval/scripts/lib/pi_telemetry.py` | Declares that telemetry is an internal-rail proxy, not total USB-C input power, and records excluded consumers and the source limitation. |
| `published.csv`, `received.csv` | Historical focused E-Perf-10 diagnostics only | `wafer-loadgen` opt-in tracing | Raw publisher/subscriber timestamp and sequence samples retained for v11-v17 compatibility. Final E-Perf-10 rejects these mandatory traces and uses bounded summaries. |
| `publisher-summary.json` | Final E-Perf-10 | `wafer-loadgen publish` | Bounded `intended`, `rejected`, and `enqueued` counters plus measured duration. `intended = rejected + enqueued` is mandatory. |
| `subscriber-metadata.json` | Final E-Perf-10 and external-MQTT experiments | `wafer-loadgen subscribe` | Bounded receive, duplicate, ignored-warmup, unexpected-sequence, parse, latency, and sequence totals. The HDR count and received-event population must match. |
| `capacity-run.json` | Final E-Perf-10 and candidate E-Perf-Capacity-Knee | `canonical_runner.py` | Trace-free run summary containing intended/rejected/enqueued/received/lost/duplicate counts, offered and achieved rates, loss, p50/p95/p99, CPU, RSS, thermal state, process/config receipts, and controlled factors. Final E-Perf-10 sets `thesis_evidence=true`; capacity-knee sets `evidence_class=candidate-supplementary`, `thesis_evidence=false`, and `n30_admitted=false`. |
| `resource-usage.csv` | E-Perf-10 and E-Perf-Capacity-Knee | `canonical_runner.py` | One-second SUT samples: wall-clock timestamp, aggregate process CPU ticks, RSS bytes, and process count. MQTT loopback records the explicit no-SUT zero baseline. |
| `process-audit.json` | E-Perf-10 and E-Perf-Capacity-Knee | `canonical_runner.py` | Active SUT PID, process affinity, exclusivity, and allowed CPU set captured before measurement. |
| `rate-sweep.json` | Historical focused E-Perf-10 diagnostics only | `canonical_runner.py` | Legacy trace-backed run summary, always `thesis_evidence=false`. It remains readable but is not accepted as a final-capacity leaf. |
| `throughput-buckets.json` | Final E-Swap-3 and E-Swap-4 | `wafer-loadgen subscribe` / `BenchSink` / `canonical_runner.py` | Contiguous 100 ms output-rate buckets with unique/event/duplicate counts. E-Swap-3 bins exactly -10 s through +10 s around the actual disruption event. E-Swap-4 keeps 1,200 source-origin primary buckets over `[0,120s)` and a separate 100-bucket drain series over `[120s,130s)`, with O(1) after-drain evidence. Unix-epoch values are labeled for source/sink or cross-process alignment only. |
| `throughput-buckets-10ms.json` | Final E-Swap-3 and E-Swap-4 | `wafer-loadgen subscribe` / `BenchSink` | Exactly 400 contiguous 10 ms buckets over `[-2s,+2s)` around actual `t0`, plus 40 nested actual-t0-aligned 100 ms parent buckets. Both levels carry unique/event/duplicate counts and reconcile exactly. E-Swap-3 parent buckets also match the canonical event-window slice; E-Swap-4 retains its separate source-origin primary/drain series as the loss-accounting authority. No message identity or timestamp trace is retained. |
| `disruption-timeline.json` | Final E-Swap-3 | `canonical_runner.py` | Strategy, actual Unix-epoch action start/end for cross-process alignment, monotonic scheduling/duration labels, the scheduled t=60 target, measured offset, signed alignment error, and a 10 ms maximum alignment tolerance. The wall-clock action end is retained independently rather than synthesized from the monotonic duration. |
| `burst-source-timing.json`, `burst-source-summary.json` | Final E-Swap-4 | `BenchSource` | Source-origin timestamp, monotonic source-completion offset, fixed phase boundaries, and exact intended/emitted populations for the 1,000 to 2,000 to 1,000 msg/s burst. |
| `burst-timeline.json` | Final E-Swap-4 | `BenchSource` / `canonical_runner.py` | One 1,000 to 2,000 to 1,000 msg/s burst with boundaries at measured seconds 55 and 65 and exactly one successful swap scheduled at second 60, plus actual alignment, phase populations, primary/drain completion evidence, sequence integrity, sink gap, and internal swap phases. |
| `startup-preparation.json` | E-Perf-9 | `run-experiment.sh` | Filesystem-cache condition and preparation action completed before the timed runtime process starts. |
| `startup.json` | E-Perf-9 | `wafer-runtime` | Monotonic process/config, component load/compile, instantiation, pipeline setup, and first-process durations; total startup duration; exactly-one-message proof; plugin SHA-256; and explicit compiled-component cache state. |
| `branch-a/`, `branch-b/` | E-Iso-7 | `BenchSink` | Independent post-warmup latency histogram, throughput series, sequence accounting, and measurement window for each branch. Each branch has its own `BenchSource`; root-level fan-out/fan-in measurements are forbidden for branch-impact analysis. |
| `branch-isolation.json` | E-Iso-7 | `canonical_runner.py` | Branch-local source identity, configured post-warmup target count, actually offered/received post-warmup counts, target shortfall, throughput samples, latency percentiles, measurement boundaries, and explicit units. |
| `branch-isolation-summary.json` | E-Iso-7 batch ledger | `canonical_runner.py` | Separate branch-A throughput-drop and p95-latency-increase rows for panic and epoch-loop attacks, including run counts and units. |
| `host-load-ladder.json` | E-Host-Thermal-Storage | `characterize-rpi5-host.sh` | Append-only clean-boot session receipt with the exact eight-phase order, per-phase pass/fail/not-run status, 75 °C stop limit, boot identity, bounded sample counts, diagnostic/final admission decisions, source state, and the PMIC internal-rail boundary. |
| `host-telemetry.csv` | E-Host-Thermal-Storage | `characterize-rpi5-host.sh` | One-second phase-labeled temperature, CPU frequency, throttling, PMIC internal-rail proxy, memory availability/pressure, USB throughput, boot ID, wall-clock, and monotonic samples. No per-message data. |
| `kernel-io.log`, `usb-integrity.json` | E-Host-Thermal-Storage | `characterize-rpi5-host.sh` | Bounded matching kernel I/O errors and per-USB-phase byte/duration/SHA-256 reconciliation. Any recorded kernel I/O error or hash mismatch stops the ladder and blocks final admission. |
| `ekuiper-runtime-summary.json` | E-Compare-eKuiper-Profile | `canonical_runner.py` | Diagnostic run identity, interval alignment, latency percentiles, bounded external `/proc` process summary when available, and explicit eKuiper 2.1.0 GC/runtime-unavailability status. It never infers GC events from RSS or latency. |
| `profiler-overhead.json` | E-Compare-eKuiper-Profile | `canonical_runner.py` | Profiler state, matched rate/run pair, collection-enabled flag, and the run-level profiled-minus-control estimator label. It declares association-only interpretation and is not primary evidence. |
| `swap_requests.json` | E-Swap-1, E-Swap-2, E-Swap-4, E-Swap-6 | `canonical_runner.py` HTTP client | One record per API request with wall-clock request boundaries, monotonic `request_duration_ns`, HTTP status, and the runtime's internal `compile_ns`, `instantiate_ns`, `signal_ns`, `ack_ns`, and `convergence_ns` phases. |
| `swap_timeline.json` | E-Swap-1..6 (any config that exercises at least one hot-swap) | `BenchSink` | Sink-observed plugin-version transitions. Each `pause_ns` is an output interarrival gap and is not an internal swap duration. |
| `hotswap-analysis.json` | E-Swap-1, E-Swap-2, E-Swap-4, E-Swap-6 | `canonical_runner.py` | Index-matched API and sink observations with explicit `*_ns` names: internal phases, `http_total_ns`, and `sink_observed_output_gap_ns`. Includes the unique measurement source leaf so shared E-Swap-2/6 views do not multiply samples. |
| `summary.json` | E-Val-1 only | `run-e-val-1-shakedown.sh` | Gate-pass summary across runs (p99 range, honesty-window check). Bespoke to the honesty-gate methodology; not consumed by canonical analysis. |

### Ownership summary

- `wafer-runtime` owns `runtime-provenance.json`, E-Perf-9 `startup.json`,
  `latency.hdr`, `throughput.csv`, and bounded `interval-latency.json` (via
  `BenchSink`), `sequence.csv` (via `BenchSink` when `track_sequences=true`),
  sink-observed `swap_timeline.json`,
  `memory.csv` and `memory-clock.json` (via `MemoryRecorder`), `queue-depth.csv`
  and `queue-depth-clock.json` (via `QueueDepthRecorder`),
  `recovery.csv` (exact recovery samples), and `per_node_metrics.csv`
  (via `PipelineOrchestrator::export_per_node_metrics`).
- `wafer-loadgen subscribe` owns `subscriber-metadata.json`,
  `latency.hdr` (E2E path), `sequence.csv`, bounded `interval-latency.json`,
  bounded `throughput-buckets.json`, and historical opt-in `received.csv` traces.
- `wafer-loadgen publish` owns `publisher-summary.json` and historical opt-in
  `published.csv` traces.
- `canonical_runner.py` owns final E-Perf-10 `capacity-run.json`,
  `resource-usage.csv`, `process-audit.json`, and the composed `interval-metrics.json`;
  historical diagnostics retain
  `rate-sweep.json`. It also owns final E-Swap `disruption-timeline.json` and
  `burst-timeline.json`, E-Backpressure `backpressure.json`,
  E-Swap `swap_requests.json` and `hotswap-analysis.json`, plus E-Iso-7
  `branch-isolation.json` and the batch-level branch-A impact summary.
- `run-experiment.sh` and the per-experiment shakedown scripts own
  `metadata.json`, `config.toml`, `stdout.log`, and E-Perf-9
  `startup-preparation.json`.

### Final amended contract

The `final_campaign` object in `eval/canonical-matrix.json` is the executable source of truth. It fixes seed 1729, explicit fuel-plus-epoch metering, eKuiper concurrency 1, the five-rate common capacity grid, 2,105 schedule records, and 1,893 executed or static measurement leaves. Every final experiment has `thesis_evidence=true`; diagnostic and focused entries remain in the separate `focused_pilot` object with `thesis_evidence=false`.

Final E-Perf-5 uses explicit transform fuel and epoch protection. Its transform-only pipeline records `filter = null` and `router = null` because those node categories are absent; this is a declared matrix exception, not an unmetered WAFER run.

Final E-Perf-10 requires `capacity-run.json`, `publisher-summary.json`, `subscriber-metadata.json`, `latency.hdr`, `throughput.csv`, `sequence.csv`, `resource-usage.csv`, and `process-audit.json`. `capacity-run.json` uses this counter identity:

```text
intended = rejected + enqueued
enqueued = received_unique + downstream_lost
received_events = received_unique + duplicates
total_undelivered = rejected + downstream_lost
```

It must contain no mandatory per-message traces. `check_capacity_run_result` rejects missing fields, inconsistent counters, non-final evidence labels, unexpected sequences, or trace mode.

Final E-Swap-3 requires 200 contiguous 100 ms buckets in `throughput-buckets.json` aligned to the actual action-start `t0`. It additionally requires 400 contiguous 10 ms buckets and 40 nested 100 ms parent buckets over `[-2s,+2s)` in `throughput-buckets-10ms.json`; the parent buckets equal the matching canonical event-window slice. `disruption-timeline.json` records the actual action start immediately before control issuance, the actual acknowledged/readiness wall-clock end, independent monotonic action duration, the scheduled measured t=60 target, actual event offset, signed alignment error, and a fixed 10 ms tolerance. The leaf fails when the actual action start misses the scheduled target by more than 10 ms.

Final E-Swap-4 uses a source-driven 1,000→2,000→1,000 msg/s schedule over measured intervals `[0,55)`, `[55,65)`, and `[65,120)` after 30,000 warmup messages. Each independent run contains exactly 130,000 measured messages and one swap scheduled at measured t=60. `throughput-buckets.json` contains 1,200 contiguous source-origin 100 ms primary buckets over `[0,120s)` plus a separate 100-bucket drain series over `[120s,130s)`. Full-run counters reconcile primary, drain, and O(1) after-drain evidence; any after-drain receive or right-censored drain rejects the run. `burst-timeline.json` records source and actual swap boundaries, monotonic source-completion offset, intended/emitted/received phase populations, sequence loss/duplication, primary/drain completion evidence, the sink-observed gap, and internal swap phases. The semantic verifier rejects wrong clocks, origins, phase rates or populations, zero or multiple swaps, a swap outside the burst, a non-centered scheduled swap, bucket gaps, clamped/misclassified tail evidence, completion at or after 130 s, population mismatches, loss, or duplication. Batch analysis admits exactly one event from each of 30 distinct runs before computing the across-run p95 and bootstrap median interval, and separately reports runs with drain arrivals and the maximum drain offset. Missing required files fail through the matrix contract; malformed files fail through experiment-specific semantic checks.

Final E-Density-1 is a static source-bound measurement. The canonical runner invokes `eval/scripts/collect-binary-sizes.sh` instead of `run-experiment.sh`, requires one `binary-sizes.csv` row for every non-comment entry in `eval/scripts/binary-sizes.index`, and records Pi host, tag, governor, throttling, telemetry, and measurement-window evidence. Its metadata uses `system = "static"` and `exit_codes.collector = 0`; runtime/Wasmtime provenance is intentionally inapplicable because no WAFER runtime process executes.

### v6 enhanced candidate contract

The signed v4 final campaign remains immutable. The signed v5 candidate is retained
as a rejected pre-format artifact because its volume label exceeds exFAT's 11
UTF-16 code-unit limit. Enhanced work uses the separate `enhanced_candidate` object
in `eval/canonical-matrix.json` and the corrective v6 release lineage. It does not
modify or supersede the existing primary estimands.

Every enhanced experiment is either `candidate-supplementary` or diagnostic,
sets `thesis_evidence=false` and `n30_admitted=false`, and remains outside the
final campaign until a post-rehearsal selection receipt records an explicit
include, defer, or reject disposition. The expanded N=5 rehearsal is diagnostic;
its runs, attempts, intervals, and events must not be pooled with the signed v4
campaign, prior rehearsals, or one another as independent replicates. The
independent sample unit is the host run unless the matrix explicitly declares a
clean-boot host session. Intervals and events are nested observations.

The candidate IDs and purposes are:

| ID | Evidence | Independent sample unit | Candidate purpose |
| --- | --- | --- | --- |
| `e-perf-capacity-knee` | candidate-supplementary | Host run at one system and offered rate | Refine the support and SUT capacity knees without replacing E-Perf-10. |
| `e-perf-payload-refinement` | candidate-supplementary | Host run at one payload size | Refine the observed payload transition. |
| `e-perf-depth-extension` | candidate-supplementary | Host run at one depth | Extend latency and RSS observations through depth 50. |
| `e-swap-independent-sessions` | candidate-supplementary | Host run | Collect five independent swap sessions with 50 nested events each. |
| `e-swap-rollback-sessions` | candidate-supplementary | Host run | Collect five independent rollback sessions with 50 nested events each. |
| `e-host-thermal-storage` | diagnostic | Clean-boot host session | Characterize thermal and removable-storage headroom under a stop-on-failure load ladder. |
| `e-compare-ekuiper-profile` | diagnostic | Host run at one rate and profiler state | Associate bounded eKuiper runtime/process summaries with tail latency using matched unprofiled controls. |

The exact condition grids, required outputs, no-pooling boundaries, and analysis
consumers are machine-readable in `enhanced_candidate.experiments`. The
architecture verifier rejects missing IDs, changed grids or sample units,
undeclared outputs, or candidate promotion before selection.

The `e-perf-capacity-knee` N=5 schedule uses MQTT-loopback rates from 4,000
through 15,000 msg/s in 1,000 msg/s steps plus 15,250, 15,500, 15,750, and
16,000 msg/s; native and WAFER rates from 8,000 through 15,000 msg/s in 1,000
msg/s steps; and eKuiper rates from 4,000 through 8,000 msg/s in 1,000 msg/s
steps. Each system-rate cell has five independent host runs. Seed 1729 fixes a
non-monotonic rate order and balanced system order within each rate block. A
60-second cooldown precedes each executed candidate cell and is recorded in the
batch progress ledger. The run-level capture remains bounded and trace-free.

Capacity-knee classification uses the unchanged delivery-good estimator: pooled
`sum(total_undelivered) / sum(intended) <= 0.01`, mean run achieved ratio at
least `0.99`, and zero duplicates. A delivery-bad MQTT-loopback cell censors SUT
cells at that offered rate and above. The N=5 candidate summary is written under
`manifests/candidate-batches/`; it remains `candidate-supplementary`, sets
`thesis_evidence=false` and `n30_admitted=false`, and cannot replace or pool with
E-Perf-10.

`e-perf-payload-refinement` preserves the E-Perf-4 in-process measurement
boundary: one `bench-source`, one pass-through Wasm transform, and one
`bench-sink`. It runs 120 B, 1 KiB, 8 KiB, 10 KiB, 16 KiB, 32 KiB, 64 KiB,
100 KiB, 128 KiB, and 256 KiB at 1,000 msg/s, with 30,000 warmup and 60,000
measured messages in each of five independent runs per size. The source emits
exactly the configured number of `0x42` bytes. Every leaf records the byte count,
`SHA-256(0x42 repeated payload_bytes times)`, config checksum, population, and
candidate/no-pooling labels in `payload-manifest.json`. Load-generator template
hashes are not evidence for this in-process experiment.

`e-perf-depth-extension` preserves the combined E-Perf-6/E-Perf-8 in-process
measurement boundary: one `bench-source`, exactly 1, 3, 5, 10, 20, or 50
identical pass-through Wasm transforms, and one `bench-sink`. Each depth has five
independent runs at 1,000 msg/s after a 30-second warmup, and each physical leaf
contains both latency and RSS evidence. `topology-manifest.json` records exact
node and edge counts, the ordered chain, shared transform behavior, effective
fuel-and-epoch metering, config checksum, and candidate/no-pooling labels.
Latency and RSS are two outcomes of the same run, not separate or duplicated raw
populations. Neither candidate family is pooled with E-Perf-3, E-Perf-4,
E-Perf-6, E-Perf-8, or earlier rehearsals.

`e-swap-independent-sessions` reuses the E-Swap-1 in-process path for five
independent host runs with 50 alternating v1/v2 events per run over a 120-second
measurement window. `e-swap-rollback-sessions` reuses the E-Swap-5 process-trap
path for five independent host runs with 50 successful rollback events per run
over a 300-second measurement window. Rollback candidate evidence reconciles
`swap_requests.json`, `rollback.json`, and `sequence.csv`; it does not require a
sink-transition timeline because failed swaps produce no successful plugin-version
transition. Event 0 in each run is labeled
`first-use-aot`; events 1–49 are labeled `cached`. Event-level rows stay nested
within their run, while comparisons use one per-run aggregate for each event
class. Sequence evidence must remain lossless, failed attempts remain immutable,
and neither candidate may reference an E-Swap-1/2/5/6 alias as its measurement
source. The 10 ms actual-t0 artifacts remain scoped to E-Swap-3/E-Swap-4; these
multi-event candidates use bounded one-second interval metrics and event timing
records instead of fabricating a single-event 10 ms series.

`e-host-thermal-storage` is one clean-boot diagnostic session, never a
performance replicate. The fixed order is idle; one, two, and three SUT-core
CPU workers; CPU plus memory; USB write; USB read; and combined CPU, memory,
and USB. Idle lasts 120 seconds and every other phase lasts 300 seconds, with
one-second telemetry. The runner requires a caller-recorded boot ID, starts
within ten minutes of that boot with initial `get_throttled=0x0`, writes a new
append-only leaf under `raw/e-host-thermal-storage/`, and stops on the first
sample at or above 75 °C, any non-zero throttling value, boot-ID change, kernel
I/O error, workload/instrumentation failure, or USB SHA-256 mismatch. It never
intentionally continues in a throttled state. `diagnostic_admission` covers all
phases through USB read; `final_admission` additionally requires the maximum
combined-load phase. A failed combined phase therefore keeps N=30 blocked even
when the preceding diagnostic gate passed. PMIC values remain an internal-rail
proxy and exclude USB and total input power. Synthetic fixture receipts are
labeled `execution_mode=fixture-synthetic`, remain admission-ineligible, and
only report the corresponding evaluated gate decisions.

`e-compare-ekuiper-profile` contains exactly five profiled and five unprofiled
control runs at each of 1,000, 4,000, and 8,000 msg/s. A pair is the profiled
and unprofiled run sharing one offered rate and run index; both use the same
canonical eKuiper 2.1.0 config, load-generator profile, QoS, operator
concurrency, 30-second warmup, and 60-second measurement. The profiled arm alone
starts a one-second external `/proc` sampler bounded to at most 62 rows. If
required process files are unreadable, the run continues and records process
metrics as unavailable. The unprofiled control must not contain
`resource-usage.csv`.

The frozen eKuiper deployment has no validated GC-event interface. Every
runtime summary therefore labels GC/runtime metrics unavailable with the exact
version limitation instead of treating RSS or latency excursions as GC events.
Analysis preserves all 30 independent host runs, forms 15 rate/run-index pairs,
and reports profiled-minus-control differences as diagnostic associations only.
The profile batch cannot be pooled with E-Perf-1, E-Perf-10, or prior diagnostic
rehearsals, and cannot support a GC-causality claim.

#### Enhanced analysis outputs

`eval/analysis/enhanced-visual-manifest.json` is the machine-readable T12
contract for twelve candidate and diagnostic chart/table families. The renderer
writes one deterministic SVG and one source CSV per family under
`derived/enhanced-n5/<batch-id>/`, an artifact manifest in the same directory,
and an explanatory local-link report under `reports/enhanced-n5/<batch-id>/`.
It never writes below `raw/` or `manifests/`.

Every normalized source row binds the explicit batch, clean source SHA, release
tag, source-relative path, evidence class, independent run, nested sample, unit,
and candidate/final flags. Rendering fails closed on missing N, wrong source or
tag, alias inflation, uncensored capacity, mixed first-use/cached event classes,
right-censored drain evidence, undeclared units, or a candidate/diagnostic row
marked as thesis evidence or admitted to N=30. Each HTML card links the local
SVG and CSV and states source fields, units, sample unit, how to read the view,
its observed N=5 pattern, and its limits. Before the expanded rehearsal, the
pattern is explicitly `PENDING`; synthetic fixtures validate only the contract
and renderer.

#### Bounded interval outputs

Applicable runs add `interval-metrics.json`, with one-second p50/p95/p99,
throughput, CPU, RSS, PMIC-proxy, temperature, and queue summaries. Each row
labels monotonic elapsed `[start,end)` bounds and Unix-epoch boundary values used
only for cross-process alignment. Cardinality is bounded by
`ceil(declared_measurement_duration_ns / 1s) + 2`; E-Swap-4 uses only its
canonical primary `[0,120s)` window here and keeps drain evidence separate.
Rows contain no message IDs, sequence IDs, or per-message timestamps, and
mandatory per-message traces remain forbidden. Interval populations must
reconcile to aggregate counters and the HDR population; intervals never replace
the primary aggregate estimator.

Latency p50/p95/p99 are bucket-HDR quantiles; throughput records both the unique
received count and its duration-normalized messages-per-second rate. CPU is the
process-tick delta divided by the resource-sample timestamp
duration, RSS is the bucket maximum, PMIC internal-rail proxy watts are the
arithmetic mean, temperature is the maximum, and queue depth is the maximum
across sampled queues. Each sampled
metric is a discriminated object with either `status=available` and `value`, or
`status=unavailable` and a specific reason. Zero is never used as a substitute
for a missing observation. `interval-latency.json` is the bounded producer
fragment; the runner composes it with resource and host telemetry before a leaf
can receive a passed terminal receipt. Late or out-of-order interval arrivals,
missing fragments on timed v5 runs, row-count overflow, clock drift, and
population mismatches reject the leaf. Both Rust producers preallocate the row
vector and reuse one bucket histogram, so interval recording adds no per-message
allocation, I/O, logging, or asynchronous send. The load-generator bridge remains
bounded and is drained before its final partial or empty bucket is flushed.

E-Swap-3 and E-Swap-4 retain their canonical 100 ms series and add exactly 400
10 ms buckets over the bounded `[-2s,+2s)` window around actual `t0`. Each fine
artifact includes 40 actual-t0-aligned 100 ms parent buckets whose
unique/event/duplicate counts equal the sums of their ten nested buckets.
E-Swap-3 parents equal the matching canonical event-window slice. E-Swap-4's
source-origin canonical grid can differ by the admitted alignment error, so its
nested parent view remains supplementary and never replaces canonical sequence,
primary, or drain accounting. E-Swap-4 continues to use 1,200 primary buckets
over `[0,120s)`, 100 separate drain
buckets over `[120s,130s)`, zero after-drain arrivals, and no right censoring.

#### Preserved primary and claim boundaries

E-Perf-10 delivery-good remains the pooled estimator
`sum(total_undelivered) / sum(intended) <= 1%`, with mean achieved ratio at
least 0.99 and zero duplicates. MQTT loopback remains the support-path censor:
a delivery-bad support cell censors SUT-only capacity claims at that rate and
above. E-Perf-10 retains bounded summaries, HDR and sequence accounting, while
its final outputs remain trace-free.

E-Perf-2 remains an alternate analysis of E-Perf-1, E-Perf-8 of E-Perf-6, and
E-Swap-2/E-Swap-6 of E-Swap-1. Candidate independent swap and rollback sessions
use new IDs and do not turn these aliases into additional observations.

The PMIC internal-rail proxy is not total input power. External total-input
power and matched x86 execution are future work; E-Perf-5 remains PENDING until
the matching x86 Linux block exists, and no cross-architecture claim is made.
The retained 5 V / 4.2 A supply receives
no waiver from throttle, temperature, reboot, or I/O gates.

#### Single-volume evidence storage

Prospective v6 evidence uses one physical exFAT volume labeled
`WAF_RESULTS`, an 11-code-unit label that the filesystem can represent. It is
mounted at `/mnt/wafer-results` on Pi/Jetson and `/Volumes/WAF_RESULTS` on macOS.
Manifests store volume-root-relative paths.
The volume contains `raw/`, `manifests/`, `derived/`, and `reports/`. Raw
artifacts are append-only, retain failed and interrupted attempts, and are never
duplicated during host transfer. Analysis opens raw inputs read-only and writes
only under `derived/` and `reports/`.

Storage qualification is a staged, non-destructive gate. `prepared.json` binds
one stable device identifier, UUID, `WAF_RESULTS` label, exFAT type, exact mount
path, mount options, read-write state, available bytes, path-device identity, and
a bounded large-file/many-small-file corpus manifest. The operator then stops
writers, synchronizes, safely unmounts, and remounts the physical volume. A fresh
mount identity is mandatory. `verified.json` records expected and observed file
and byte counts plus missing, extra, and mismatched counts after full SHA-256
verification; every error count must be zero. macOS and Jetson handoff receipts
bind the same UUID, stable identifier, and volume-relative raw manifest before
analysis may open the raw tree. The qualification tooling does not format,
relabel, mount, unmount, copy, or delete storage.

Validate this architecture before implementing or scheduling candidates:

```sh
python3 eval/scripts/validate-enhanced-architecture.py \
  --matrix eval/canonical-matrix.json \
  --decision .plans/rpi5-v5-enhanced-experiment-readiness/architecture-decision.json \
  --contract eval/RESULT-CONTRACT.md
```

### Focused-pilot contract

The follow-up pilot is selected by `focused_pilot` in
`eval/canonical-matrix.json` and launched with:

```sh
python3 eval/scripts/lib/canonical_runner.py --focused --execute --batch-id <new-id>
```

This mode is diagnostic only. Every selected experiment declares its exact
condition/run indices, sample unit, repetitions, event count where applicable,
warmup, measurement boundary, required outputs, and analysis consumer. The
runner rejects a non-frozen seed and requires the committed
`eval/focused-pilot-schedule.json` snapshot. It writes both `schedule.json` and
`focused-pilot-execution.json` in the batch ledger. The execution receipt binds
the schedule and source revision to `eval/focused-pilot-freeze.json`; execution
stops if either the canonical-matrix or schedule SHA-256 no longer matches the
freeze receipt.

Every focused leaf records `thesis_evidence=false` and a `focused_pilot`
metadata object containing the matrix hash, memory-retention fix commit, and
eKuiper operator concurrency. Validate focused results with:

```sh
python3 eval/scripts/verify-result-contract.py --canonical --focused \
  eval/results/<experiment>/rpi5-<batch-id>
```

In addition to file presence and canonical host provenance, focused validation
enforces these semantic invariants:

- E-Perf-10 result status, counts, units, trace hashes, and system identity;
- eKuiper's captured rule uses the frozen default operator concurrency of one;
- E-Backpressure crossed its queue threshold, drained, remained lossless, and
  stayed within the frozen RSS bound;
- E-Iso-4 recorded contained traps with one recovery per trap and no reuse of
  an interrupted component instance;
- E-Iso-7 uses independent source and sink populations, a branch-local
  post-warmup measurement boundary, and lossless branch-A counts rather than
  shared-source or aggregate fan-in values;
- E-Perf-9 records every monotonic startup phase, one processed message, and
  valid compiled-cache state;
- E-Swap-1/2/4/6 keep internal phases and sink-observed gaps as distinct
  nanosecond fields with the frozen event count;
- E-Swap-3 and E-Swap-5 preserve sequence integrity, and E-Swap-5 records all
  requested rollbacks;
- the matrix records the closed memory-retention decision and its verified
  zero-byte/message diagnostic slope.

### `startup.json` schema

E-Perf-9 measures startup from runtime process entry through the first successful
sink collection. All durations use the runtime's monotonic clock and exclude
shutdown, result export, and analysis. `phases_ns` contains non-overlapping
`process_config`, `component_load_compile`, `instantiation`, `pipeline_setup`,
and `first_process` durations. Their sum must not exceed
`total_wall_duration_ns`; unmeasured transition overhead may be at most 5 ms.

The one-message probe requires `processed_messages = 1`. `plugin_sha256` maps
each Wasm node to the exact loaded component digest. The
`compiled_component_cache` object records `mode`, `hit`, `artifact`, and
`identity`; a hit is invalid unless both artifact and identity are present.
Current production startup reports `mode = "disabled"`, `hit = false`, and null
artifact/identity because initial component loading does not use the dormant
serialized-component cache.

## `metadata.json` schema

`metadata.json` merges two sources: **run-experiment.sh** stamps run
metadata and per-invocation context (git_sha, timestamps, loadgen /
mosquitto blocks, exit codes) while **wafer-runtime** emits a
`runtime-provenance.json` sidecar with the fields only the runtime can
produce authoritatively (`wasmtime_version` from the resolved lockfile,
`config_sha256`, `wafer_plugin_hashes` sharing the P0.12 hot-swap guard
cache, `wafer_runtime_sha256`, `rustc_version`, `kernel`, effective engine fuel
budgets, epoch deadline/tick, and effective metering mode). `_write_metadata`
prefers the runtime's values for those keys so provenance stays
authoritative through cross-compilation.

```json
{
  "experiment": "e-perf-4",
  "host_tag": "shakedown-macos",
  "generated_at": "2026-07-21T14:30:15Z",
  "started_at": "2026-07-21T14:30:15Z",
  "finished_at": "2026-07-21T14:32:47Z",
  "duration_ns": 152000000000,
  "git_sha": "38252494da5f39ce1e02e5a642fae85d0791a527",
  "git_dirty": false,
  "git_tags": ["rpi5-eval-v1"],
  "hostname": "wafer-pi5",
  "kernel": "6.18.39+rpt-rpi-2712",
  "arch": "aarch64",
  "os": "linux",
  "hardware_model": "Raspberry Pi 5 Model B Rev 1.0",
  "memory_total_kib": 4194304,
  "cpu_governors": ["performance"],
  "isolated_cpus": "1-3",
  "temperature_millicelsius": 53800,
  "throttled": "0x0",
  "rustc_version": "rustc 1.85.0 (unknown)",
  "wafer_runtime_version": "0.1.0",
  "wafer_runtime_sha256": "…",
  "wasmtime_version": "43.0.0",
  "wafer_plugin_hashes": {
    "parser": "…",
    "filter": "…"
  },
  "config_path": "eval/configs/pipeline-c-passthrough.toml",
  "config_sha256": "…",
  "engine_fuel_budgets": {
    "transform": 10000000,
    "filter": 500000,
    "router": 500000
  },
  "epoch_deadline": 100,
  "epoch_tick_ms": 10,
  "effective_metering_mode": "fuel-and-epoch",
  "loadgen": {
    "profile_path": "eval/loadgen/generic-1kb.toml",
    "subscribe_topic": "wafer/bench/output"
  },
  "mosquitto": {
    "container_id": "docker-container-id",
    "image": "eclipse-mosquitto:2.0.18"
  },
  "exit_codes": {
    "wafer_runtime": 0
  }
}
```

For final WAFER leaves, the verifier compares these effective metering fields with the condition in `canonical-matrix.json`. E-Perf-7 uses its four-way `metering_modes` table; declared attack stimuli use their condition-specific exceptions; all other WAFER conditions use `final_campaign.canonical_metering`. A missing or mismatched value is a contract violation.

Canonical runs require `git_dirty: false`, a non-empty `git_tags` array identifying a tag that points at `git_sha`, and the Pi 5 host fields below. Smoke runs may be dirty but cannot be promoted to thesis evidence. Validate canonical leaves with:

```sh
python3 eval/scripts/verify-result-contract.py --canonical <result-dir>
```

### Pi 5 host fields

Canonical `rpi5` runs additionally require:

| Field | Source | Required value |
| --- | --- | --- |
| `hardware_model` | `/proc/device-tree/model` | Raspberry Pi 5 |
| `memory_total_kib` | `/proc/meminfo` | approximately 4 GB; exact firmware-visible value is recorded |
| `cpu_governors` | `/sys/devices/system/cpu/cpu*/cpufreq/scaling_governor` | only `performance` during measured runs |
| `isolated_cpus` | `/sys/devices/system/cpu/isolated` | `1-3` |
| `temperature_millicelsius` | `/sys/class/thermal/thermal_zone0/temp` | captured at run completion |
| `throttled` | `vcgencmd get_throttled` | `0x0` |

The preflight rejects a host that does not meet these conditions. Smoke runs may retain the same `rpi5` path prefix, but their metadata and invocation are labelled non-canonical and must not be consumed as thesis evidence.

### Runtime-owned fields

| Field | Producer | Rationale |
| --- | --- | --- |
| `wasmtime_version` | `wafer-runtime` build.rs, parsed from workspace `Cargo.lock` | The resolved dep version differs from the Cargo.toml declaration when wasmtime is pulled from git; this captures what actually ran. |
| `rustc_version` | `wafer-runtime` build.rs, `rustc --version` at compile time | Cross-compilation drops the host rustc; build-time capture keeps provenance intact. |
| `wafer_runtime_version` | `env!("CARGO_PKG_VERSION")` | Semver of the binary that ran, not the workspace. |
| `wafer_runtime_sha256` | `std::env::current_exe()` + SHA256 | Exact binary bytes so a canonical-run number can be tied to the exact build artefact. |
| `wafer_plugin_hashes` | `PipelineOrchestrator::plugin_hashes_snapshot()` | Populated at initial launch AND after every hot-swap by `PipelineHandle::record_plugin_hash`; the P0.12 hot-swap guard reads from the same map, so metadata + guard stay coherent (F2 AC2). |
| `config_sha256` | Runtime `Sha256` of the effective config file at load time | Notebook cross-references use this as the provenance root. |
| `kernel` | `uname -r` via subprocess from the runtime | Kept in both the runtime provenance and the shell metadata; the runtime version wins on merge. |
| `engine_fuel_budgets` | Parsed effective engine config | Per-category fuel budgets; absent limits are JSON `null`. Node overrides remain represented by the config digest. |
| `epoch_deadline`, `epoch_tick_ms` | Parsed effective engine config | Effective epoch deadline and tick; an omitted deadline is JSON `null`. |
| `effective_metering_mode` | Derived from parsed fuel and epoch options | Stable value: `neither`, `fuel-only`, `epoch-only`, or `fuel-and-epoch`. |

### Sidecar file

- Path: `runtime-provenance.json` inside the result directory.
- Written by `wafer-runtime` on successful launch when either
  `WAFER_METADATA_OUTPUT` (explicit path) or `WAFER_BENCH_OUTPUT_DIR`
  (existing convention) is set. `run-experiment.sh` sets the latter.
- The runtime emits the sidecar even if the run subsequently traps, so
  post-mortem analysis retains provenance.

Fields are omitted when not applicable:

- `loadgen` is absent when the config is BenchSource/BenchSink only (no MQTT).
- `mosquitto` is absent when the harness reuses an already-running broker
  (see the `WAFER_HARNESS_MQTT` environment variable in
  [`eval/scripts/run-experiment.sh`](./scripts/run-experiment.sh)).
- `wafer_plugin_hashes` is present but empty (`{}`) for all-native
  pipelines (RFC-008 §D5 baselines).

`config_sha256` is the digest of the effective `config.toml` bytes as
loaded by the runtime — reproducibility hinge for notebook cross-references.

## Idempotency and safety

- **Safe to re-run**: `run-experiment.sh` never overwrites an existing
  timestamped directory. Two runs at the same second (rare) get a
  suffixed second timestamp.
- **Never `sudo`** on the development machine. The Pi host setup how-to explicitly marks its privileged device configuration commands.
- **Never `rm -rf`** on a result directory. Cleanup is the operator's job.
- **Native broker on Pi 5**: canonical Pi runs pass `--broker 127.0.0.1:1883`; the harness never starts Docker there.
- **Development fallback only**: when no broker is supplied, `run-experiment.sh` may auto-start a labelled Mosquitto container for laptop shakedowns.

## Reproducibility hinge

Every artefact carries the run's `metadata.json.config_sha256` as its
provenance root. Notebooks that plot a result MUST read `metadata.json`
before reading the artefact, and MUST assert the SHA256 against the
notebook cell that produced the config revision. This is how P7.1
achieves reproducibility across shakedown and canonical runs.
