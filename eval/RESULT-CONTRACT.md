# Result-directory contract

Every WAFER experiment run produces one self-contained raw leaf beneath the
selected results layout. Repository-local tests use
`eval/results/<experiment-id>/<host-tag>-<UTC-timestamp>/`; campaign runs use
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
        ├── swap_requests.json          ← API-side hot-swap timing/outcomes
        ├── swap_timeline.json          ← successful sink-observed transitions
        ├── hotswap-analysis.json       ← matched internal/output-gap evidence
        ├── rollback.json               ← failed-replacement rollback evidence
        ├── post-rollback-continuity.json ← output after the final rollback
        ├── branch-a/                   ← E-Iso-7 only
        │   ├── latency.hdr
        │   ├── throughput.csv
        │   ├── sequence.csv
        │   └── measurement-window.json
        └── branch-b/                   ← E-Iso-7 only; same branch-local files
```

Campaign runs pass an explicit results root. The approved volume layout is:

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
managed directory. Linux bind mounts count when the exact root appears in
`/proc/self/mountinfo`; an ordinary child directory on that filesystem does not.
Paths containing spaces are supported. Stored paths are relative to the volume root.

Generated path segments use a conservative exFAT-safe ASCII set. The layout rejects
reserved DOS names, separators, control characters, trailing dots or spaces,
case-fold/Unicode-normalization collisions, symlinks, and hardlinked evidence.
Temporary files used for atomic receipts live beside their destination; an existing
immutable destination or a cross-device rename fails instead of falling back to a
copy. `BenchSink`, `BenchSource`, and the `wafer-loadgen` subscriber artifacts,
`publisher-summary.json` and publisher timing receipt are written the same way: a
hidden `.<name>.tmp` file beside the destination, synced, then renamed.

Each batch ledger under `manifests/canonical-batches/` or `manifests/candidate-batches/`
holds `schedule.json`, `progress.jsonl`, and `batch.json`, plus `scout-complete.json` when
the batch has bracket rates. The runner writes `batch.json`
once, when the batch starts: `schema_version`, `batch_id`, `host`, `source_git_sha`,
`source_dirty`, `canonical_matrix_sha256`, `seed`, sorted `experiments`, `repetitions`
(the diagnostic override or `null`), `thesis_evidence` (true only for a
`canonical-batches` ledger without `repetitions`), `started_at`, and, for a batch that runs
E-Perf-10, `capacity_brackets` (see [Per-host bracket rates](#per-host-bracket-rates)). A
resume from another source SHA or matrix hash is refused. `mise run approve-batch` adds
`raw.sha256`: one `sha256sum` line per file of the batch under `raw/`, its alias receipts,
and its ledger, including the copy of the scout summary its bracket rates came from,
sorted, with volume-relative paths. It records the batch and the SHA-256 of `raw.sha256`
in the repository file `eval/final-batches.json`, which canonical analysis reads.

Raw attempts are additive. A failed or interrupted attempt remains in place and the
next attempt uses the next numeric suffix. A terminal receipt makes the leaf
immutable; [Attempts and retries](#attempts-and-retries) defines the receipt and
which attempt of a run is admitted. Analysis resolves raw inputs through the same results root and may create
outputs only below `derived/` or `reports/`; output traversal or overlap with raw is
rejected.

Shared canonical views do not create another raw tree. A small alias receipt under
`manifests/aliases/` records the volume-relative source leaf, source status digest,
shared sample identity, machine evidence class `final`, and
`independent_n_contribution = 0`. Consumers validate and dereference that receipt to
one admitted, thesis-eligible source leaf (a clean pass or a system outcome). Alias mappings must be acyclic; aliases may
not target rejected, diagnostic, or candidate evidence. Copies, symlinks, and
hardlinks are not alias mechanisms. `final` is the machine value; prose names its
admitted analytical class `canonical-primary` or `final admitted`. Historical
repository-local result trees remain unchanged.

**Host tags** (explicit whitelist — the harness rejects anything else):

- `shakedown-macos` — MacBook laptop shakedown.
- `rpi5` — Raspberry Pi 5 4 GB canonical run.
- `jetson` — Jetson Orin Nano replication run.
- `x86` — x86-64 Linux replication run.

**Timestamp format**: `YYYY-MM-DDTHH-MM-SSZ` — UTC, colons replaced with
hyphens so the path is `mv`-safe on every filesystem.

## Attempts and retries

Each scheduled unit (one experiment, condition and run index) may have several
attempt directories, `run-NN-attempt-MM`. Every attempt ends in one of three
classes, which its terminal receipt `canonical-status.json` records:

| Class | Receipt | Meaning | Admitted | Retried |
| --- | --- | --- | --- | --- |
| clean pass | `"status": "passed"` | The run met every integrity gate and every criterion it measures. | yes | no |
| system outcome | `"status": "failed"`, `"failure_class": "sut_outcome"`, `"reasons": [...]` | The system under test failed a criterion the run measures. | yes, and it counts against that criterion | never |
| infrastructure failure | `"status": "failed"`, `"failure_class": "infrastructure"`, `"reasons": [...]` | The host, the harness or the evidence failed before the run could be judged. | no | at most once, in place |

Every receipt also records `experiment`, `condition`, `run_index` and
`updated_at`; a failure may add a free-text `detail`. An attempt directory
without a receipt was interrupted and counts as one infrastructure attempt with
the reason `interrupted`.

| Reason | Class | When |
| --- | --- | --- |
| `runtime-exit` | system outcome | The runtime exited non-zero after it started the pipeline, crashed, or was killed after its shutdown grace period (see [Runtime exit status](#runtime-exit-status)). For eKuiper, `ekuiper-health.json` shows a different `NRestarts` or `MainPID` for the `kuiper` unit after the run than before it (see [eKuiper health](#ekuiper-health)). |
| `rule-error` | system outcome | eKuiper kept its main process, but the rule the run ends with (`pipeline_a`, or `pipeline_a_v2` after the E-Swap-3 make-before-break replacement) did not report `running` at the end of the run, or its status carried a `message` (see [eKuiper health](#ekuiper-health)). |
| `containment-escape` | system outcome | `containment.json` records `contained: false`. |
| `message-loss` | system outcome | Messages were lost where the criterion expects none: E-Swap-1, the E-Swap-3 hot swap, E-Swap-4, E-Swap-5, the candidate swap and rollback sessions, and the E-Backpressure `slow` policy. |
| `duplicates` | system outcome | Messages were delivered twice where the criterion expects none, in the same experiments. |
| `swap-failed` | system outcome | A hot-swap request of E-Swap-1, E-Swap-4 or the candidate swap sessions did not return HTTP 200. The E-Swap-3 hot swap also fails when its response body reports `rolled_back` or does not report `replacement_adopted: true`. |
| `rollback-failed` | system outcome | A failed replacement of E-Swap-5 or the candidate rollback sessions did not return `rolled_back`. |
| `memory-limit` | system outcome | E-Backpressure resident memory went over its declared limit. |
| `harness-error` | infrastructure | Host facts or preflight, throttle or temperature gates, storage, the broker, a load generator or publisher that crashed or did not start, an action that missed its alignment window, or any other harness exception. |
| `contract-violation` | infrastructure | The verifier rejected the leaf: missing or invalid telemetry, schema, provenance or host evidence. |
| `interrupted` | infrastructure | The attempt has no receipt. |

`eval/analysis/src/wafer_analysis/attempts.py` declares both reason lists and
reads the system-outcome reasons back from a leaf's artefacts; a new reason is one
entry there and its check. The runner writes the receipt from those reasons after
the verifier accepted the leaf, and the analysis rejects a receipt whose reasons
differ from the ones the artefacts give.

A run that the system under test stopped early (`runtime-exit`, `rule-error`,
`swap-failed` or `rollback-failed`) is not post-processed. The verifier checks only
what the harness owns: the core artefacts, provenance, host facts, Pi telemetry, the
measurement window and, for eKuiper, `ekuiper-health.json`. When the runtime died
before its sink exported `measurement-window.json`, the harness writes the attempt's
own wall-clock bounds in its place, so the leaf still verifies. An eKuiper failure
leaves every harness and load-generator artefact in place, because eKuiper writes
nothing into the leaf, but those artefacts measure a pipeline that died or stopped
during the window. Post-processing them would turn the failure into latency,
disruption and capacity numbers, and a check that the failure broke would retry the
attempt as infrastructure, so an eKuiper run with `runtime-exit` or `rule-error`
stops early as a WAFER run with `runtime-exit` does. A run that completed with a
failed criterion goes through every check. The verifier prints an `OUTCOME` line for
each system outcome and still exits 0 when nothing else is wrong. A runtime that dies
before its control plane answers, or that dies when E-Swap-3 restarts it, is judged
by its exit code like any other run. An eKuiper rule that does not run cleanly before
warm-up never started the measured pipeline and fails the attempt as infrastructure,
as a WAFER startup refusal does. Three paths still end as infrastructure failures
although the system under test may have caused them: an E-Swap-3 restart whose new
runtime keeps running but never answers its control plane, an E-Swap-3 eKuiper
rule update whose PUT or start does not return 200 or whose updated rule does not count
output within 10 seconds, and an E-Swap-3 make-before-break replacement whose REST calls fail
or whose replacement rule does not count output within 10 seconds. A REST answer that
breaks off fails the attempt the same way, including one that breaks off because
kuiperd died during the call.

`final_campaign.attempt_policy` in `canonical-matrix.json` sets the retry cap: one
infrastructure retry per unit, and none for the experiments in `gate_experiments`
(E-Val-1). The runner retries an infrastructure failure immediately, before the
next scheduled unit, so the retry keeps the unit's place in the randomized order. A
system outcome is never retried. On resume, a unit with an admitted attempt is
skipped, a unit whose attempts reached the cap without one is reported missing and
not run again, and an interrupted attempt counts against the cap. The capacity
scout is not a final experiment; it has no cap and keeps its own stop rules.

The analysis admits exactly one attempt per unit: its clean pass or its system
outcome. A unit with more attempts than the cap allows, two admitted attempts, or
an attempt after the admitted one rejects the batch, and so does a missing unit.
Each criterion counts system outcomes as failures: a run the runtime did not
survive is not contained, its E-Iso-7 attack condition fails, and its
E-Perf-10 rate is delivery-bad; a run with loss, duplicates or a failed swap is not
lossless, and a failed rollback is not a successful rollback. The summary notebook
writes `campaign-attempts`, one row per experiment, system and condition with its
units, attempts, clean passes, system outcomes, infrastructure failures, retries and
missing units, counted from the attempt directories themselves.

E-Val-1 is the gate for the rest of the batch and is never retried. Every
repetition must pass on its only attempt inside the p99 band; a failed, outcome or
interrupted attempt fails the gate, and a new gate needs a new batch.

## Manifest

The result-directory contract is split into **core artefacts**
(mandatory on every run) and **per-experiment optional artefacts**
(present only when the experiment's semantics require them).

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
| `latency.hdr` | Every experiment with a BenchSink or `wafer-loadgen subscribe` (E-Val-1, E-Perf-1..10, E-Backpressure, E-Iso-*, E-Swap-*) | `BenchSink` (in-process) or `wafer-loadgen subscribe` (E2E) | HdrHistogram latency in nanoseconds. The two producers use different encodings of the same V2 histogram: `BenchSink` writes an HdrHistogram interval log (text, one base64 entry tagged `latency_ns`), the subscriber writes the bare binary V2 histogram (starts with the bytes `1c 84 93`). `wafer-loadgen hdr-summary` reads both; pick the reader by content, not by experiment. The value is measured from each message's scheduled send time: arrival minus scheduled time. Both generators pace an open-loop schedule fixed at the start of the run, so time a message waited before it could be sent (a blocked source, a lagging publisher) counts as latency (no coordinated omission). In-process: source-to-sink. E2E: MQTT publish → MQTT consume. Every latency histogram (this one, `service.hdr`, `source-lag.hdr` and the interval rows) uses the same range, 1 µs to 1 h at 3 significant digits. No sample is dropped: a value below zero or above 1 h is recorded at that bound and counted (`latency_clamps` in `measurement-window.json`, `negative_latency_count` and `above_highest_latency_count` in `subscriber-metadata.json`), and a non-zero count rejects the run. In-process timestamps come from one monotonic clock anchored to the wall clock once per process, so a wall-clock step cannot change an in-process latency. |
| `service.hdr`, `source-lag.hdr` | In-process runs whose source is `BenchSource` | `BenchSink` | Two parts of the `latency.hdr` value, same interval-log encoding and bounds. `service.hdr` (tag `service_ns`): arrival minus the time the message left the source. `source-lag.hdr` (tag `source_lag_ns`): time the message left the source minus its scheduled time. Lag grows when the pipeline pushes back on the source; a run whose lag is close to its latency is limited by the source, not by the stage under test. Both are absent when no message carried `BenchSource` stamps. |
| `service-percentiles.json` | Final E-Perf-4 | `canonical_runner.py` via `wafer-loadgen hdr-summary` | Summary of `service.hdr` with the same fields as the runner's `percentiles.json` summary of `latency.hdr`: `total_count`, `p50_ns`, `p95_ns`, `p99_ns` and `p999_ns`. Its `total_count` must be positive and equal to the `total_count` of `percentiles.json`, so every measured message has a service time. E-Perf-4 analysis reads the boundary cost from this file, not from `latency.hdr`, so that source lag stays out of the WAFER-minus-native difference. |
| `interval-latency.json` | Timed runs with latency output | `BenchSink` or `wafer-loadgen subscribe` | Bounded producer fragment containing one-second latency histograms reduced to p50/p95/p99 and event/unique/duplicate counts. This is an implementation input to the composer, not a replacement for `latency.hdr`. Both producers write the same keys: `schema_version`, `interval_clock` (`monotonic-elapsed`), `alignment_clock` (`unix-epoch`), `alignment_clock_purpose`, `measurement_start_unix_epoch_ns`, `declared_measurement_duration_ns`, `bucket_width_ns`, `maximum_rows`, `row_count`, `late_arrivals`, `aggregate_latency_count` (the `latency.hdr` population) and `rows`. The subscriber also writes `publisher_drain_ns`, the longest a publisher that completed its schedule takes to exit (its 5 s acknowledgement drain and 1 s disconnect wait), and `drain_grace_ns`, the drain grace from `--drain-grace-secs`; its rows may run that far past the declared window. Each row has `interval_start_ns`, `interval_end_ns`, `interval_start_unix_epoch_ns`, `interval_end_unix_epoch_ns`, `latency_count`, `latency_p50_ns`, `latency_p95_ns`, `latency_p99_ns` (null for an empty bucket), `received_events`, `throughput_messages` and `duplicates`. |
| `interval-metrics.json` | Timed runs with latency or throughput output | `canonical_runner.py` or `interval_metrics.py` | Bounded one-second rows combining latency, throughput, CPU, RSS, PMIC internal-rail proxy, temperature, and queue observations. Missing observations use an explicit `unavailable` status and reason. |
| `throughput.csv` | Same as `latency.hdr` | `BenchSink` (in-process) or `run-experiment.sh` (E2E) | The two producers write different files under this name. `BenchSink`: `elapsed_secs,msg_count,bytes`, one row per fixed 1 s bucket counted from the first post-warmup arrival up to the last arrival. `elapsed_secs` is the bucket's end in whole seconds, a second with no arrivals between two that have some is a zero row, and the last row is the second in which the last message arrived. `msg_count` counts every post-warmup arrival, duplicates included. E2E: `run-experiment.sh` derives one summary row from `subscriber-metadata.json` with `eval/scripts/lib/write_throughput.py`: `timestamp_ns,messages_received,throughput_msg_s,duration_ns` (end of the subscriber run, `total_recorded`, their ratio, and the subscriber's run duration). |
| `sequence.csv` | Loadgen with sequence tracking, or `BenchSink.track_sequences = true` (E-Perf-1..3, E-Perf-8, E-Perf-10, E-Backpressure, E-Swap-*) | `wafer-loadgen subscribe` / `BenchSink` | Exact per-message sequence accounting: every sequence number in the declared population is marked when it arrives, so a late arrival is a reorder (not a gap plus a duplicate), a duplicate is a number seen twice, and a number never seen is missing whether or not a later one arrived. `BenchSink` writes one summary row (`total_expected,total_received,received_unique,gap_ranges,gap_msgs,duplicates_count,out_of_order,out_of_range`) over the population from the measurement start sequence up to the sequence end that `BenchSource` stamps on every message (its per-message warmup stamp excludes the warmup population), so `gap_msgs` includes messages missing at the tail; without those stamps the population runs from the first measured number to the highest one seen. `total_expected = received_unique + gap_msgs` and `total_received = received_unique + duplicates_count + out_of_range`; `out_of_range` (a number outside the population) must be zero. `wafer-loadgen subscribe` writes long-form events (`event_type,seq_start,seq_end,count`): one `gap` row per run of missing numbers (tail included when `--sequence-end-exclusive` declares the population) and one `duplicate` row per retained example, bounded by `--sequence-example-limit`; zero data rows when lossless. Its totals live in `subscriber-metadata.json`. |
| `memory.csv` | E-Perf-6, E-Perf-7, E-Perf-8, E-Backpressure (and any run with `WAFER_BENCH_OUTPUT_DIR` set) | `wafer-runtime` via `MemoryRecorder::sample_loop` (1 Hz, cross-platform via `memory-stats` crate; on Linux it reads `/proc/self/statm`). Flush on graceful shutdown. | 1 Hz process-RSS timeline of `wafer-runtime`: `elapsed_ms,rss_bytes`. Runtime-owned since A19 closure (thesis-hardening T4). Legacy harness `ps` polling removed. |
| `memory-clock.json` | Runs with `memory.csv` | `wafer-runtime` | Pairs the memory recorder's monotonic zero with one Unix-epoch anchor for cross-process interval composition: `schema_version`, `elapsed_clock` (`monotonic`), `alignment_clock` (`unix-epoch`), `alignment_clock_purpose` and `start_unix_epoch_ns`. |
| `per_node_metrics.csv` | All experiments (emitted on graceful shutdown when `WAFER_BENCH_OUTPUT_DIR` set) | `wafer-runtime` via `PipelineOrchestrator::export_per_node_metrics` | One row per pipeline node: `node_id,messages_in,messages_out,filtered_out,traps_total,traps_memory_out_of_bounds,traps_unreachable,traps_interrupt,traps_out_of_fuel,traps_memory_limit,traps_other,guest_bad_input,guest_dependency_failed,guest_processing_failed,guest_timed_out,guest_unrecoverable,attempts_failed,retries,dlq_sent,dlq_lost,skipped,retry_exhausted_skips,dropped_on_recovery,dropped_on_teardown,error_state_seconds,recovery_count`. `messages_in` is the node's input-queue dequeue count (0 for sources). `traps_*` count calls the host aborted, by kind; `guest_*` count errors the guest returned, by WIT category; `attempts_failed` counts every failed call, retries included. Every processing node satisfies `messages_in = messages_out + filtered_out + skipped + retry_exhausted_skips + dlq_sent + dlq_lost + dropped_on_recovery + dropped_on_teardown` (pending retries are flushed to the DLQ at shutdown). Every config under `eval/configs/` has a file DLQ, so a message the error policy removes, or whose call trapped, is counted as `dlq_sent` and has a record in `dlq.jsonl`; `dlq_lost` counts records refused by a full or closed DLQ, and `dropped_on_recovery` stays zero unless a pipeline runs without a DLQ. Runtime-owned since A19 closure (thesis-hardening T4). |
| `dlq.jsonl` | All experiments (every config under `eval/configs/` sets `[dead_letter] kind = "file"`, `path = "dlq.jsonl"`, which the runtime resolves under `WAFER_BENCH_OUTPUT_DIR`) | `wafer-runtime` DLQ sink | One JSON object per dead-lettered message, written as it arrives: `timestamp` (ms since the Unix epoch), `source_node`, `error_category` (`bad_input`, `dependency_failed`, `processing_failed`, `timed_out`, `unrecoverable`, or null for a trap or queue overflow), `error_message`, `retry_count`, `reason` (`type` one of `bad_input`, `timed_out`, `retries_exhausted`, `retry_buffer_full`, `hot_swap_drain`, `shutdown`, `queue_full`, `trapped` with `kind`, `unrecoverable`, `recovery_failed`), `original` (id, timestamp, source, metadata, base64 payload, retry_count), `trace_id`, `parent_id`. The file exists, possibly empty, for every graceful run; the sink drains until every node has exited, so the record count equals the sum of `dlq_sent` over `per_node_metrics.csv` plus the edges' `dead_lettered` counts. |
| `queue-depth.csv` | E-Backpressure, when `WAFER_QUEUE_DEPTH_OUTPUT` is set | `wafer-runtime` via `QueueDepthRecorder` | Bounded internal Tokio queue samples at 10 ms intervals: `elapsed_ns,queue,depth,capacity,accepted,dequeued,processed,dropped,dead_lettered,downstream_closed,dlq_full,dlq_closed`. The five policy counters expose existing R1 `QueueSnapshot` state without changing dispatch behavior. Counters are internal pipeline observations; broker backlog is excluded. Collection is capped at 131,072 rows and reports truncation in the runtime log. |
| `queue-depth-clock.json` | Runs with `queue-depth.csv` | `wafer-runtime` | Pairs the queue recorder's monotonic zero with one Unix-epoch anchor for cross-process interval composition. Same keys as `memory-clock.json`. |
| `backpressure.json` | E-Backpressure | `canonical_runner.py` | Schema v2 records condition/policy, run index, measured queue, threshold crossing and recovery, offered/accepted/processed/drained rates, exact R1 queue counters, sequence counts, policy equation, producer-progress mode, explicit DLQ-full/closed failures, and peak-RSS bound (exceeding it is a `memory-limit` outcome). Attempted messages come from the frozen BenchSource population; `sequence.csv` validates its observed span because a trailing overflow disposition is not visible at the sink. `slow` expects attempted = delivered with no overflow disposition, and a shortfall is a `message-loss` outcome; `drop` requires attempted = delivered + dropped; `dead-letter` requires attempted = delivered + dead_lettered + dlq_full + dlq_closed. `dead_lettered` is the runtime's successful enqueue to the configured DLQ delivery path; full/closed failures are never counted as success. A high offered rate without measured occupancy is `not-saturated`. |
| `recovery.csv` | E-Iso-8 and any run with recovery events | `wafer-runtime` via `PipelineOrchestrator::export_per_node_metrics` | Exact recovery samples: `node_id,sample_index,duration_ns`. Retains nanosecond precision for trap-to-running percentiles. |
| `measurement-window.json` | Canonical Pi 5 runs | `BenchSink` for in-process runs with post-warmup output; canonical harness for external MQTT subscribers, zero-output containment runs, and runs the runtime did not survive (the attempt's own bounds) | Exact `started_ns` and `finished_ns` bounds (wall clock) for excluding warmup and teardown from PMIC energy integration. For an external MQTT subscriber the window ends when the publisher exits; the drain grace after it is outside the window. `canonical_runner.py` and `run-experiment.sh` both wait on the publisher process instead of polling it, so `finished_ns` is the exit itself for every system. The `BenchSink` copy adds `latency_clamps`: for each of `latency`, `service` and `source_lag`, the `negative` and `above_highest` sample counts recorded at a histogram bound. The verifier rejects a canonical leaf where any of them is non-zero. It also adds `wall_clock_step_ns`: how far the wall clock moved away from the in-process monotonic clock between the process start and the export, signed. In-process latencies do not depend on it; it tells whether `started_ns` and `finished_ns` can be aligned with other processes' telemetry. |
| `pi-telemetry.csv` | Canonical runs on every host | `eval/scripts/lib/pi_telemetry.py` | Timestamped temperature, CPU frequency, governor, throttling state, and summed rail watts from the host's backend (`rail_proxy_watts`; on Jetson the module input rail `VDD_IN` alone, because it already feeds the other INA3221 rails). `throttled` is `0x0` when the host is not throttled on every backend: the Pi writes the `vcgencmd get_throttled` value; Jetson writes `cpuN-below-pinned-clock` when a core runs under 95% of its pinned clock; x86 writes a nonzero policy token if an online CPU policy disappears, leaves `performance`, or no longer has readable equal minimum/maximum limits, and writes `cpuN-thermal-throttle` or `package-thermal-throttle` when the corresponding hardware counter increases. Active-mode `intel_pstate` defines per-core `scaling_cur_freq` as an average P-state between scheduler callbacks, so x86 retains it as audit data instead of treating an idle-core sample as a throttle verdict. The temperature comes from the `cpu-thermal` (Pi, Jetson; `CPU-therm` on L4T R32) or `x86_pkg_temp` zone, falling back to `k10temp`/`coretemp` hwmon, and is 0 when none exists. The power column is 0 while the backend has no reading (the first x86 RAPL sample, or an x86 host without RAPL), which `interval_metrics.py` reports as unavailable. |
| `pmic-rails.csv` | Canonical runs on every host | `eval/scripts/lib/pi_telemetry.py` | Long-form named rail samples (`timestamp_ns,rail,current_a,voltage_v,power_w`). Pi: PMIC rails, without rails lacking either voltage or current. Jetson: INA3221 channels from hwmon. x86: one row per RAPL package domain with `power_w` from the energy delta since the previous sample and empty current and voltage; header only when the host has no RAPL. |
| `power-boundary.json` | Canonical runs on every host | `eval/scripts/lib/pi_telemetry.py` | Names the `backend` (`pi`, `jetson`, `x86`) and its `measurement` (`rpi5-pmic-internal-rail-proxy`, `jetson-ina3221-rail-proxy`, `x86-rapl-package-energy`, or `unavailable`), states that none is total input power, and records the excluded consumers and the source. Power numbers are comparable only between leaves with the same `measurement`. |
| `cpu-cores.csv` | Canonical runs (every leaf the canonical runner or `run-experiment.sh --canonical` produces) | `eval/scripts/lib/proc_telemetry.py` | One row per CPU per second from `/proc/stat`: `timestamp_ns,cpu,user,nice,system,idle,iowait,irq,softirq,steal,frequency_hz`. Jiffies are cumulative since boot, so a reader takes the difference between consecutive rows and divides by `clock_ticks_per_second` from `host-sidecar.json`; `frequency_hz` is that core's `scaling_cur_freq` (empty where cpufreq is absent). This is what tells SUT-core utilisation on CPUs 1-3 apart from support-core utilisation on CPU 0. |
| `host-sched.csv` | Same as `cpu-cores.csv` | `eval/scripts/lib/proc_telemetry.py` | One row per second of system-wide scheduler and memory-pressure counters: `timestamp_ns,ctxt,processes,procs_running,procs_blocked,mem_available_bytes,psi_cpu_some_avg10,psi_memory_some_avg10,psi_memory_full_avg10,psi_io_some_avg10,psi_io_full_avg10,sampler_cpu_ms`. `ctxt` and `processes` are cumulative; PSI columns are empty on kernels without `/proc/pressure`; `sampler_cpu_ms` is the CPU time the sampler itself spent since its previous row, so every leaf carries its own overhead evidence. |
| `sut-processes.csv` | Same as `cpu-cores.csv` | `eval/scripts/lib/proc_telemetry.py` | One row per second per tracked process (`wafer`, `wafer-runtime`, `wafer-loadgen`, `kuiperd`, `mosquitto`, matched on `/proc/<pid>/comm`): `timestamp_ns,pid,comm,utime,stime,minflt,majflt,voluntary_ctxt_switches,nonvoluntary_ctxt_switches,threads,rss_bytes,cpus_allowed_list`. Counters are cumulative for the process's lifetime. Support processes are recorded next to the SUT so the leaf shows where the broker and the load generator ran. |
| `host-sidecar.json` | Same as `cpu-cores.csv` | `eval/scripts/lib/proc_telemetry.py` | Written when the sampler stops: `schema_version`, `sampler`, `interval_secs`, `started_unix_epoch_ns`, `finished_unix_epoch_ns`, `samples`, `sampler_affinity` (the CPU list it pinned itself to, the support cpuset under the canonical runner), `sut_cpus`, `sampler_cpu_seconds` and `sampler_max_rss_bytes` (its total cost), `clock_ticks_per_second`, `page_size_bytes`, `cpu_count`, `boot_id`, `kernel_cmdline`, `uptime_secs_at_start` and `tracked_comms`. If the sampler dies it writes `host-sidecar-error.json` (`error`, `timestamp_ns`) instead; the four files are supplementary host evidence, so their absence does not reject a leaf. |
| `published.csv`, `received.csv` | Historical E-Perf-10 diagnostics only | `wafer-loadgen` opt-in tracing | Raw publisher/subscriber timestamp and sequence samples retained for v11-v17 compatibility. Final E-Perf-10 rejects these mandatory traces and uses bounded summaries. |
| `publisher-summary.json` | Every measured MQTT publisher: final E-Perf-1 to E-Perf-3, E-Perf-10 and E-Swap-3, and the MQTT candidate and diagnostic runs | `wafer-loadgen publish` | `schema_version`, bounded `intended`, `rejected`, `enqueued`, `acked`, and `unacked_at_exit` counters plus the scheduled `measurement_duration_ns`. `intended = rejected + enqueued` and `enqueued = acked + unacked_at_exit` are mandatory: `enqueued` is what was handed to the MQTT client, `acked` is what the broker acknowledged (QoS 1 PUBACK), and `unacked_at_exit` is what was still unacknowledged when the 5 s drain window after a completed schedule closed (a run stopped by a signal skips the drain). `connects` counts successful CONNACKs and `connection_errors` counts event-loop errors; the clock starts only after the first CONNACK (10 s timeout, then the run fails without a summary). The verifier rejects `connects != 1` or `unacked_at_exit != 0`, and loss downstream of the publisher is `acked - received_unique`. `published` and `errors` repeat `intended` and `rejected` under their older names. `elapsed_ms` and `actual_rate` are the wall time the schedule took and `intended` divided by it; `deadline_misses` counts messages whose hand-off to the client ended after the next message was due; `hotswap_triggered_at_secs` is the swap offset for the `hotswap-trigger` profile and null otherwise. `source_lag_ns` (`count`, `p50`, `p99`, `p999`, `max`) is how late each message was handed to the MQTT client relative to its scheduled time; payload `ts` is the scheduled time, so this lag is part of the measured latency. `exit_reason` is `duration` when the schedule ran to its end, `sigterm`/`sigint` when a signal stopped it, or `broker-lost` when the MQTT session ended for good mid-run; the counters then cover the messages offered before the stop, and the verifier rejects the run. A second signal exits at once without a summary. |
| `subscriber-metadata.json` | Final E-Perf-10 and external-MQTT experiments | `wafer-loadgen subscribe` | Bounded receive, duplicate, ignored-warmup, parse, latency, and sequence totals: `broker`, `topic`, `started_at_ns`, `ended_at_ns`, `git_sha`, `host_tag`, `sequence_end_exclusive`, `total_messages` (every message read), `total_recorded` (the `latency.hdr` population), `ignored_sequence_count` (warmup), `parse_errors`, `latency_min_ns`, `latency_max_ns`, `latency_mean_ns`, `latency_p50_ns`, `latency_p95_ns`, `latency_p99_ns`, `latency_p999_ns`, the histogram range `histogram_lowest_ns`, `histogram_highest_ns`, `histogram_sig_digits`, and `sequence` (`expected`, `total_received`, `received_unique`, `total_gaps` (missing, tail included when `sequence_end_exclusive` is set), `total_duplicates`, `out_of_order`, `out_of_range`, plus bounded `gap_ranges` and `duplicate_seqs` examples and `examples_truncated`). The HDR count and received-event population must match. `negative_latency_count` and `above_highest_latency_count` count samples recorded at a histogram bound; `clock_steps` counts wall-clock steps larger than 100 ms seen between received messages (the gap between the wall clock and the monotonic clock changed). Latency subtracts the publisher's wall-clock schedule from the subscriber's wall-clock receive time, so a step shifts every later sample; the verifier rejects the run when any of the three is non-zero. `exit_reason` is `total-messages` (every distinct number up to `--total-messages` arrived), `sigterm`, `sigint`, or `eof`. `status` is `complete`, or `partial` when `interval-latency.json` or `throughput-buckets.json` overflowed its bound or failed its checks; `partial_reasons` names each failure. A partial run keeps `latency.hdr`, `sequence.csv` and this file, leaves out the failed artifact, exits non-zero, and is rejected by the verifier. Written last, so its presence means the other subscriber artifacts are complete. A second SIGTERM or SIGINT exits at once without flushing. |
| `export-errors.json` | Any run with a `BenchSink` output directory, only when an artifact failed | `BenchSink` | `{"schema_version":1,"errors":[{"artifact":…,"error":…}]}`. `BenchSink` writes `sequence.csv`, `measurement-window.json`, `swap_timeline.json`, `latency.hdr`, `service.hdr`, `source-lag.hdr` and `throughput.csv` first and the derived artifacts after, and one failed artifact does not stop the rest. Its presence rejects the leaf. |
| `capacity-run.json` | Final E-Perf-10 and candidate E-Perf-Capacity-Knee | `canonical_runner.py` | Trace-free run summary containing intended/rejected/enqueued/acked/received/lost/duplicate counts, offered and achieved rates, loss, p50/p95/p99, CPU, RSS, thermal state, process/config receipts, and controlled factors. Final E-Perf-10 sets `thesis_evidence=true`; capacity-knee sets `evidence_class=candidate-supplementary`, `thesis_evidence=false`, and `n30_admitted=false`. |
| `resource-usage.csv` | E-Perf-10 and E-Perf-Capacity-Knee | `canonical_runner.py` | One-second SUT samples: wall-clock timestamp, aggregate process CPU ticks, RSS bytes, and process count. MQTT loopback records the explicit no-SUT zero baseline. |
| `process-audit.json` | E-Perf-10 and E-Perf-Capacity-Knee | `canonical_runner.py` | Active SUT PID, process affinity, exclusivity, and allowed CPU set captured before measurement. |
| `rate-sweep.json` | Historical E-Perf-10 diagnostics only | Earlier `canonical_runner.py` versions | Legacy trace-backed run summary, always `thesis_evidence=false`. It remains readable but is not accepted as a final-capacity leaf. |
| `throughput-buckets.json` | Final E-Swap-3 and E-Swap-4 | `wafer-loadgen subscribe` / `BenchSink` / `canonical_runner.py` | Contiguous 100 ms output-rate buckets with unique/event/duplicate counts. E-Swap-3 bins exactly -10 s through +10 s around the actual disruption event. E-Swap-4 keeps 1,200 source-origin primary buckets over `[0,120s)` and a separate 100-bucket drain series over `[120s,130s)`, with O(1) after-drain evidence. Unix-epoch values are labeled for source/sink or cross-process alignment only. |
| `throughput-buckets-10ms.json` | Final E-Swap-3 and E-Swap-4 | `wafer-loadgen subscribe` / `BenchSink` | Exactly 400 contiguous 10 ms buckets over `[-2s,+2s)` around actual `t0`, plus 40 nested actual-t0-aligned 100 ms parent buckets. Both levels carry unique/event/duplicate counts and reconcile exactly. E-Swap-3 parent buckets also match the canonical event-window slice; E-Swap-4 retains its separate source-origin primary/drain series as the loss-accounting authority. No message identity or timestamp trace is retained. |
| `disruption-timeline.json` | Final E-Swap-3 | `canonical_runner.py` | Strategy, actual Unix-epoch action start/end for cross-process alignment, monotonic scheduling/duration labels, the scheduled t=60 target, measured offset, signed alignment error, and a 10 ms maximum alignment tolerance. This is the single retained runner-owned final action timeline for E-Swap-3. |
| `disruption-analysis.json` | Final E-Swap-3 | `canonical_runner.py` | Derived disruption summary containing baseline and event-window rates, dip percentage, interruption and recovery timings, action duration, loss, duplicates, and latency percentiles. `placebo_offset_ns`, `placebo_event_min_rate_msg_s` and `placebo_dip_percent` apply the same dip estimator to the event window moved to the declared placebo instant, 6 s before the action, against the same baseline. |
| `rule-update.json` | Final E-Swap-3 `ekuiper-rule-update` | `canonical_runner.py` | The rule `pipeline_a`, the updated SQL, the emission metric, and every REST call in order: one `PUT /rules/pipeline_a` (200) whose body is the audited `pipeline_a` rule with only the SQL bound raised and `"triggered": false` added, one `POST /rules/pipeline_a/start` (200) without a body, then each `GET /rules/pipeline_a/status` until one returns 200 with `sink_mqtt_0_0_records_out_total` above 0, the make-before-break arm's signal. Each call keeps its method, path, the rule it sent and its SQL (both `null` for the start and a status read), HTTP status, body, start and end offsets in nanoseconds from the monotonic action start, and start and end Unix-epoch timestamps. |
| `rule-replacement.json` | Final E-Swap-3 `ekuiper-make-before-break` | `canonical_runner.py` | The retired rule `pipeline_a`, the replacement rule `pipeline_a_v2` and its SQL, the emission metric, `status_polls`, and the REST calls in order: `POST /rules` (201), the last `GET /rules/pipeline_a_v2/status` (200, its body showing `sink_mqtt_0_0_records_out_total` above 0) and `DELETE /rules/pipeline_a` (200). Each call keeps its HTTP status, body, and start and end offsets in nanoseconds from the monotonic action start. |
| `burst-source-timing.json`, `burst-source-summary.json` | Final E-Swap-4 | `BenchSource` | Source-origin timestamp, monotonic source-completion offset, fixed phase boundaries, and exact intended/emitted populations for the 1,000 to 2,000 to 1,000 msg/s burst. |
| `swap-actual-t0.json` | Final E-Swap-4 | `canonical_runner.py` | Runner-owned actual-`t0` receipt declaring the source origin, scheduled swap timestamp, actual request timestamp, signed alignment error, and fixed 10 ms tolerance for the single measured swap. |
| `burst-timeline.json` | Final E-Swap-4 | `BenchSource` / `canonical_runner.py` | One 1,000 to 2,000 to 1,000 msg/s burst with boundaries at measured seconds 55 and 65 and exactly one successful swap scheduled at second 60, plus actual alignment, phase populations, primary/drain completion evidence, sequence integrity, sink gap, and internal swap phases. |
| `startup-preparation.json` | E-Perf-9 | `run-experiment.sh` | Filesystem-cache condition and preparation action completed before the timed runtime process starts. |
| `startup.json` | E-Perf-9 | `wafer-runtime` | Monotonic process/config, component load/compile, instantiation, pipeline setup, and first-process durations; total startup duration; exactly-one-message proof; plugin SHA-256; and explicit compiled-component cache state. |
| `containment.json` | E-Iso-1..8 | `canonical_runner.py` | Containment verdict for one attack condition: expected condition, attack node, expected mechanism (the `per_node_metrics.csv` column that must count the attack, see `eval/scripts/lib/containment.py`), its count, unexpected outcomes on the attack node, trap total, runtime-panic flag, healthy-node output count, per-node runtime metrics, and the dead-letter evidence: `dlq_sent_total` (sum over nodes) and `dlq_records` (lines in `dlq.jsonl`, null when the file is absent). The verifier rejects an E-Iso-1..6 leaf whose `dlq.jsonl` exists but holds a different number of records than its nodes sent to the dead-letter queue. An attack counts as contained only when the expected mechanism stopped it at least once and nothing else happened on that node: no other trap kind or guest error, and no message passed on. `contained` is null for conditions without an attack. The runner fails the run when `per_node_metrics.csv` is malformed or lacks the attack node's counters. The analysis never counts a record whose condition differs from the attack, and it counts runs that were not contained or recorded a runtime panic instead of dropping them. `contained: false` is a `containment-escape` outcome, not a rejected leaf, and a run whose runtime exited before it wrote `containment.json` counts as not contained. |
| `branch-a/`, `branch-b/` | E-Iso-7 | `BenchSink` | Independent post-warmup latency histogram, throughput series, sequence accounting, and measurement window for each branch. Each branch has its own `BenchSource`; root-level fan-out/fan-in measurements are forbidden for branch-impact analysis. |
| `branch-isolation.json` | E-Iso-7 | `canonical_runner.py` | Branch-local source identity, configured post-warmup target count, actually offered/received post-warmup counts, target shortfall, throughput samples, latency percentiles, measurement boundaries, and explicit units. |
| `ekuiper-audit.json` | Every eKuiper run: E-Perf-1, E-Perf-2, E-Perf-10, E-Perf-Capacity-Knee, the capacity scout, the E-Swap-3 `ekuiper-rule-update` and `ekuiper-make-before-break` arms and E-Compare-eKuiper-Profile | `canonical_runner.py` | Taken before warm-up: the package version and install receipt, the effective `kuiper.service` properties and unit text with its SHA-256, the path and SHA-256 of `/etc/kuiper/mqtt_source.yaml` (which must equal `eval/ekuiper/mqtt-source-default.yaml`) and of `/etc/kuiper/kuiper.yaml`, the process tree with each process's `Cpus_allowed_list`, the active stream and rule, and the seed script's dry run. The active `pipeline_a` must equal the dry run's `rule_payload`; eKuiper keeps a rule's last definition across restarts, so a `pipeline_a` that an E-Swap-3 rule update left at the raised bound, or any other drift, fails the attempt as infrastructure before warm-up. `metadata.json` names it and its SHA-256 under `comparator_audit`. |
| `ekuiper-health.json` | Same as `ekuiper-audit.json` | `canonical_runner.py` | The `kuiper` unit and `pipeline_a` before warm-up, and the unit and the rule the run ends with after it; see [eKuiper health](#ekuiper-health). |
| `ekuiper-runtime-summary.json` | E-Compare-eKuiper-Profile | `canonical_runner.py` | Diagnostic run identity, interval alignment, latency percentiles, bounded external `/proc` process summary when available, and the Go GC trace summary for the measurement window (`gc_runtime_metrics`) when the profiled run logged one. It never infers GC events from RSS or latency. |
| `ekuiper-gctrace.log` | E-Compare-eKuiper-Profile | `canonical_runner.py` | One line per Go GC cycle that the `kuiper.service` journal recorded from eKuiper start to stop: the journal receive time in Unix-epoch nanoseconds, a space, and the unchanged `GODEBUG=gctrace=1` line. Empty in the unprofiled control. |
| `profiler-overhead.json` | E-Compare-eKuiper-Profile | `canonical_runner.py` | Profiler state, matched rate/run pair, collection-enabled flag, and the run-level profiled-minus-control estimator label. It declares association-only interpretation and is not primary evidence. |
| `swap_requests.json` | E-Swap-1, E-Swap-2, E-Swap-3 (`wafer-hotswap`), E-Swap-4, E-Swap-5, E-Swap-6 | `canonical_runner.py` HTTP client | One record per API request with request boundaries, monotonic `request_duration_ns`, HTTP status, and the runtime's typed internal outcome phases. E-Swap-3 records its one request, to `wafer_threshold_filter_v2.wasm`, with the plugin, HTTP status and response body only; `disruption-timeline.json` holds its action boundaries. E-Swap-5 requires 50 process-trap requests whose response status is `rolled_back`. |
| `swap_timeline.json` | E-Swap-1, E-Swap-2, E-Swap-4, E-Swap-6 | `BenchSink` | Sink-observed successful plugin-version transitions. Each `pause_ns` is an output interarrival gap and is not an internal swap duration. E-Swap-5 forbids this artifact because the rejected v2 never becomes sink-observed. |
| `hotswap-analysis.json` | E-Swap-1, E-Swap-2, E-Swap-4, E-Swap-6 | `canonical_runner.py` | Index-matched API and sink observations with explicit `*_ns` names: internal phases, `http_total_ns`, and `sink_observed_output_gap_ns`. Includes the unique measurement source leaf so shared E-Swap-2/6 views do not multiply samples. |
| `rollback.json` | E-Swap-5 and candidate rollback sessions | `canonical_runner.py` | Request-indexed compile, instantiate, signal, and rollback durations plus exact attempt/success counts and full-run sequence evidence; loss or duplication is a system outcome. It contains no successful-v2 sink transition. |
| `post-rollback-continuity.json` | E-Swap-5 | `canonical_runner.py` | Explicit output observed after the final rollback, with final request identity, bounded observation interval, message count, interval-metrics provenance, and full-run sequence evidence. |

### Ownership summary

- `wafer-runtime` owns `runtime-provenance.json`, E-Perf-9 `startup.json`,
  in-process `latency.hdr`, `service.hdr`, `source-lag.hdr`,
  `throughput.csv`, `measurement-window.json` and bounded
  `interval-latency.json` (via `BenchSink`), `sequence.csv` (via `BenchSink` when `track_sequences=true`),
  sink-observed `swap_timeline.json` for E-Swap-1/2/4/6,
  `memory.csv` and `memory-clock.json` (via `MemoryRecorder`), `queue-depth.csv`
  and `queue-depth-clock.json` (via `QueueDepthRecorder`),
  `recovery.csv` (exact recovery samples), `dlq.jsonl` (via the configured
  file DLQ sink), and `per_node_metrics.csv`
  (via `PipelineOrchestrator::export_per_node_metrics`).
- `canonical_runner.py` owns `containment.json` for E-Iso-1..8, derived from runtime-emitted `per_node_metrics.csv` plus `stdout.log` panic detection.
- `canonical_runner.py` owns E-Perf-4 `service-percentiles.json`, summarised from the runtime's `service.hdr` with `wafer-loadgen hdr-summary`.
- `wafer-loadgen subscribe` owns `subscriber-metadata.json`,
  `latency.hdr` (E2E path), `sequence.csv`, bounded `interval-latency.json`,
  bounded `throughput-buckets.json`, and historical opt-in `received.csv` traces.
- `wafer-loadgen publish` owns `publisher-summary.json` and historical opt-in
  `published.csv` traces.
- `canonical_runner.py` owns final E-Perf-10 `capacity-run.json`,
  `resource-usage.csv`, `process-audit.json`, and the composed `interval-metrics.json`;
  historical diagnostics retain
  `rate-sweep.json`. It also owns final E-Swap-3 `disruption-timeline.json`,
  `disruption-analysis.json`, `rule-update.json` and `rule-replacement.json`, final
  E-Swap-4 `swap-actual-t0.json`
  and `burst-timeline.json`, E-Backpressure `backpressure.json`,
  E-Swap `swap_requests.json` and `hotswap-analysis.json`, final E-Swap-5
  `rollback.json` and `post-rollback-continuity.json`, plus E-Iso-7
  `branch-isolation.json`.
- `run-experiment.sh` owns
  `metadata.json`, `config.toml`, `stdout.log`, the E2E `throughput.csv`, and E-Perf-9
  `startup-preparation.json`.
- `eval/scripts/lib/pi_telemetry.py` owns `pi-telemetry.csv`, `pmic-rails.csv`
  and `power-boundary.json`; `eval/scripts/lib/proc_telemetry.py` owns
  `cpu-cores.csv`, `host-sched.csv`, `sut-processes.csv` and
  `host-sidecar.json`. Both are sidecar processes the canonical runner and
  `run-experiment.sh --canonical` start before the runtime and stop after it
  exits. Both pin themselves to the support CPUs (`--pin-cpus`) before they
  start sampling, so neither they nor the commands they run share a CPU with
  the SUT; neither reads or changes anything the runtime measures.

### Schema table

`eval/result-schema.json` lists, per producer, the CSV header or the top-level
JSON keys of every file the runtime (`BenchSink`, the memory and queue
recorders, `per_node_metrics.csv`, `recovery.csv`, `runtime-provenance.json`),
`wafer-loadgen subscribe`, `wafer-loadgen publish` and `run-experiment.sh`
(the E2E `throughput.csv`) write, and which `latency.hdr` encoding each uses. Three tests keep it, the
writers and this document in step:

- `crates/wafer-runtime/tests/result_contract.rs` runs a small native
  pipeline and fails when the runtime writes a file the table does not list,
  omits one it lists, or writes a different header or key set; it also fails
  when this document does not name a listed file, header or key.
- `crates/wafer-loadgen/tests/signals.rs` checks the subscriber and publisher
  files the same way.
- `eval/scripts/tests/test_write_throughput.py` checks the E2E
  `throughput.csv` header.

`startup.json` and `swap_timeline.json` are left out: they need a Wasm plugin
and have their own tests (`startup_phases.rs`, `hotswap_success.rs`).

### Final amended contract

The `final_campaign` object in `eval/canonical-matrix.json` is the executable source of truth. It fixes seed 1729, explicit fuel-plus-epoch metering, eKuiper concurrency 1, the five-rate common capacity grid, 2,351 schedule records, and 2,121 executed or static measurement leaves. These counts describe the common schedule that every host shares. Each host's final batch adds its own E-Perf-10 bracket rates: 120 leaves and 3.00 nominal hours per rate, at most six rates, so at most 720 leaves and 18.00 nominal hours per host, and its `batch.json` records what they add (see [Per-host bracket rates](#per-host-bracket-rates)). Every final experiment has `thesis_evidence=true`.

Every MQTT arm (WAFER, native, eKuiper and MQTT loopback) of every experiment that measures over MQTT ends its run the same way. `final_campaign.mqtt_drain_grace_secs` (5 s) is the one drain grace. The warmup publisher numbers its messages from the measured count `N` up, and the subscriber declares the measured range `[0, N)` with `--sequence-end-exclusive N` and `--total-messages N`, so it ignores late warmup messages and counts every measured number it never received as a gap, tail included. The subscriber stops on its own once it has received all `N` measured numbers; a duplicate does not count toward `N`. After the measured publisher exits, the subscriber has the drain grace to get there; if it has not, the harness stops it with SIGINT and it writes its artefacts. The grace is counted from the publisher's exit time, the window's `finished_ns`, so SIGINT lands at that time plus 5 s in both harnesses. A run is never rejected or retried because messages were lost: the gaps are the run's loss, and the subscriber's interval rows are bounded to cover the publisher's exit drain and the grace (`--drain-grace-secs`), so a run that ends at the grace is never partial. The subscriber does not mark which messages arrived during the grace, so loss is reported only as messages that never arrived. Final E-Perf-1 to E-Perf-3 require `publisher-summary.json` and `subscriber-metadata.json`, which pass the same summary checks as E-Perf-10 and E-Swap-3, and the verifier rejects a final E-Perf-1 to E-Perf-3 or E-Swap-3 leaf whose `sequence_end_exclusive` and `sequence.expected` differ from the publisher's `intended`. For those leaves and final E-Perf-10 it also rejects an `interval-metrics.json` whose `drain_grace_ns` is not the matrix grace, and a subscriber whose `ended_at_ns` is more than the grace plus 0.5 s after the window's `finished_ns`. Target-load analysis takes received, duplicate and missing totals from `subscriber-metadata.json`, and the E-Perf-3 depth analysis reports each depth's median loss from the same totals next to its latency.

Final E-Perf-4 runs eight conditions: a WAFER pass-through transform at 120 B, 1 KiB, 10 KiB and 100 KiB, and a native pass-through arm (`native-120b`, `native-1kb`, `native-10kb`, `native-100kb`) at each of the same sizes. Each native config matches its WAFER config except for the `[pipeline]` name and description, the transform plugin and the absent `[engine]` section, so the native runs record `effective_metering_mode = "neither"`. All eight conditions share one randomised block per run index. The estimand per payload size is the median over run indices of WAFER service time minus native service time, at p50, p95 and p99, with a run-level bootstrap 95% CI over the pairs. The difference covers the whole Wasm stage, including metering and the copies into and out of guest memory. Every leaf requires `latency.hdr`, `service.hdr`, `service-percentiles.json`, `throughput.csv` and `sequence.csv`. The matrix records the measurement boundary as the in-process path from `bench-source` through one pass-through transform to `bench-sink`. Results describe that path only: the MQTT source and sink keep rumqttc's default 10 KiB packet limit, so the 10 KiB and 100 KiB sizes cannot pass through an MQTT-bookended pipeline and no MQTT payload result is claimed.

Final E-Perf-5 uses explicit transform fuel and epoch protection. Its transform-only pipeline records `filter = null` and `router = null` because those node categories are absent; this is a declared matrix exception, not an unmetered WAFER run.

E-Perf-1 and E-Perf-5 run all their conditions in one randomised block per run index, so analysis pairs each WAFER run with the native or eKuiper run of the same index and resamples those pairs. E-Perf-5 has a descriptive estimator on each host: `overhead_contrast_table` pairs the WAFER and native run p50 and reports the median WAFER over the median native (`median_ratio`) and the median over pairs of WAFER minus native (`difference_ns`), each with a bootstrap 95% CI over run pairs. It carries no verdict and makes no cross-architecture claim. `04-cross-arch.ipynb` saves it as `e-perf-5-wafer-native-contrast` with a leading `host` column holding the host tag (`rpi5`, `jetson` or `x86`). `wafer_analysis.paths.resolve_host_batches()` picks the hosts: in canonical mode each host's batch is the one its own `eval/final-batches.json` entry approves, so a batch is never reported under another host's tag, and a host without an entry has no row; explicit `E_PERF_5_RPI_DIR`, `E_PERF_5_JETSON_DIR` or `E_PERF_5_X86_DIR` paths make every host diagnostic. E-Perf-1 adds `target_contrast_table`, the same paired contrast for run p95 and run p50, with the CI half-width and the minimum detectable difference of that design at the observed spread:

```text
MDD = (z(1 - alpha/2) + z(power)) * SD(WAFER - native over pairs) / sqrt(N pairs)
alpha = 0.05 two-sided, power = 0.80, so the factor is 1.960 + 0.842 = 2.802
```

The MDD is a normal approximation for a mean paired shift: it says how large a WAFER minus native difference this design detects, not whether one exists. The contrast carries no verdict. Both contrast tables have one row per statistic with these columns:

| Column | Meaning |
|---|---|
| `statistic` | Run-level latency percentile compared: `p95` or `p50` |
| `condition`, `reference_condition` | `wafer` and `native` |
| `N_pairs` | Run indices with both a WAFER and a native run that the system under test did not stop early |
| `runs_stopped_early` | WAFER and native runs the system under test stopped early; each is left out together with its partner run |
| `wafer_median_ns`, `native_median_ns` | Median of the statistic over the paired runs of each arm |
| `median_ratio`, `ratio_ci95_low`, `ratio_ci95_high` | WAFER median over native median with a bootstrap 95% CI over run pairs |
| `difference_ns`, `difference_ci95_low_ns`, `difference_ci95_high_ns` | Median over pairs of WAFER minus native with a bootstrap 95% CI over run pairs |
| `difference_ci_half_width_ns` | Half the width of that interval |
| `paired_sd_ns` | Sample standard deviation of the paired differences; empty with one pair |
| `mdd_ns` | Minimum detectable difference from the formula above; empty with one pair |

The E-Perf-1 target-load table reads the WAFER/eKuiper p95 ratio, its two-sided 95% interval (`ratio_ci95_low`, `ratio_ci95_high`, on every system's row against eKuiper) and Cliff's delta against eKuiper from the same run pairs.

Final E-Perf-10 requires `capacity-run.json`, `publisher-summary.json`, `subscriber-metadata.json`, `latency.hdr`, `throughput.csv`, `sequence.csv`, `resource-usage.csv`, and `process-audit.json`; a run the runtime did not survive has no `capacity-run.json`. Such a run counts toward its rate's 30 runs as `sut_outcome_runs` in `rate-sweep-summary.json` and makes that rate delivery-bad; the rate's loss, achieved-rate and p99 figures cover the runs that completed. `capacity-run.json` uses this counter identity:

```text
intended = rejected + enqueued
enqueued = acked
acked = received_unique + downstream_lost
received_events = received_unique + duplicates
total_undelivered = rejected + downstream_lost
```

It must contain no mandatory per-message traces. `check_capacity_run_result` rejects missing fields, inconsistent counters, non-final evidence labels, or trace mode.

The batch summary `manifests/canonical-batches/<host-tag>-<batch-id>/rate-sweep-summary.json` carries, per system and rate, `run_count` (completed runs), `sut_outcome_runs`, `pooled_loss`, `mean_achieved_ratio`, `total_duplicates`, and one value per completed run of achieved rate, achieved ratio, loss, and p99; `run_count + sut_outcome_runs` is 30. Canonical analysis classifies every cell from these counters, never from the stored `classification` string: a cell is delivery-good when it has no outcome run, pooled loss is at most 0.01, mean achieved ratio is at least 0.99, and the duplicate total is zero (the `per-rate-cell` rows of `verdict_rules`), and a SUT cell at or above the lowest delivery-bad MQTT loopback rate is support-confounded. A stored classification or support-censoring rate that disagrees with the counters, or a pooled loss or mean achieved ratio that disagrees with the run values, rejects the summary. The rate table reports run-level min, quartiles, max, and a bootstrap 95% CI of the median for achieved rate, achieved ratio, loss, and p99; the pooled loss with a run-resampling bootstrap 95% CI; the mean achieved ratio; the duplicate total; and the normalized p99, whose interval resamples both that rate's runs and the 1,000 msg/s runs. These statistics cover the completed runs, and a rate with no completed run reports none. `rate_points_msg_s` lists the tested rates, the common grid and the batch's bracket rates together, and `bracket_rate_points_msg_s` lists the bracket rates alone. Any strictly increasing grid that contains 1,000 msg/s is accepted.

Each system's delivery ceiling is bracketed by tested rates, not read as one grid point. The tested rates are the common grid plus the host's bracket rates. The lower bound is the highest delivery-good tested rate with no delivery-bad tested rate below it, or zero. The upper bound is the lowest delivery-bad tested rate above every delivery-good one; it is unbounded when no such rate exists, as when the top of the grid is delivery-good. A delivery-bad rate below a delivery-good one is flagged as non-monotonic and widens the bracket to cover both readings instead of invalidating the population. Support-confounded cells set neither bound, so a ceiling with only support-confounded cells above its lower bound is unbounded above. The WAFER/eKuiper ratio interval runs from WAFER lower / eKuiper upper (worst case) to WAFER upper / eKuiper lower (best case, unbounded when the eKuiper lower bound is zero). The decision is `PASS` when the worst case is at least 0.70, `FAIL` when the best case is below 0.70, and `CENSORED` otherwise, with 0.70 read from the `tested-rate-bracket` row of `verdict_rules`; an incomplete or malformed population is `PENDING`. The criterion is a statement about the tested grid: WAFER's tested-grid delivery ceiling is at least 0.70 of eKuiper's, with each ceiling bracketed by tested rates. The claim-boundary table reports both systems' bounds, the ratio interval, and the verdict.

Final E-Swap-3 applies one control action at measured t=60 to Pipeline A in four arms. `wafer-hotswap` hot-swaps the WAFER threshold filter for `threshold-filter-v2`; `wafer-restart` restarts the WAFER runtime; `ekuiper-rule-update` is the eKuiper rule update, sent as `PUT /rules/pipeline_a` with `"triggered": false` and followed by `POST /rules/pipeline_a/start`: the PUT saves the audited `pipeline_a` definition with the raised bound and stops the rule, the start runs it, and the harness then waits until the updated rule's sink counts output; `ekuiper-make-before-break` creates `pipeline_a_v2` on the shared stream, waits until that rule's own sink counts output, and only then deletes `pipeline_a`. The replacement plugin, the updated rule and the replacement rule make the same change, raising the lower temperature bound from 50 to 60, and the workload's constant 72.5 passes both versions, so the delivered output stays the same. The arms are reported side by side; no arm passes or fails against another.

Final E-Swap-3 requires `publisher-summary.json`, `subscriber-metadata.json`, 200 contiguous 100 ms buckets in `throughput-buckets.json`, 400 contiguous 10 ms buckets and 40 nested 100 ms parent buckets in `throughput-buckets-10ms.json`, `disruption-timeline.json`, and `disruption-analysis.json`. The 100 ms series stays aligned to the actual action-start `t0`, and the parent buckets equal the matching canonical event-window slice. `disruption-timeline.json` records the actual action start immediately before control issuance, the actual acknowledged/readiness wall-clock end, independent monotonic action duration, the scheduled measured t=60 target, actual event offset, signed alignment error, and a fixed 10 ms tolerance. `swap_timeline.json` is not a final E-Swap-3 artifact and must not remain in the admitted raw leaf. The leaf fails when the actual action start misses the scheduled target by more than 10 ms. Loss or duplication in a `wafer-hotswap` run is a `message-loss` or `duplicates` outcome; the three comparators have no zero-loss criterion, so their loss is reported data. A `wafer-hotswap` run records its one request in `swap_requests.json`; a request that does not return HTTP 200, or whose body reports `rolled_back` or does not report `replacement_adopted: true`, is a `swap-failed` outcome. A `wafer-hotswap` run on a host where `threshold-filter-v2` is not built fails as infrastructure before warm-up instead, because the runtime would refuse the swap for a missing file. An `ekuiper-rule-update` run records its REST calls in `rule-update.json`. eKuiper answers the start only after the updated rule has started, so that answer ends the action, as the runtime's answer ends a hot swap; the make-before-break action ends with its delete, which waits for the replacement's output. The verifier rejects the leaf unless the calls are one `PUT /rules/pipeline_a` that returned 200 and sent the rule recorded in `ekuiper-audit.json` with only `temperature >= 50` raised to `temperature >= 60` and `"triggered": false`, then one `POST /rules/pipeline_a/start` that returned 200 and sent no body, followed by status reads of which only the last returns 200 with `sink_mqtt_0_0_records_out_total` above 0; unless every read before that last one ended less than 10 s after the start returned, the make-before-break arm's deadline; unless the calls are in order on both clocks; and unless the PUT starts within the action and the start's end is the action's end on both clocks. The update is not sent as a single PUT of a triggered rule, because in eKuiper 2.1.5 that form leaves a planned topology's input channel attached to the shared stream (see "E-Swap-3 rule changes" in `docs/benchmarks/ekuiper-comparator.md`). An `ekuiper-make-before-break` run records its REST calls in `rule-replacement.json`, and the verifier rejects the leaf unless the calls are exactly the create, the status read that shows output and the delete, in that order and within the action, and unless the replacement SQL is the audited SQL with the same change. Each of the three records is rejected on an arm it does not belong to. `disruption-analysis.json` also reports a placebo dip: the same estimator with its event window moved by `placebo_offset_secs` (-6 s, declared in the frozen `event_alignment`) to [-8 s, -4 s], inside the baseline window and 2 s before the event window. No action happens there, so the placebo dip shows how far the minimum of a 4 s window falls below the baseline median from noise alone. The analysis reports it next to the dip for every arm as a descriptive noise floor; it has no verdict and leaves the dip rule unchanged. A runtime that exits during the run, including one restarted by `wafer-restart` that dies, is a `runtime-exit` outcome; a restarted runtime that keeps running without answering its control plane is an infrastructure failure.

Final E-Swap-4 uses a source-driven 1,000→2,000→1,000 msg/s schedule over measured intervals `[0,55)`, `[55,65)`, and `[65,120)` after 30,000 warmup messages. Each independent run contains exactly 130,000 measured messages and one swap scheduled at measured t=60. `throughput-buckets.json` contains 1,200 contiguous source-origin 100 ms primary buckets over `[0,120s)` plus a separate 100-bucket drain series over `[120s,130s)`. Full-run counters reconcile primary, drain, and O(1) after-drain evidence; any after-drain receive or right-censored drain rejects the run. `swap-actual-t0.json` declares the single source-origin actual-`t0` receipt, and it must reconcile with `swap_requests.json`, `throughput-buckets-10ms.json`, `throughput-buckets.json`, and `burst-timeline.json` on source origin, scheduled t=60, actual request timestamp, alignment error, and the fixed 10 ms tolerance. `burst-timeline.json` records source and actual swap boundaries, monotonic source-completion offset, intended/emitted/received phase populations, sequence loss/duplication, primary/drain completion evidence, the sink-observed gap, and internal swap phases. The semantic verifier rejects wrong clocks, origins, phase rates or populations, zero or multiple swaps, a missing or inconsistent actual-`t0` receipt, a swap outside the burst, a non-centered scheduled swap, bucket gaps, clamped/misclassified tail evidence, completion at or after 130 s, or population mismatches. Loss or duplication is a `message-loss` or `duplicates` outcome and a failed swap a `swap-failed` outcome; the run is admitted and fails the zero-loss criterion. Batch analysis admits exactly one event from each of 30 distinct runs that kept running, counts every admitted run in the zero-loss criterion, before computing the across-run p95 and bootstrap median interval, and separately reports runs with drain arrivals and the maximum drain offset. Missing required files fail through the matrix contract; malformed files fail through experiment-specific semantic checks.

Final E-Swap-1, E-Swap-2, E-Swap-5, and E-Swap-6 use the complete process run as the independent unit: E-Swap-1 and E-Swap-5 each have 10 independent runs, and the 50 swap or rollback events of each run are nested observations. E-Swap-4 uses 30 independent runs with one nested swap each. E-Swap-2 and E-Swap-6 are alias views of the 10 E-Swap-1 runs and contribute zero additional independent N.

Each process run starts with an empty in-memory compile cache, so the first swap of a run compiles its replacement and the later swaps reuse the cached component. Batch analysis takes the class from the `compile_cache` value the runtime returns for each request: `compiled` is first-use and `memory_hit` or `disk_hit` is cached. A canonical E-Swap-1 or E-Swap-5 run must hold exactly one first-use event, its first, so 10 runs give 10 first-use observations. Analysis reduces each run first, to its first-use value and to the median of its cached events, plus the p95 of its cached E-Swap-1 phase totals and sink gaps, and then reports the median of those run values with a bootstrap 95% CI over runs; nested events never enter the CI as independent samples. A run the system under test stopped early still counts among the 10 runs as an outcome and contributes no timing values. Loss and duplication (E-Swap-2) and rollback success and post-rollback output (E-Swap-5) are reported for every run and as totals over the 10 runs.

Each final E-Swap-5 run contains 50 nested failed-replacement events. Each request must return `rolled_back`; a request that does not is a `rollback-failed` outcome. `rollback.json` reconciles all request-indexed internal phases and the full-run sequence, where loss or duplication is a system outcome. `post-rollback-continuity.json` proves output after the final rollback and explicitly records that no successful v2 transition was observed. A retained or synthesized `swap_timeline.json`, missing continuity, or any request/rollback mismatch rejects the leaf.

Final E-Density-1 is a static source-bound measurement. The canonical runner invokes `eval/scripts/collect-binary-sizes.sh` instead of `run-experiment.sh`, requires one `binary-sizes.csv` row for every non-comment entry in `eval/scripts/binary-sizes.index`, and records Pi host, source provenance, governor, throttling, telemetry, and measurement-window evidence. Its metadata uses `system = "static"` and `exit_codes.collector = 0`; runtime/Wasmtime provenance is intentionally inapplicable because no WAFER runtime process executes.

Each `binary-sizes.csv` row holds only measured sizes: `plugin`, `wasm_bytes` and `wasm_kb`. The collector also copies the measured container floor for the host architecture, `eval/container-floor/linux-arm64.json` or `linux-amd64.json`, into the leaf as `container-floor.json`, and fails when that file is missing. The floor is measured, not estimated: `eval/scripts/measure-container-floor.py` runs on a machine with Docker before deployment and builds a `FROM scratch` image whose only file is `/worker`, a statically linked Rust stdin-to-stdout pass-through (`eval/container-floor/worker`) compiled with the plugins' release profile in a digest-pinned `rust:<toolchain>-alpine` image, where `<toolchain>` comes from `rust-toolchain.toml`. It builds and then runs the image once with no network and requires its input back unchanged, then reads the image that `docker save` exports. `image_bytes` is the uncompressed size of the single layer, checked against the layer's `diff_id`, and `binary_bytes` is the size of `/worker` inside it. The file also records `platform`, `build_image`, `rust_toolchain`, `inputs_sha256` (the SHA-256 of the Dockerfile and each worker source file), `layer_diff_id`, `docker_server_version` and `measured_at`. The verifier rejects a floor that is not a one-layer `FROM scratch` image, whose worker is larger than its image, whose platform does not match the host architecture, whose build image is not pinned by digest, or that lacks its input hashes. The eval-script tests fail when a committed floor no longer matches its build inputs or the pinned toolchain. The floor is one pass-through image, not an image per plugin; analysis reports it as one reference size beside every component.

### Verdict rules and declared thresholds

`verdict_rules` in `eval/canonical-matrix.json` declares every threshold that decides a criterion, once. Each row of `verdict_rules.thresholds` gives the `criterion`, the `experiments` it applies to, the `value`, its `unit`, the `direction` (`<`, `<=`, `>=` or `>`), the `statistic` compared with the value, the `rule` that turns the statistic into a verdict, the `role` (`criterion`, `reference` or `gate`), the `origin` of the value, and `pilot_data_visible`: `true` when pilot measurements existed when the value was set, `false` when the value was written before them, `unknown` when the history does not say. The `origin` text also dates the later choice of statistic where that came after pilot data. `eval/scripts/validate-canonical.py` rejects a matrix whose table is missing or malformed, and checks that the E-Perf-10 `capacity_envelope` and the E-Swap-3 `event_alignment` declare the same values as the table. The analysis reads the table through `wafer_analysis.verdicts.declared_thresholds()`; no table builder keeps its own copy of a threshold, so changing a value in the matrix changes the verdict.

| Rule | Verdicts | Decision |
|---|---|---|
| `one-sided-bound` | `PASS`, `FAIL`, `INCONCLUSIVE`, `PENDING` | A percentile bootstrap over runs, or over run pairs where the design pairs runs by index, gives the one-sided 95% lower and upper bounds of the statistic: the two ends of its two-sided 90% interval. `PASS` when the bound on the favourable side meets the threshold, `FAIL` when the bound on the other side misses it, `INCONCLUSIVE` otherwise. `PENDING` when no run gives the statistic a value. |
| `exact-count` | `PASS`, `FAIL` | Zero tolerance: the count of violating runs or messages must be 0. One violation fails; no interval is drawn. |
| `every-run` | gate passed or not | Every run's value must lie inside the band. |
| `per-rate-cell` | delivery-good or delivery-bad | Point rules on one E-Perf-10 rate cell's pooled counters; the cells feed the bracket. |
| `tested-rate-bracket` | `PASS`, `FAIL`, `CENSORED`, `PENDING` | The E-Perf-10 ratio interval between bracketed delivery ceilings, as described above. |

A `one-sided-bound` criterion adds five columns, named after its prefix, to the table the notebook saves: `<prefix>_verdict`, `<prefix>_estimate` (the point estimate of the bounded statistic), `<prefix>_threshold` (the declared value), `<prefix>_flips_at` and `<prefix>_ci_half_width`. `flips_at` is the bound the threshold has to cross for the verdict to change: the favourable bound for `PASS`, the other bound for `FAIL`, and the nearer bound for `INCONCLUSIVE`. `ci_half_width` is half the distance between the two one-sided bounds. A row that combines criteria also has `verdict`: `FAIL` when any of them fails, otherwise `PENDING` when any is pending, otherwise `INCONCLUSIVE` when any is inconclusive, otherwise `PASS`. An `exact-count` criterion that replication compares adds three columns: `<prefix>_verdict`, `<prefix>_estimate` (the count) and `<prefix>_threshold`; E-Perf-1 duplicates is the only one so far. The two-sided 95% intervals in the `*_ci95_*` columns stay descriptive. Diagnostic batches get the same columns and stay labelled `thesis_evidence=false`.

| Criterion | Threshold | Statistic | Rule | Columns |
|---|---|---|---|---|
| E-Val-1 | `>= 45 ms` and `<= 55,017,471 ns` | p99 of each run with the injected 50 ms delay | `every-run` (gate) | `gate_passed` |
| E-Perf-1 latency | `<= 2.0` | median WAFER run p95 / median eKuiper run p95; run pairs of the same index resampled together | `one-sided-bound` | `p95_ratio_*`, `verdict` |
| E-Perf-1 loss | `<= 0.01` | pooled loss over one system's runs | `one-sided-bound` | `loss_*`, `delivery_verdict` |
| E-Perf-1 delivery | `>= 0.99` | mean achieved / offered ratio over one system's runs | `one-sided-bound` | `achieved_ratio_*`, `delivery_verdict` |
| E-Perf-1 duplicates | `<= 0` | duplicates over one system's runs | `exact-count` | `duplicates_*`, `delivery_verdict` |
| E-Perf-4 | `< 50,000 ns` | median over run pairs of WAFER minus native service p50 | `one-sided-bound` (reference, not a pass criterion) | `per_hop_reference_*` |
| E-Perf-10 cells | loss `<= 0.01`, ratio `>= 0.99`, duplicates `<= 0` | pooled counters of one system at one tested rate | `per-rate-cell` | `classification` |
| E-Perf-10 | `>= 0.70` | WAFER / eKuiper tested-grid delivery ceiling | `tested-rate-bracket` | `competitive_*` |
| E-Iso-1..6 | `<= 0` | runs not contained, counting runs the runtime did not survive | `exact-count` | `all_contained` |
| E-Iso-7 drop | `< 1 percent` | drop of the median branch-A throughput below the control median; each condition's runs resampled apart | `one-sided-bound` | `drop_*`, `verdict` |
| E-Iso-7 survival | `<= 0` | attack runs the runtime did not survive | `exact-count` | `verdict` |
| E-Swap-2 | `<= 0` | E-Swap-1 runs with loss, duplicates or an early stop | `exact-count` | per-run counts and totals |
| E-Swap-3 dip | `< 5 percent` | median dip over the WAFER hot-swap runs | `one-sided-bound` | `dip_*`, `verdict` |
| E-Swap-3 integrity | `<= 0` | hot-swap runs with loss, duplicates or an early stop | `exact-count` | `zero_loss_and_duplication`, `verdict` |
| E-Swap-4 gap | `< 100 ms` | nearest-rank p95 over runs of the sink-observed gap | `one-sided-bound` | `p95_gap_*`, `verdict` |
| E-Swap-4 integrity | `<= 0` | runs with full-run loss, duplicates or an early stop | `exact-count` | `zero_loss_and_duplication`, `verdict` |
| E-Swap-5 | `<= 0` | runs with a failed rollback, no output after the final rollback, loss or duplicates | `exact-count` | `all_rolled_back`, exact totals |

The table's `origin` column records where each value came from. Values taken from the research questions (2.0, 0.70, 50 microseconds, 1 percent, 5 percent, zero loss and duplication, full containment) were written before any measurement; the forms that read them as run-level medians, upper bounds, brackets or paired differences were fixed later, after pilot data existed. The 100 ms swap pause target predates the runtime. The loss, delivery-ratio and duplicate rules for target load and capacity cells were added after the rate-sweep and capacity scouts. The lower end of the E-Val-1 band is recorded as `unknown` because it was introduced together with the first delay test.

#### Replication concordance

The Raspberry Pi 5 decides every verdict. Jetson and x86 batches replicate it: `replication_concordance` in `eval/canonical-matrix.json`, next to `verdict_rules`, declares the canonical host, the replication hosts, and how a replication host's verdict on one criterion is compared with the canonical one. `eval/scripts/validate-canonical.py` rejects a matrix whose rule names a different canonical host or whose replication hosts disagree with the `hosts` roles. The analysis reads it through `wafer_analysis.verdicts.declared_concordance()` and refuses classes other than the four it applies.

The rule covers the `one-sided-bound` and `exact-count` criteria listed in `criteria_rules`. Direction is the side of the threshold on which a host's point estimate (`<prefix>_estimate`) falls: it meets the threshold or misses it. An exact count is its own point estimate, so its verdict follows its side and it is only ever `same-verdict` or `opposite-direction`. Each criterion and row gets the first class that holds:

| Class | When |
|---|---|
| `not-estimable` | Either host lacks the criterion, reports it `PENDING`, or has no point estimate |
| `same-verdict` | Both hosts report the same `PASS`, `FAIL` or `INCONCLUSIVE` |
| `same-direction` | The verdicts differ and both point estimates fall on the same side of the threshold |
| `opposite-direction` | The verdicts differ and the point estimates fall on opposite sides of the threshold |

Replication never changes a canonical verdict: the concordance table copies it and adds no combined verdict. A combined verdict (`verdict`, `delivery_verdict`) is not a criterion and is not classified, but every criterion it combines is, so a replication host whose combined verdict differs has at least one criterion row that is not `same-verdict`. `concordance_table` takes one verdict table per host, all built by the same table builder, and matches rows by `condition` (or another key). Canonical analysis resolves each host's batch through `wafer_analysis.paths.approved_host_batches()`, which reads that host's entry in `eval/final-batches.json` and validates the batch against it; a host without an entry is missing and its rows are `not-estimable`. The E-Perf-1 notebook applies the rule to `p95_ratio`, `loss`, `achieved_ratio` and `duplicates` and saves `e-perf-1-replication-concordance`. No notebook applies it to another experiment yet: E-Perf-4, E-Iso-7, E-Swap-3 and E-Swap-4 report `<prefix>_estimate` for their bounded criteria, but their exact counts are folded into `verdict` without columns of their own. `e-perf-1-replication-concordance` has these columns:

| Column | Meaning |
|---|---|
| `criterion`, `condition`, `host` | Declared criterion, table row and replication host |
| `canonical_host` | `rpi5` |
| `threshold`, `direction` | Declared value and comparator |
| `canonical_verdict`, `canonical_estimate`, `canonical_side` | The canonical host's verdict, point estimate and side of the threshold (`meets` or `misses`) |
| `replication_verdict`, `replication_estimate`, `replication_side` | The same for the replication host |
| `concordance` | One of the four classes |
| `thesis_evidence` | True only when both rows are canonical evidence |

### Candidate experiment contract

Candidate experiments live in the separate `enhanced_candidate` object in
`eval/canonical-matrix.json`. They do not modify or supersede the primary
estimands.

Every candidate experiment is either `candidate-supplementary` or diagnostic and
sets `thesis_evidence=false` and `n30_admitted=false`. Candidates run as their own
batch with `--experiments candidates`, and the runner refuses to mix them with
final experiments. A candidate becomes thesis evidence only by moving it into the
final `experiments` list before a final batch starts; the batch then records the
changed matrix hash. Candidate runs, attempts, intervals, and events must not be
pooled with final batches or with one another as independent replicates. The
independent sample unit is the host run. Intervals and events are nested observations.

The candidate IDs and purposes are:

| ID | Evidence | Independent sample unit | Candidate purpose |
| --- | --- | --- | --- |
| `e-perf-capacity-knee` | candidate-supplementary | Host run at one system and offered rate | Refine the support and SUT capacity knees without replacing E-Perf-10. |
| `e-perf-payload-refinement` | candidate-supplementary | Host run at one payload size | Refine the observed payload transition. |
| `e-perf-depth-extension` | candidate-supplementary | Host run at one depth | Extend latency and RSS observations through depth 50. |
| `e-swap-independent-sessions` | candidate-supplementary | Host run | Collect five independent swap sessions with 50 nested events each. |
| `e-swap-rollback-sessions` | candidate-supplementary | Host run | Collect five independent rollback sessions with 50 nested events each. |
| `e-compare-ekuiper-profile` | diagnostic | Host run at one rate and profiler state | Associate bounded eKuiper process and Go GC trace summaries with tail latency using matched unprofiled controls. |

The exact condition grids, required outputs, no-pooling boundaries, and analysis
consumers are machine-readable in `enhanced_candidate.experiments`. Under
`--canonical`, the verifier rejects a candidate leaf whose experiment is not
listed there or that lacks one of its `required_outputs`.

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
transition. Each event's label comes from the `compile_cache` value the runtime
returns in its swap response: `compiled` is labeled `first-use-aot`, and
`memory_hit` or `disk_hit` is labeled `cached`. Event 0 in each run must be
`first-use-aot` and events 1–49 must be `cached`; any other outcome fails the
run. Event-level rows stay nested
within their run, while comparisons use one per-run aggregate for each event
class. Sequence evidence must reconcile, loss or duplication is a system outcome, failed attempts remain immutable,
and neither candidate may reference an E-Swap-1/2/5/6 alias as its measurement
source. The 10 ms actual-t0 artifacts remain scoped to E-Swap-3/E-Swap-4; these
multi-event candidates use bounded one-second interval metrics and event timing
records instead of fabricating a single-event 10 ms series.

`e-compare-ekuiper-profile` contains exactly five profiled and five unprofiled
control runs at each of 1,000, 4,000, and 8,000 msg/s. A pair is the profiled
and unprofiled run sharing one offered rate and run index; both use the same
canonical eKuiper 2.1.5 config, load-generator profile, QoS, operator
concurrency, 30-second warmup, and 60-second measurement. The profiled arm alone
starts a one-second external `/proc` sampler bounded to at most 62 rows; it
stops when the publisher exits, before the drain grace. If
required process files are unreadable, the run continues and records process
metrics as unavailable. The unprofiled control must not contain
`resource-usage.csv`. The latency intervals start at the first measured
arrival, so a warmup backlog moves them later and can leave fewer than 60
rows. The runtime summary's `interval_alignment` records `row_count` and the
interval file's `maximum_rows`; the run is rejected only when it has no row,
more rows than that bound, or a first measured arrival outside the publisher's
window plus the drain grace. The process sampler is checked against
`measurement-window.json`, the window it sampled.

The profiled arm also starts eKuiper with `GODEBUG=gctrace=1`, through the
runtime drop-in `/run/systemd/system/kuiper.service.d/wafer-gctrace.conf`
installed from `eval/ekuiper/gctrace-drop-in.conf`, so the Go runtime writes one
line per GC cycle to the unit's journal. After eKuiper stops, both arms read the
`kuiper.service` journal since the run started and keep the GC lines in
`ekuiper-gctrace.log`. `gc_runtime_metrics` in the runtime summary then counts
the GC cycles whose journal time falls inside the 60-second measurement window
(`cycle_count`), sums and maximizes their two stop-the-world pauses (sweep
termination and mark termination wall clock, `stw_pause_total_ns` and
`stw_pause_max_ns`), and records the largest heap at GC start, live heap, and
heap goal in MiB. `trace_line_count` and `missing_cycle_count` cover the whole
run, so gaps in the GC numbering are visible. A profiled run whose journal is
unreadable, has no GC lines, or has lines in an unknown format keeps its other
outputs and records `status=unavailable` with `kuiper-journal-unreadable`,
`gctrace-lines-missing-from-journal`, or `gctrace-format-unrecognized`. The
unprofiled control records `gctrace-disabled-by-design`, and any GC line in its
log fails the run. Journal times are receive times, so a cycle is assigned to
the window by when its line was logged, which is when the cycle ended. The cost
of writing the trace is part of the profiled-minus-control difference, and GC
summaries are diagnostic associations with tail latency, not causal evidence.

The runner removes the drop-in, with eKuiper stopped, at the end of every
profiled run, and again before any other eKuiper start if it is still present.
Every eKuiper run outside the profiled arm must show no `GODEBUG` in the unit
environment recorded in `ekuiper-audit.json`: the runner fails such a run at
audit time and the verifier rejects its leaf.

Analysis preserves all 30 independent host runs, forms 15 rate/run-index pairs,
and reports profiled-minus-control differences as diagnostic associations only.
The profile batch cannot be pooled with E-Perf-1, E-Perf-10, or prior diagnostic
rehearsals, and cannot support a GC-causality claim.

#### Bounded interval outputs

Applicable runs add `interval-metrics.json`, with one-second p50/p95/p99,
throughput, CPU, RSS, PMIC-proxy, temperature, and queue summaries. Each row
labels monotonic elapsed `[start,end)` bounds and Unix-epoch boundary values used
only for cross-process alignment. Cardinality is bounded by
`ceil(declared_measurement_duration_ns / 1s) + ceil(publisher_drain_ns / 1s) +
ceil(drain_grace_ns / 1s) + 2`, where an absent field is zero; E-Swap-4 uses only its
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
can receive its terminal receipt. Late or out-of-order interval arrivals,
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
power is future work. E-Perf-5 remains PENDING until an approved x86 batch
exists; the Pi and x86 WAFER/native contrasts are then set side by side as
descriptive results, and no cross-architecture verdict is drawn.
The retained 5 V / 4.2 A supply gets no threshold waiver: every final run must
record `throttled=0x0`, and `approve-batch` refuses a batch with any other value.

### Reduced-repetition diagnostic batches

`canonical_runner.py --repetitions N` runs only runs 1 to N of the frozen
schedule, in the frozen order, with N below the matrix count. Such a batch is
diagnostic. Every leaf records `thesis_evidence=false` and
`diagnostic_repetitions=N` in `metadata.json`; `metadata.json` is the label, and
artifacts such as `capacity-run.json` keep their final schema. The batch skips
alias views (E-Perf-2, E-Perf-8, E-Swap-2, E-Swap-6), whose runs are the source
experiment's, and the batch summaries, which need the full run population. The
verifier warns on every such leaf and rejects one that sets
`diagnostic_repetitions` with `thesis_evidence` other than `false`. The
analysis gate never admits it, because it rejects `thesis_evidence=false` and
requires the matrix's full run population.

### Capacity scout

`canonical_runner.py --capacity-scout --batch-id ID` runs the diagnostic search
that chose the E-Perf-10 rate grid. Each probe is three runs of one system at one
offered rate on the Pipeline A MQTT workload, with a 30-second warmup and a
60-second measurement. The search starts at 500 msg/s, doubles the rate until two
consecutive probes are delivery-bad, then bisects that bracket. A run writes
`capacity-scout.json` under `raw/capacity-scout/<batch>/<system>/`, with
`batch_class=capacity-scout`, `thesis_evidence=false` and no per-message traces.
Decisions are hash-chained under `manifests/capacity-scout/<batch>/decisions/`, and
`scout-complete.json` records the final state of every system. The frozen grid
comes from batch `capacity-scout-v3-20260904T045000Z`, and a later scout batch does not
change it. Each host's own scout sets only that host's E-Perf-10 bracket rates (see
[Per-host bracket rates](#per-host-bracket-rates)). The grid's provenance is the summary
and candidate hashes recorded in `eval/canonical-matrix.json`, not a replay of that
batch: the runner replays a batch against its current system set, so a batch recorded
before the diagnostic arm existed cannot be resumed with the current runner. The scout keeps its own stop
rules, so a WAFER runtime that exits non-zero, or an eKuiper run whose
`ekuiper-health.json` gives `runtime-exit` or `rule-error`, fails the scout attempt
as infrastructure instead of being admitted as a system outcome.

Besides MQTT loopback, native, WAFER and eKuiper, the scout runs a diagnostic arm,
`wafer-max-inflight-1`. It is the WAFER scout pipeline with `max_inflight = 1` on
its MQTT sink (`eval/configs/capacity-scout-wafer-max-inflight-1.toml`), so WAFER
waits for each QoS 1 PUBACK before the next publish, as the eKuiper 2.1 MQTT sink
does. WAFER otherwise keeps up to 100 publishes in flight. The arm tests one
hypothesis: that this flow-control difference, rather than the engine, explains a
capacity gap between WAFER and eKuiper. It goes through the same search, rate
blocks and MQTT support-path censoring as the other systems. `scout-complete.json`
reports its state under `diagnostic_states`, apart from the `states` that informed
the grid. The arm is never thesis evidence and has no verdict: it is not an E-Perf-10
or capacity-knee system, the runner and the result verifier reject a
`capacity-run.json` that names it, and the analysis never reads it.

### Per-host bracket rates

With the common grid alone, two ceilings between 4,000 and 8,000 msg/s can only give
ratio bounds of 0.5, 1 or 2, so the competitive decision is often `CENSORED`. Each host's final batch therefore adds a few E-Perf-10 rates taken
from that host's own capacity scout. `final_campaign.capacity_grid.bracket_rates` in
`eval/canonical-matrix.json` declares the rule once:

- For WAFER and for eKuiper, when the scout resolved its ceiling, the rates at 0.95x and
  1.05x the scout ceiling, which is the highest delivery-good scout rate
  (`lower_good_rate_msg_s`). 0.95x rounds down and 1.05x up to the 100 msg/s step. A
  support-censored or left-censored state adds no rate.
- Two decision points, from the threshold in the `e-perf-10-competitive-ratio` row of
  `verdict_rules`: the threshold times eKuiper's 1.05x rate, rounded up, and the first
  step strictly above WAFER's 1.05x rate divided by the threshold. With eKuiper
  delivery-bad at its 1.05x rate, a WAFER lower bound at the first point gives a
  worst-case ratio at or above the threshold; with WAFER delivery-bad at its 1.05x rate, an
  eKuiper lower bound at the second point gives a best-case ratio below it.
- Rates on the common grid, repeated rates and rates above the highest delivery-good
  MQTT-loopback rate of the scout are dropped, since a rate the support path cannot carry
  is support-confounded for every system. A decision point is added only when the 1.05x
  rate it starts from is within that limit. At most six rates remain, taken in the order
  WAFER 0.95x and 1.05x, eKuiper 0.95x and 1.05x, then the two decision points.

Every E-Perf-10 system, MQTT loopback included, runs each bracket rate in the same seeded
rate blocks and with the same 30 runs per rate as the common grid, so the support path
censors bracket rates the same way. The competitive rule and its threshold do not change.

`canonical_runner.py --execute --batch-id ID --scout-batch-id SCOUT` reads
`manifests/capacity-scout/<host-tag>-SCOUT/scout-complete.json` from the host's results
root and writes the rates into `batch.json` before the first run, as
`capacity_brackets`: `scout_batch_id`, `scout_summary` (the volume-relative path),
`scout_summary_sha256`, `rates_msg_s`, and what the rates add to this batch's schedule,
`added_measured_leaves` and `added_nominal_hours` (warmup plus measurement time). It also
copies the summary, byte for byte, to `scout-complete.json` in the batch ledger. A resume
derives the rates again from that copy and refuses the batch when the scout batch, the
copy's SHA-256 or the rates differ, including when another `--scout-batch-id` is given.
Running the scout command again later, which rewrites the scout's own summary, does not
affect a batch that has started. A final batch refuses to start without a usable scout
summary for its host: a missing or malformed file, a scout that has not stopped, a
missing WAFER or eKuiper state, or no delivery-good MQTT-loopback rate. The refusal names
the scout command to run when the summary is missing or the scout has not stopped. A
diagnostic `--repetitions` batch may run without one; it then runs the common grid only
and records `capacity_brackets` as `null`. A batch without E-Perf-10 has no
`capacity_brackets`. `--dry-run` with `--scout-batch-id` prints the rates and what they
add. A dry run of a final batch without it prints the common schedule and a `NOTE` that
`--execute` refuses to start that batch.

The rates need no commit to the repository. `approve-batch` derives them again from the
ledger copy of the scout summary and refuses the batch when they no longer match
`batch.json`, and checks `schedule.json` against the schedule with those rates. The copy
is part of the ledger, so `raw.sha256` lists it, and `eval/final-batches.json` records the
hash of `raw.sha256`. Canonical analysis expects
the common grid plus the rates in `batch.json` and rejects an E-Perf-10 batch whose
`batch.json` records none; the attempts table counts the bracket rates as scheduled units.
The result verifier checks each capacity leaf against its own rate.

Each bracket rate adds 120 leaves (four systems, 30 runs) and 3.00 nominal hours (90 s
per run) to a host's final batch; six rates, the maximum, add 720 leaves and 18.00
nominal hours. `expected_schedule_records` and `expected_measured_leaves` in the matrix
describe the common schedule, and `batch.json` records what the host's bracket rates add.

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

The phases start at `process_started`, the first statement of the async boot
sequence, after the tokio runtime is built. `process_entry` records the earlier
point: `unix_epoch_ns` is taken as the first statement of `main`, before the
runtime exists, for alignment with the harness's pre-exec `runtime_started_ns`;
`to_process_started_ns` is the monotonic gap from entry to `process_started`.
Neither value is part of `phases_ns` or `total_wall_duration_ns`, and exec,
dynamic loading and static constructors before `main` remain outside both.

`pipeline_setup` ends at the internal `task_activation_started` boundary,
captured immediately before any DLQ or node task can be spawned, after
subtracting component loading/compilation and instantiation. `first_process`
begins at that same boundary and ends at the first successful sink collection.
Pipeline tasks may process the first message before the launch future returns
to its caller; caller-return time is therefore not a phase boundary.

The runtime performs no provenance work between task activation and the first
sink collection: during the probe `runtime-provenance.json` is written only at
shutdown, and the runtime-binary hash is computed on the blocking pool after
the run.

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
  "isolated_cpus": "",
  "housekeeping_cpus": "0",
  "irq_default_cpus": "0",
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
    "profile_path": "eval/loadgen/telemetry-120b.toml",
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

### Runtime exit status

`exit_codes.wafer_runtime` is the only in-band failure signal the harness
has, so the runtime never exits `0` for a run that failed. Codes `1` and `2`
mean the runtime refused to start the pipeline, and `130` and `143` mean a
second interrupt reached it from outside the run; the canonical runner records
these as infrastructure failures. Any other non-zero code, including `3`, a
crash, or a kill after the shutdown grace period, is a `runtime-exit` outcome
of the system under test: the attempt is admitted, not retried, and fails the
run's criteria.

| Code | Meaning | Artefacts |
| --- | --- | --- |
| `0` | Every node task exited cleanly (natural completion or SIGTERM/SIGINT drain). | All runtime-owned artefacts. |
| `1` | Startup failed for a reason other than configuration: engine creation, plugin compile, control-plane bind, or a `startup.json` write after an otherwise clean run. | Whatever was written before the failure; usually none. |
| `2` | Invalid configuration: TOML load, semantic validation, or a source/sink `validate()` run at launch (for example a bench-source `rate = 0` or an inconsistent burst block, a mistyped HTTP `bind`), a local plugin path that cannot be read, or bad `--swap-*` arguments. Nothing is spawned, except that a `--swap-node` that is not a swappable Wasm node is only found after launch and the pipeline is drained first. Also clap's code for bad arguments. | None. |
| `3` | The pipeline started but failed while running: a node task panicked (the log names the node), a source/sink `init()` failed (the pipeline is cancelled at once), a source hit its poll-error budget, a Wasm node could not be re-instantiated after a trap or was torn down by its error policy (the pipeline is cancelled at once), a sink's final flush or `close()` failed, a node did not stop within the 5 s shutdown deadline and was aborted, the DLQ sink failed or did not stop in time, or a `--swap-after-secs` swap could not be prepared or dispatched. | Bench artefacts and `runtime-provenance.json` are still flushed before exiting. The leaf is an admitted `runtime-exit` outcome; its partial artefacts are kept for post-mortem analysis and are not used as measurements. |
| `130`, `143` | A second SIGINT (`130`) or SIGTERM (`143`) arrived before the graceful shutdown finished, and the runtime exited at once. | Whatever was written before the second signal; the leaf is not a valid sample. |

A run killed by the harness after its SIGTERM grace period reports the
signal's code (e.g. `137`), not one of the above.

`wafer-loadgen publish --profile hotswap-trigger` follows the same rule: if
the hot-swap POST fails (transport error or non-2xx), it still writes
`--hotswap-result-path` with an `error` field (and `http_status` when a
response arrived) and the summary file, with `hotswap_triggered_at_secs: null`, then exits non-zero. A `--hotswap-swap-at-secs` at or beyond `--duration-secs` is rejected before publishing.

For final WAFER leaves, the verifier compares these effective metering fields with the condition in `canonical-matrix.json`. E-Perf-7 uses its four-way `metering_modes` table; declared attack stimuli use their condition-specific exceptions; all other WAFER conditions use `final_campaign.canonical_metering`. A missing or mismatched value is a contract violation.

Canonical runs require `git_dirty: false`, a 40-character `git_sha`, and the host profile fields below. `git_tags` records any tags on that commit; a tag is not required. Smoke runs may be dirty but cannot be promoted to thesis evidence. Validate canonical leaves with:

```sh
python3 eval/scripts/verify-result-contract.py --canonical <result-dir>
```

Every leaf checked in one invocation must come from a clean tree
(`git_dirty: false`) at one `git_sha`, and leaves of the same experiment and
condition must carry the same `wafer_plugin_hashes` (keyed by node id, the
same node ids on every leaf); the verifier names the first leaf that differs
and the leaf it differs from. The matrix is part of the source tree, so an
equal clean `git_sha` also means an equal `canonical-matrix.json`. Before the
same experiment from two hosts is compared, pass the other host's batch with
`--match` (repeatable): its leaves join this comparison without the rest of
their contract being checked.

```sh
python3 eval/scripts/verify-result-contract.py --canonical <pi-batch> --match <jetson-batch> --match <x86-batch>
```

### eKuiper health

The harness starts and stops eKuiper as the `kuiper` systemd unit, so it has no
process exit code to read. Every eKuiper run seeds the stream and `pipeline_a`
afresh. eKuiper keeps rules across restarts, including the definition an E-Swap-3
rule update leaves in `pipeline_a`, so the seed script deletes `pipeline_a` and
`pipeline_a_v2` first and fails when it cannot create `pipeline_a` again. Before
warm-up, `ekuiper-audit.json` must record the seeded `pipeline_a`; the run then waits
up to 10 seconds for `pipeline_a` to report `running`, checks that `GET /rules` lists
`pipeline_a` and nothing else, and takes a snapshot of the unit and the rule. It takes
a second snapshot after the run, before it stops the unit. Both go into `ekuiper-health.json` (rule status shortened):

```json
{
  "schema_version": 1,
  "unit": "kuiper.service",
  "rule": "pipeline_a",
  "before": {
    "captured_at_ns": 1784644215000000000,
    "service": {"NRestarts": 0, "ExecMainStatus": 0, "MainPID": 4242},
    "rule_status": {"status": "running", "message": "", "lastStartTimestamp": 1784644214512},
    "rule_status_error": null
  },
  "after": {
    "captured_at_ns": 1784644377000000000,
    "service": {"NRestarts": 0, "ExecMainStatus": 0, "MainPID": 4242},
    "rule_status": {"status": "running", "message": "", "lastStartTimestamp": 1784644214512},
    "rule_status_error": null
  }
}
```

`service` holds `systemctl show kuiper.service -p NRestarts,ExecMainStatus,MainPID`.
`rule_status` is the body of `GET http://127.0.0.1:9081/rules/pipeline_a/status`,
unchanged, or `null` with the error in `rule_status_error` when the request failed.
The E-Swap-3 `ekuiper-make-before-break` arm ends the run with `pipeline_a_v2` in
place of `pipeline_a`, so its file adds `"replacement_rule": "pipeline_a_v2"` and
`after` holds that rule's status; the checks below then apply to the replacement.
A rule that is not `running` with an empty `message` before warm-up fails the
attempt as infrastructure. After the run:

- A different `NRestarts` or `MainPID` means the unit lost or replaced its main
  process: a `runtime-exit` outcome.
- Otherwise a rule status that cannot be read, is not `running`, or carries a
  `message` is a `rule-error` outcome. eKuiper keeps the status `running` while it
  retries a failed rule and leaves `retrying after error: ...` in `message`, even
  after a retry succeeded.
- The per-operator `exceptions_total` counters are not judged, because they also
  count messages dropped from a full buffer under overload.

`exit_codes.ekuiper` in `metadata.json` follows from the snapshots: `0` when the
main process survived the run, the unit's `ExecMainStatus` when it has no main
process at the end, and `null` when systemd already started a new one, which
resets `ExecMainStatus`.

The verifier checks `ekuiper-health.json` on every canonical eKuiper leaf,
including one that stopped early. It rejects a leaf whose snapshots are missing or
malformed, do not bracket `measurement-window.json`, show a rule that was not
running cleanly before warm-up, disagree with `exit_codes.ekuiper`, or name another
main PID than `ekuiper-audit.json`. Only the `ekuiper-make-before-break` arm may name
a `replacement_rule`, and it must name `pipeline_a_v2`; that rule must have started
no earlier than the action start in `disruption-timeline.json`, allowing for the
whole milliseconds of `lastStartTimestamp`. In the `ekuiper-rule-update` arm the PUT
stops `pipeline_a` without starting it, eKuiper answers the start that follows only
after it started `pipeline_a` again, and the start sets `lastStartTimestamp` to the
wall clock in whole milliseconds, so the rule's start time must lie between the action
start less 1 ms and the start call's end timestamp in `rule-update.json`. In every
other experiment a moved `lastStartTimestamp` without a unit restart means something
outside the run started the rule, and the leaf is rejected.

### Host profile fields

The `hosts` map of `eval/canonical-matrix.json` (schema version 2) holds one
profile per host tag: `rpi5` is the canonical host, `jetson` and `x86` are
replication hosts with their own batches, never pooled with the Pi's. The
canonical runner (`--host`), `validate-canonical.py` (`--host`) and the
verifier (by the leaf's `<host>-` batch directory) check a leaf only against its own
profile:

| Field | Source | `rpi5` | `jetson` | `x86` |
| --- | --- | --- | --- | --- |
| `host_tag` | runner `--host` | `rpi5` | `jetson` | `x86` |
| `arch` | `uname -m` | `aarch64` | `aarch64` | `x86_64` |
| `hardware_model` | `/proc/device-tree/model` or DMI | contains `Raspberry Pi 5` | contains `Jetson Orin Nano` | any |
| `cpu_governors` | `/sys/devices/system/cpu/cpu*/cpufreq/scaling_governor` | `performance` | `performance` | `performance` |
| `isolated_cpus` | `/sys/devices/system/cpu/isolated` | empty | empty | empty |
| `housekeeping_cpus` | `Cpus_allowed_list` of PID 1 (systemd `CPUAffinity=`) | `0` | `0` | `0` |
| `irq_default_cpus` | `/proc/irq/default_smp_affinity` as a CPU list (`irqaffinity=`) | `0` | `0` | `0` |
| `throttled` | `pi_telemetry.py` backend | `0x0` | `0x0` | `0x0` |
| `online_cpus` | `/sys/devices/system/cpu/online` | any | `0-3` | any |
| `power_mode` | `nvpmodel -q` | any | `25W` | any |
| `smt` | `/sys/devices/system/cpu/smt/control` | any | any | `off`, `forceoff` or `notsupported` |
| `turbo` | `intel_pstate/no_turbo` or `cpufreq/boost` | any | any | `off` |

The SUT runs on CPUs `1-3` (`sut_cpus`) and the load generator, broker and
samplers on CPU `0` on every host. The runner starts WAFER and the native
baseline under `taskset -c 1-3`, and the eKuiper unit sets `CPUAffinity=1 2 3`.
The runner pins the load generator and the host samplers to `support_cpus`.
Everything else, Mosquitto included, inherits `housekeeping_cpus` from
systemd, and the kernel sends new interrupts there too. No CPU is isolated
with `isolcpus`: its default domain isolation stops load balancing on CPUs
1-3, so every thread of a SUT would stay on the one CPU its process started
on. A `null` value means the fact could not be read, which fails the check.
`memory_total_kib` (`/proc/meminfo`) and
`temperature_millicelsius` (the host's thermal zone at run completion) are
recorded, not checked.

Every `metadata.json` and `host-facts.json` also carries the platform facts
below, on every host tag. A fact the host does not expose is `null`, never
omitted, so a verifier can require the keys everywhere and compare values
only where they exist.

| Field | Source | Notes |
| --- | --- | --- |
| `hardware_model` | `/proc/device-tree/model`, else DMI `sys_vendor` + `product_name` | `unknown` when neither exists |
| `cpu_model` | `/proc/cpuinfo` `model name` (x86) or `Model`/`Hardware`, else the first device-tree `compatible` entry | `null` on hosts whose cpuinfo names no model |
| `physical_cores` | distinct (`physical id`, `core id`) pairs in `/proc/cpuinfo`, else the online CPU count | SMT siblings count once |
| `online_cpus` | `/sys/devices/system/cpu/online` | cpuset syntax |
| `smt` | `/sys/devices/system/cpu/smt/control` | `on`, `off`, `notsupported`, or `null` |
| `turbo` | `intel_pstate/no_turbo` or `cpufreq/boost` | `on`, `off`, or `null` |
| `cpufreq_driver` | `intel_pstate`/`amd_pstate` status, else `cpu0/cpufreq/scaling_driver` | for example `intel_pstate:active` |
| `os_release` | `PRETTY_NAME` in `/etc/os-release` | |
| `glibc_version` | `getconf GNU_LIBC_VERSION` | |
| `power_mode` | `nvpmodel -q` (`NV Power Mode`) | Jetson only, `null` elsewhere |

The preflight rejects a host that does not meet these conditions. Smoke runs may retain the same `rpi5` path prefix, but their metadata and invocation are labelled non-canonical and must not be consumed as thesis evidence.

### Runtime-owned fields

| Field | Producer | Rationale |
| --- | --- | --- |
| `provenance_written_at` | `wafer-runtime` | `launch`, `swap` or `shutdown`: which write the sidecar holds. A completed run holds `shutdown`. |
| `wasmtime_version`, `wasmtime_source` | `wafer-runtime` build.rs, parsed from workspace `Cargo.lock` | The resolved dep version differs from the Cargo.toml declaration when wasmtime is pulled from git; the `source` line carries the git rev, which the version alone does not identify. |
| `ort_sys_version`, `ort_sys_source`, `ort_link` | `wafer-runtime` build.rs: `Cargo.lock` and `ORT_LIB_LOCATION` | Which ONNX Runtime binding was built and whether the library came from `ort-sys` `download-binaries`, a local `ORT_LIB_LOCATION`, or the `system` (download feature off). |
| `runtime_build` | `wafer-runtime` build.rs: `PROFILE`, `OPT_LEVEL`, `TARGET`, `CARGO_ENCODED_RUSTFLAGS`, `CARGO_FEATURE_*`, `git rev-parse HEAD`, `git status` | Explains two different `wafer_runtime_sha256` values and ties the binary to the commit it was built from (a stale binary cannot inherit the harness's run-time `git_sha`). `git_dirty` reflects the tree when the build script last ran. Values are strings; `unknown` when git is unavailable. |
| `rustc_version` | `wafer-runtime` build.rs, `rustc --version` at compile time | Cross-compilation drops the host rustc; build-time capture keeps provenance intact. |
| `wafer_runtime_version` | `env!("CARGO_PKG_VERSION")` | Semver of the binary that ran, not the workspace. |
| `wafer_runtime_sha256` | `std::env::current_exe()` + SHA256, on the blocking pool after the run | Exact binary bytes so a canonical-run number can be tied to the exact build artefact. `null` in `launch` and `swap` writes. |
| `wafer_plugin_hashes` | `PipelineOrchestrator::plugin_hashes_snapshot()` | Populated at initial launch by `PipelineHandle::record_plugin_hash`. On a hot-swap (API or timed `--swap-after-secs`) the node's runner records the replacement's hash when it adopts it and restores the replaced hash if it rolls back inside the canary window, including after the swap outcome was reported. The P0.12 hot-swap guard reads from the same map, so metadata + guard stay coherent (F2 AC2). The shutdown write therefore names the plugin that was loaded when the run ended. |
| `config_path` | The `--config` argument as given | Which file the run loaded; `config_sha256` identifies its bytes. |
| `config_sha256` | Runtime `Sha256` of the effective config file at load time | Notebook cross-references use this as the provenance root. |
| `kernel` | `/proc/sys/kernel/osrelease` (`uname -r` subprocess only where procfs is absent) | Kept in both the runtime provenance and the shell metadata; the runtime version wins on merge. |
| `tokio_worker_threads`, `available_parallelism`, `cpus_allowed_list` | tokio runtime metrics, `std::thread::available_parallelism`, `/proc/self/status` | Effective scheduler width and CPU affinity (`WAFER_RUNTIME_CPUSET` / `taskset`) the run actually had. |
| `engine_fuel_budgets` | Parsed effective engine config | Per-category fuel budgets; absent limits are JSON `null`. Node overrides remain represented by the config digest. |
| `epoch_deadline`, `epoch_tick_ms` | Parsed effective engine config | Effective epoch deadline and tick; an omitted deadline is JSON `null`. |
| `effective_metering_mode` | Derived from parsed fuel and epoch options | Stable value: `neither`, `fuel-only`, `epoch-only`, or `fuel-and-epoch`. |

### Sidecar file

- Path: `runtime-provenance.json` inside the result directory.
- Written by `wafer-runtime` when either `WAFER_METADATA_OUTPUT` (explicit
  path) or `WAFER_BENCH_OUTPUT_DIR` (existing convention) is set.
  `run-experiment.sh` sets the latter.
- Each write replaces the file atomically. The runtime writes it at launch
  (skipped for the E-Perf-9 probe, whose launch write would fall inside
  `first_process`), again after each adopted timed hot-swap, and finally at
  shutdown after the run and result export. A runtime that dies mid-run
  leaves the launch or swap write, with `wafer_runtime_sha256 = null`.
- A write failure is logged and the run continues; the harness then records
  `null` provenance and the canonical verifier rejects the leaf.

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
