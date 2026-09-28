# Result-directory contract

Every WAFER experiment run produces one self-contained raw leaf beneath the
selected results layout. Repository-local tests use
`eval/results/<experiment-id>/<host-tag>-<UTC-timestamp>/`; v10 campaign runs use
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

Prospective v10 runs pass an explicit results root. The approved volume layout is:

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
copy. `BenchSink`, `BenchSource`, and the `wafer-loadgen` subscriber artifacts,
`publisher-summary.json` and publisher timing receipt are written the same way: a
hidden `.<name>.tmp` file beside the destination, synced, then renamed.

Raw attempts are additive. A failed or interrupted attempt remains in place and the
next attempt uses the next numeric suffix. A passed terminal receipt makes the leaf
immutable. Analysis resolves raw inputs through the same results root and may create
outputs only below `derived/` or `reports/`; output traversal or overlap with raw is
rejected.

Shared canonical views do not create another raw tree. A small alias receipt under
`manifests/aliases/` records the volume-relative source leaf, source status digest,
shared sample identity, machine evidence class `final`, and
`independent_n_contribution = 0`. Consumers validate and dereference that receipt to
one passed, thesis-eligible source leaf. Alias mappings must be acyclic; aliases may
not target rejected, diagnostic, or candidate evidence. Copies, symlinks, and
hardlinks are not alias mechanisms. `final` is the machine value; prose names its
admitted analytical class `canonical-primary` or `final admitted`. Historical
repository-local result trees remain unchanged.

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
| `latency.hdr` | Every experiment with a BenchSink or `wafer-loadgen subscribe` (E-Val-1, E-Perf-1..10, E-Backpressure, E-Iso-*, E-Swap-*) | `BenchSink` (in-process) or `wafer-loadgen subscribe` (E2E) | HdrHistogram latency in nanoseconds. The two producers use different encodings of the same V2 histogram: `BenchSink` writes an HdrHistogram interval log (text, one base64 entry tagged `latency_ns`), the subscriber writes the bare binary V2 histogram (starts with the bytes `1c 84 93`). `wafer-loadgen hdr-summary` reads both; pick the reader by content, not by experiment. The value is measured from each message's scheduled send time: arrival minus scheduled time. Both generators pace an open-loop schedule fixed at the start of the run, so time a message waited before it could be sent (a blocked source, a lagging publisher) counts as latency (no coordinated omission). In-process: source-to-sink. E2E: MQTT publish → MQTT consume. Every latency histogram (this one, `service.hdr`, `source-lag.hdr` and the interval rows) uses the same range, 1 µs to 1 h at 3 significant digits. No sample is dropped: a value below zero or above 1 h is recorded at that bound and counted (`latency_clamps` in `measurement-window.json`, `negative_latency_count` and `above_highest_latency_count` in `subscriber-metadata.json`), and a non-zero count rejects the run. In-process timestamps come from one monotonic clock anchored to the wall clock once per process, so a wall-clock step cannot change an in-process latency. |
| `service.hdr`, `source-lag.hdr` | In-process runs whose source is `BenchSource` | `BenchSink` | Two parts of the `latency.hdr` value, same interval-log encoding and bounds. `service.hdr` (tag `service_ns`): arrival minus the time the message left the source. `source-lag.hdr` (tag `source_lag_ns`): time the message left the source minus its scheduled time. Lag grows when the pipeline pushes back on the source; a run whose lag is close to its latency is limited by the source, not by the stage under test. `service.hdr` is absent when no message carried `bench.emit_ns`; `source-lag.hdr` is absent when none carried both `bench.emit_ns` and `bench.intended_ns`. |
| `interval-latency.json` | Timed runs with latency output | `BenchSink` or `wafer-loadgen subscribe` | Bounded producer fragment containing one-second latency histograms reduced to p50/p95/p99 and event/unique/duplicate counts. This is an implementation input to the composer, not a replacement for `latency.hdr`. Both producers write the same keys: `schema_version`, `interval_clock` (`monotonic-elapsed`), `alignment_clock` (`unix-epoch`), `alignment_clock_purpose`, `measurement_start_unix_epoch_ns`, `declared_measurement_duration_ns`, `bucket_width_ns`, `maximum_rows`, `row_count`, `late_arrivals`, `aggregate_latency_count` (the `latency.hdr` population) and `rows`. Each row has `interval_start_ns`, `interval_end_ns`, `interval_start_unix_epoch_ns`, `interval_end_unix_epoch_ns`, `latency_count`, `latency_p50_ns`, `latency_p95_ns`, `latency_p99_ns` (null for an empty bucket), `received_events`, `throughput_messages` and `duplicates`. |
| `interval-metrics.json` | Timed runs with latency or throughput output | `canonical_runner.py` or `interval_metrics.py` | Bounded one-second rows combining latency, throughput, CPU, RSS, PMIC internal-rail proxy, temperature, and queue observations. Missing observations use an explicit `unavailable` status and reason. |
| `throughput.csv` | Same as `latency.hdr` | `BenchSink` (in-process) or `run-experiment.sh` (E2E) | The two producers write different files under this name. `BenchSink`: `elapsed_secs,msg_count,bytes`, one row per fixed 1 s bucket counted from the first post-warmup arrival up to the last arrival. `elapsed_secs` is the bucket's end in whole seconds, a second with no arrivals between two that have some is a zero row, and the last row is the second in which the last message arrived. `msg_count` counts every post-warmup arrival, duplicates included. E2E: `run-experiment.sh` derives one summary row from `subscriber-metadata.json` with `eval/scripts/lib/write_throughput.py`: `timestamp_ns,messages_received,throughput_msg_s,duration_ns` (end of the subscriber run, `total_recorded`, their ratio, and the subscriber's run duration). |
| `sequence.csv` | Loadgen with sequence tracking, or `BenchSink.track_sequences = true` (E-Perf-1..3, E-Perf-8, E-Perf-10, E-Backpressure, E-Swap-*) | `wafer-loadgen subscribe` / `BenchSink` | Producer-specific sequence accounting. `BenchSink` writes one summary row (`total_expected,total_received,gap_ranges,gap_msgs,duplicates_count`); for `BenchSource` input, its `bench.warmup` marker makes this the exact post-warmup population. `wafer-loadgen subscribe` writes long-form gap/duplicate events (`event_type,seq_start,seq_end,count`), with zero data rows when lossless; its totals live in `subscriber-metadata.json`. |
| `memory.csv` | E-Perf-6, E-Perf-7, E-Perf-8, E-Backpressure (and any run with `WAFER_BENCH_OUTPUT_DIR` set) | `wafer-runtime` via `MemoryRecorder::sample_loop` (1 Hz, cross-platform via `memory-stats` crate; on Linux it reads `/proc/self/statm`). Flush on graceful shutdown. | 1 Hz process-RSS timeline of `wafer-runtime`: `elapsed_ms,rss_bytes`. Runtime-owned since A19 closure (thesis-hardening T4). Legacy harness `ps` polling removed. |
| `memory-clock.json` | Runs with `memory.csv` | `wafer-runtime` | Pairs the memory recorder's monotonic zero with one Unix-epoch anchor for cross-process interval composition: `schema_version`, `elapsed_clock` (`monotonic`), `alignment_clock` (`unix-epoch`), `alignment_clock_purpose` and `start_unix_epoch_ns`. |
| `per_node_metrics.csv` | All experiments (emitted on graceful shutdown when `WAFER_BENCH_OUTPUT_DIR` set) | `wafer-runtime` via `PipelineOrchestrator::export_per_node_metrics` | One row per pipeline node: `node_id,messages_in,messages_out,filtered_out,traps_total,traps_memory_out_of_bounds,traps_unreachable,traps_interrupt,traps_out_of_fuel,traps_memory_limit,traps_other,guest_bad_input,guest_dependency_failed,guest_processing_failed,guest_timed_out,guest_unrecoverable,attempts_failed,retries,dlq_sent,dlq_lost,skipped,retry_exhausted_skips,dropped_on_recovery,dropped_on_teardown,error_state_seconds,recovery_count`. `messages_in` is the node's input-queue dequeue count (0 for sources). `traps_*` count calls the host aborted, by kind; `guest_*` count errors the guest returned, by WIT category; `attempts_failed` counts every failed call, retries included. Every processing node satisfies `messages_in = messages_out + filtered_out + skipped + retry_exhausted_skips + dlq_sent + dlq_lost + dropped_on_recovery + dropped_on_teardown` (pending retries are flushed to the DLQ at shutdown). Every config under `eval/configs/` has a file DLQ, so a message the error policy removes, or whose call trapped, is counted as `dlq_sent` and has a record in `dlq.jsonl`; `dlq_lost` counts records refused by a full or closed DLQ, and `dropped_on_recovery` stays zero unless a pipeline runs without a DLQ. Runtime-owned since A19 closure (thesis-hardening T4). |
| `dlq.jsonl` | All experiments (every config under `eval/configs/` sets `[dead_letter] kind = "file"`, `path = "dlq.jsonl"`, which the runtime resolves under `WAFER_BENCH_OUTPUT_DIR`) | `wafer-runtime` DLQ sink | One JSON object per dead-lettered message, written as it arrives: `timestamp` (ms since the Unix epoch), `source_node`, `error_category` (`bad_input`, `dependency_failed`, `processing_failed`, `timed_out`, `unrecoverable`, or null for a trap or queue overflow), `error_message`, `retry_count`, `reason` (`type` one of `bad_input`, `timed_out`, `retries_exhausted`, `retry_buffer_full`, `hot_swap_drain`, `shutdown`, `queue_full`, `trapped` with `kind`, `unrecoverable`, `recovery_failed`), `original` (id, timestamp, source, metadata, base64 payload, retry_count), `trace_id`, `parent_id`. The file exists, possibly empty, for every graceful run; the sink drains until every node has exited, so the record count equals the sum of `dlq_sent` over `per_node_metrics.csv` plus the edges' `dead_lettered` counts. |
| `queue-depth.csv` | E-Backpressure, when `WAFER_QUEUE_DEPTH_OUTPUT` is set | `wafer-runtime` via `QueueDepthRecorder` | Bounded internal Tokio queue samples at 10 ms intervals: `elapsed_ns,queue,depth,capacity,accepted,dequeued,processed,dropped,dead_lettered,downstream_closed,dlq_full,dlq_closed`. The five policy counters expose existing R1 `QueueSnapshot` state without changing dispatch behavior. Counters are internal pipeline observations; broker backlog is excluded. Collection is capped at 131,072 rows and reports truncation in the runtime log. |
| `queue-depth-clock.json` | Runs with `queue-depth.csv` | `wafer-runtime` | Pairs the queue recorder's monotonic zero with one Unix-epoch anchor for cross-process interval composition. Same keys as `memory-clock.json`. |
| `backpressure.json` | E-Backpressure | `canonical_runner.py` | Schema v2 records condition/policy, run index, measured queue, threshold crossing and recovery, offered/accepted/processed/drained rates, exact R1 queue counters, sequence counts, policy equation, producer-progress mode, explicit DLQ-full/closed failures, and peak-RSS bound. Attempted messages come from the frozen BenchSource population; `sequence.csv` validates its observed span because a trailing overflow disposition is not visible at the sink. `slow` requires attempted = delivered with no overflow disposition; `drop` requires attempted = delivered + dropped; `dead-letter` requires attempted = delivered + dead_lettered + dlq_full + dlq_closed. `dead_lettered` is the runtime's successful enqueue to the configured DLQ delivery path; full/closed failures are never counted as success. A high offered rate without measured occupancy is `not-saturated`. |
| `recovery.csv` | E-Iso-8 and any run with recovery events | `wafer-runtime` via `PipelineOrchestrator::export_per_node_metrics` | Exact recovery samples: `node_id,sample_index,duration_ns`. Retains nanosecond precision for trap-to-running percentiles. |
| `measurement-window.json` | Canonical Pi 5 runs | `BenchSink` for in-process runs with post-warmup output; canonical harness for external MQTT subscribers and zero-output containment runs | Exact `started_ns` and `finished_ns` bounds (wall clock) for excluding warmup and teardown from PMIC energy integration. The `BenchSink` copy adds `latency_clamps`: for each of `latency`, `service` and `source_lag`, the `negative` and `above_highest` sample counts recorded at a histogram bound. The verifier rejects a canonical leaf where any of them is non-zero. It also adds `wall_clock_step_ns`: how far the wall clock moved away from the in-process monotonic clock between the process start and the export, signed. In-process latencies do not depend on it; it tells whether `started_ns` and `finished_ns` can be aligned with other processes' telemetry. |
| `pi-telemetry.csv` | Canonical Pi 5 runs | `eval/scripts/lib/pi_telemetry.py` | Timestamped temperature, CPU frequency, governor, throttling state, and summed PMIC internal-rail proxy watts. |
| `pmic-rails.csv` | Canonical Pi 5 runs | `eval/scripts/lib/pi_telemetry.py` | Long-form named PMIC rail voltage/current/power samples. Rails without both voltage and current are not included. |
| `power-boundary.json` | Canonical Pi 5 runs | `eval/scripts/lib/pi_telemetry.py` | Declares that telemetry is an internal-rail proxy, not total USB-C input power, and records excluded consumers and the source limitation. |
| `published.csv`, `received.csv` | Historical focused E-Perf-10 diagnostics only | `wafer-loadgen` opt-in tracing | Raw publisher/subscriber timestamp and sequence samples retained for v11-v17 compatibility. Final E-Perf-10 rejects these mandatory traces and uses bounded summaries. |
| `publisher-summary.json` | Final E-Perf-10 and Final E-Swap-3 | `wafer-loadgen publish` | `schema_version`, bounded `intended`, `rejected`, `enqueued`, `acked`, and `unacked_at_exit` counters plus the scheduled `measurement_duration_ns`. `intended = rejected + enqueued` and `enqueued = acked + unacked_at_exit` are mandatory: `enqueued` is what was handed to the MQTT client, `acked` is what the broker acknowledged (QoS 1 PUBACK), and `unacked_at_exit` is what was still unacknowledged when the 5 s drain window after a completed schedule closed (a run stopped by a signal skips the drain). `connects` counts successful CONNACKs and `connection_errors` counts event-loop errors; the clock starts only after the first CONNACK (10 s timeout, then the run fails without a summary). The verifier rejects `connects != 1` or `unacked_at_exit != 0`, and loss downstream of the publisher is `acked - received_unique`. `published` and `errors` repeat `intended` and `rejected` under their older names. `elapsed_ms` and `actual_rate` are the wall time the schedule took and `intended` divided by it; `deadline_misses` counts messages whose hand-off to the client ended after the next message was due; `hotswap_triggered_at_secs` is the swap offset for the `hotswap-trigger` profile and null otherwise. `source_lag_ns` (`count`, `p50`, `p99`, `p999`, `max`) is how late each message was handed to the MQTT client relative to its scheduled time; payload `ts` is the scheduled time, so this lag is part of the measured latency. `exit_reason` is `duration` when the schedule ran to its end, `sigterm`/`sigint` when a signal stopped it, or `broker-lost` when the MQTT session ended for good mid-run; the counters then cover the messages offered before the stop, and the verifier rejects the run. A second signal exits at once without a summary. |
| `subscriber-metadata.json` | Final E-Perf-10 and external-MQTT experiments | `wafer-loadgen subscribe` | Bounded receive, duplicate, ignored-warmup, unexpected-sequence, parse, latency, and sequence totals: `broker`, `topic`, `started_at_ns`, `ended_at_ns`, `git_sha`, `host_tag`, `sequence_end_exclusive`, `total_messages` (every message read), `total_recorded` (the `latency.hdr` population), `ignored_sequence_count` (warmup), `unexpected_sequence_count`, `parse_errors`, `latency_min_ns`, `latency_max_ns`, `latency_mean_ns`, `latency_p50_ns`, `latency_p95_ns`, `latency_p99_ns`, `latency_p999_ns`, the histogram range `histogram_lowest_ns`, `histogram_highest_ns`, `histogram_sig_digits`, and `sequence` (received, gap and duplicate totals plus bounded examples). The HDR count and received-event population must match. `negative_latency_count` and `above_highest_latency_count` count samples recorded at a histogram bound; `clock_steps` counts wall-clock steps larger than 100 ms seen between received messages (the gap between the wall clock and the monotonic clock changed). Latency subtracts the publisher's wall-clock schedule from the subscriber's wall-clock receive time, so a step shifts every later sample; the verifier rejects the run when any of the three is non-zero. `exit_reason` is `total-messages`, `sigterm`, `sigint`, or `eof`. `status` is `complete`, or `partial` when `interval-latency.json` or `throughput-buckets.json` overflowed its bound or failed its checks; `partial_reasons` names each failure. A partial run keeps `latency.hdr`, `sequence.csv` and this file, leaves out the failed artifact, exits non-zero, and is rejected by the verifier. Written last, so its presence means the other subscriber artifacts are complete. A second SIGTERM or SIGINT exits at once without flushing. |
| `export-errors.json` | Any run with a `BenchSink` output directory, only when an artifact failed | `BenchSink` | `{"schema_version":1,"errors":[{"artifact":…,"error":…}]}`. `BenchSink` writes `sequence.csv`, `measurement-window.json`, `swap_timeline.json`, `latency.hdr`, `service.hdr`, `source-lag.hdr` and `throughput.csv` first and the derived artifacts after, and one failed artifact does not stop the rest. Its presence rejects the leaf. |
| `capacity-run.json` | Final E-Perf-10 and candidate E-Perf-Capacity-Knee | `canonical_runner.py` | Trace-free run summary containing intended/rejected/enqueued/acked/received/lost/duplicate counts, offered and achieved rates, loss, p50/p95/p99, CPU, RSS, thermal state, process/config receipts, and controlled factors. Final E-Perf-10 sets `thesis_evidence=true`; capacity-knee sets `evidence_class=candidate-supplementary`, `thesis_evidence=false`, and `n30_admitted=false`. |
| `resource-usage.csv` | E-Perf-10 and E-Perf-Capacity-Knee | `canonical_runner.py` | One-second SUT samples: wall-clock timestamp, aggregate process CPU ticks, RSS bytes, and process count. MQTT loopback records the explicit no-SUT zero baseline. |
| `process-audit.json` | E-Perf-10 and E-Perf-Capacity-Knee | `canonical_runner.py` | Active SUT PID, process affinity, exclusivity, and allowed CPU set captured before measurement. |
| `rate-sweep.json` | Historical focused E-Perf-10 diagnostics only | `canonical_runner.py` | Legacy trace-backed run summary, always `thesis_evidence=false`. It remains readable but is not accepted as a final-capacity leaf. |
| `throughput-buckets.json` | Final E-Swap-3 and E-Swap-4 | `wafer-loadgen subscribe` / `BenchSink` / `canonical_runner.py` | Contiguous 100 ms output-rate buckets with unique/event/duplicate counts. E-Swap-3 bins exactly -10 s through +10 s around the actual disruption event. E-Swap-4 keeps 1,200 source-origin primary buckets over `[0,120s)` and a separate 100-bucket drain series over `[120s,130s)`, with O(1) after-drain evidence. Unix-epoch values are labeled for source/sink or cross-process alignment only. |
| `throughput-buckets-10ms.json` | Final E-Swap-3 and E-Swap-4 | `wafer-loadgen subscribe` / `BenchSink` | Exactly 400 contiguous 10 ms buckets over `[-2s,+2s)` around actual `t0`, plus 40 nested actual-t0-aligned 100 ms parent buckets. Both levels carry unique/event/duplicate counts and reconcile exactly. E-Swap-3 parent buckets also match the canonical event-window slice; E-Swap-4 retains its separate source-origin primary/drain series as the loss-accounting authority. No message identity or timestamp trace is retained. |
| `disruption-timeline.json` | Final E-Swap-3 | `canonical_runner.py` | Strategy, actual Unix-epoch action start/end for cross-process alignment, monotonic scheduling/duration labels, the scheduled t=60 target, measured offset, signed alignment error, and a 10 ms maximum alignment tolerance. This is the single retained runner-owned final action timeline for E-Swap-3. |
| `disruption-analysis.json` | Final E-Swap-3 | `canonical_runner.py` | Derived disruption summary containing baseline and event-window rates, dip percentage, interruption and recovery timings, action duration, loss, duplicates, and latency percentiles. |
| `burst-source-timing.json`, `burst-source-summary.json` | Final E-Swap-4 | `BenchSource` | Source-origin timestamp, monotonic source-completion offset, fixed phase boundaries, and exact intended/emitted populations for the 1,000 to 2,000 to 1,000 msg/s burst. |
| `swap-actual-t0.json` | Final E-Swap-4 | `canonical_runner.py` | Runner-owned actual-`t0` receipt declaring the source origin, scheduled swap timestamp, actual request timestamp, signed alignment error, and fixed 10 ms tolerance for the single measured swap. |
| `burst-timeline.json` | Final E-Swap-4 | `BenchSource` / `canonical_runner.py` | One 1,000 to 2,000 to 1,000 msg/s burst with boundaries at measured seconds 55 and 65 and exactly one successful swap scheduled at second 60, plus actual alignment, phase populations, primary/drain completion evidence, sequence integrity, sink gap, and internal swap phases. |
| `startup-preparation.json` | E-Perf-9 | `run-experiment.sh` | Filesystem-cache condition and preparation action completed before the timed runtime process starts. |
| `startup.json` | E-Perf-9 | `wafer-runtime` | Monotonic process/config, component load/compile, instantiation, pipeline setup, and first-process durations; total startup duration; exactly-one-message proof; plugin SHA-256; and explicit compiled-component cache state. |
| `containment.json` | E-Iso-1..8 | `canonical_runner.py` | Containment verdict for one attack condition: expected condition, attack node, expected mechanism (the `per_node_metrics.csv` column that must count the attack, see `eval/scripts/lib/containment.py`), its count, unexpected outcomes on the attack node, trap total, runtime-panic flag, healthy-node output count, per-node runtime metrics, and the dead-letter evidence: `dlq_sent_total` (sum over nodes) and `dlq_records` (lines in `dlq.jsonl`, null when the file is absent). The verifier rejects an E-Iso-1..6 leaf whose `dlq.jsonl` exists but holds a different number of records than `dlq_sent_total`. An attack counts as contained only when the expected mechanism stopped it at least once and nothing else happened on that node: no other trap kind or guest error, and no message passed on. `contained` is null for conditions without an attack. False containment, runtime panic, malformed or pre-split metrics, or condition drift reject the leaf. |
| `branch-a/`, `branch-b/` | E-Iso-7 | `BenchSink` | Independent post-warmup latency histogram, throughput series, sequence accounting, and measurement window for each branch. Each branch has its own `BenchSource`; root-level fan-out/fan-in measurements are forbidden for branch-impact analysis. |
| `branch-isolation.json` | E-Iso-7 | `canonical_runner.py` | Branch-local source identity, configured post-warmup target count, actually offered/received post-warmup counts, target shortfall, throughput samples, latency percentiles, measurement boundaries, and explicit units. |
| `branch-isolation-summary.json` | E-Iso-7 batch ledger | `canonical_runner.py` | Separate branch-A throughput-drop and p95-latency-increase rows for panic and epoch-loop attacks, including run counts and units. |
| `host-load-ladder.json` | E-Host-Thermal-Storage | `characterize-rpi5-host.sh` | Append-only clean-boot session receipt with the exact eight-phase order, per-phase pass/fail/not-run status, 75 °C stop limit, boot identity, bounded sample counts, diagnostic/final admission decisions, source state, and the PMIC internal-rail boundary. |
| `host-telemetry.csv` | E-Host-Thermal-Storage | `characterize-rpi5-host.sh` | One-second phase-labeled temperature, CPU frequency, throttling, PMIC internal-rail proxy, memory availability/pressure, USB throughput, boot ID, wall-clock, and monotonic samples. No per-message data. |
| `kernel-io.log`, `usb-integrity.json` | E-Host-Thermal-Storage | `characterize-rpi5-host.sh` | Bounded matching kernel I/O errors and per-USB-phase byte/duration/SHA-256 reconciliation. Any recorded kernel I/O error or hash mismatch stops the ladder and blocks final admission. |
| `ekuiper-runtime-summary.json` | E-Compare-eKuiper-Profile | `canonical_runner.py` | Diagnostic run identity, interval alignment, latency percentiles, bounded external `/proc` process summary when available, and explicit eKuiper 2.1.0 GC/runtime-unavailability status. It never infers GC events from RSS or latency. |
| `profiler-overhead.json` | E-Compare-eKuiper-Profile | `canonical_runner.py` | Profiler state, matched rate/run pair, collection-enabled flag, and the run-level profiled-minus-control estimator label. It declares association-only interpretation and is not primary evidence. |
| `swap_requests.json` | E-Swap-1, E-Swap-2, E-Swap-4, E-Swap-5, E-Swap-6 | `canonical_runner.py` HTTP client | One record per API request with request boundaries, monotonic `request_duration_ns`, HTTP status, and the runtime's typed internal outcome phases. E-Swap-5 requires 50 process-trap requests whose response status is `rolled_back`. |
| `swap_timeline.json` | E-Swap-1, E-Swap-2, E-Swap-4, E-Swap-6 | `BenchSink` | Sink-observed successful plugin-version transitions. Each `pause_ns` is an output interarrival gap and is not an internal swap duration. E-Swap-5 forbids this artifact because the rejected v2 never becomes sink-observed. |
| `hotswap-analysis.json` | E-Swap-1, E-Swap-2, E-Swap-4, E-Swap-6 | `canonical_runner.py` | Index-matched API and sink observations with explicit `*_ns` names: internal phases, `http_total_ns`, and `sink_observed_output_gap_ns`. Includes the unique measurement source leaf so shared E-Swap-2/6 views do not multiply samples. |
| `rollback.json` | E-Swap-5 and candidate rollback sessions | `canonical_runner.py` | Request-indexed compile, instantiate, signal, and rollback durations plus exact attempt/success counts and lossless sequence evidence. It contains no successful-v2 sink transition. |
| `post-rollback-continuity.json` | E-Swap-5 | `canonical_runner.py` | Explicit output observed after the final rollback, with final request identity, bounded observation interval, message count, interval-metrics provenance, and lossless full-run sequence evidence. |
| `summary.json` | E-Val-1 only | `run-e-val-1-shakedown.sh` | Gate-pass summary across runs (p99 range, honesty-window check). Bespoke to the honesty-gate methodology; not consumed by canonical analysis. |

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
- `wafer-loadgen subscribe` owns `subscriber-metadata.json`,
  `latency.hdr` (E2E path), `sequence.csv`, bounded `interval-latency.json`,
  bounded `throughput-buckets.json`, and historical opt-in `received.csv` traces.
- `wafer-loadgen publish` owns `publisher-summary.json` and historical opt-in
  `published.csv` traces.
- `canonical_runner.py` owns final E-Perf-10 `capacity-run.json`,
  `resource-usage.csv`, `process-audit.json`, and the composed `interval-metrics.json`;
  historical diagnostics retain
  `rate-sweep.json`. It also owns final E-Swap-3 `disruption-timeline.json`
  and `disruption-analysis.json`, final E-Swap-4 `swap-actual-t0.json`
  and `burst-timeline.json`, E-Backpressure `backpressure.json`,
  E-Swap `swap_requests.json` and `hotswap-analysis.json`, final E-Swap-5
  `rollback.json` and `post-rollback-continuity.json`, plus E-Iso-7
  `branch-isolation.json` and the batch-level branch-A impact summary.
- `run-experiment.sh` and the per-experiment shakedown scripts own
  `metadata.json`, `config.toml`, `stdout.log`, the E2E `throughput.csv`, and E-Perf-9
  `startup-preparation.json`.

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

### Production P2 async A/B experiment

This checked-in specification freezes the host-only asynchronous Preview 2
candidate. It does not change `wafer:pipeline@0.1.0`, guest WIT, plugin bytes,
existing evaluation results, or the separately planned P3 PoC.

The workspace and lockfile pin Wasmtime 48.0.2 to
`e9f1ea232fd245aea338ab3eb7d73487ae75cab1`. At that revision the candidate
uses `wasmtime_wasi::p2::add_to_linker_async`; host `bindgen!` invocations use
`imports: { default: async }`, `exports: { default: async }`, and
`require_store_data_send: true`; generated typed pre-instances use
`instantiate_async`; and lifecycle and processing calls are awaited.
`Config::async_support(true)` is excluded because it is a deprecated no-op at
the pinned revision. The existing synchronous `wasmtime_wasi_nn` linker wiring
remains, while inference instantiation and exports follow the same async
Component Model path as Transform.

#### Current and candidate execution paths

| Path | Current implementation | Async candidate | Preserved boundary |
| --- | --- | --- | --- |
| Startup and linker | `WaferEngine::build_linker` registers `p2::add_to_linker_sync`; typed `*Pre` values cover Transform, inference, Filter, and Router. | Register `p2::add_to_linker_async` and generate async imports/exports. | One engine and the existing four worlds and capabilities. |
| Initial instantiation | Launch creates one Store per loaded node, applies limits and metering, calls synchronous `*Pre::instantiate`, then lifecycle. | Await `*Pre::instantiate_async`, `validate`, and `init` in the same sequential launch future. | One Store owner; fuel before start functions; memory, epoch, config, version, and inference grant unchanged. |
| Lifecycle | `Wasm*Node::validate_and_init` resets configured fuel and epoch, calls `validate` then `init`, and flushes logs. Retirement drops the Store rather than invoking guest `close`. | Await the same ordered calls; do not add new `close` behavior. | No lifecycle-policy or cleanup-order change. |
| Processing | Transform `process`, Filter `evaluate`, and Router `route` clear logs, reset metering, push one borrowed buffer, synchronously call the guest, flush logs, delete the buffer on every result, and map output/error/trap. | Make the wrappers async and await the generated call while retaining that exact setup/call/cleanup order. | Logs, fuel, relative epoch, borrowed-buffer cleanup, metadata, lineage, retry count, and forwarding unchanged. |
| Runner scheduling | `spawn_wasm_runner` uses `JoinSet::spawn_blocking` plus `Handle::block_on`; each guest call uses `tokio::task::block_in_place`. | Use ordinary `JoinSet` tasks and await node calls directly. | `ProcessingGuard` spans the call; no per-message spawn, Store mutex, abort, timeout race, or Wasm future in `select!`. |
| Trap/timeout recovery | After a completed error, `recover_from_cached_pre` creates a fresh Store, reapplies capabilities, memory, fuel, epoch, cached pre-instantiation, and lifecycle. | Await fresh-Store instantiation and lifecycle before receiving another message. | A trapped, interrupted, or cancellation-dropped Store is never reused. |
| Reconfigure | A fresh Store and cached instance are prepared synchronously; lifecycle runs after swap-in; rejection restores the prior Store, bindings, and config. | Await preparation and lifecycle before commit or rollback. | Atomic between-message reconfigure. |
| Hot-swap | Preparation already uses `instantiate_async` with a typed pre-instance built from the sync P2 linker; adoption and lifecycle happen between messages. | Use the async P2 linker/bindings throughout preparation and await adoption lifecycle. | Watch signalling, grants, memory limits, canary rollback, and one active call. |
| Direct harness | `PluginTestHarness` synchronously instantiates and `TransformHarness::process` calls the sync production wrapper. | Use the same async production binding and call route without a test-only exported API. | Production bindings and cached pre-instantiation remain the harness path. |

The inventory is reconciled by these exact searches:

```sh
DEVELOPER_DIR=/Library/Developer/CommandLineTools git grep -n -E \
  'add_to_linker_(sync|async)|spawn_blocking|block_in_place|\.instantiate(_async)?\(|call_(validate|init|process|evaluate|route)' \
  -- crates/wafer-core/src

WASMTIME_REPO=/Users/i572543/Dev/pi-repos/repos/github.com/bytecodealliance/wasmtime/main
DEVELOPER_DIR=/Library/Developer/CommandLineTools git -C "$WASMTIME_REPO" \
  grep -n -E 'pub fn add_to_linker_(async|sync)|require_store_data_send' \
  e9f1ea232fd245aea338ab3eb7d73487ae75cab1 -- \
  crates/wasi/src/p2 crates/wasmtime/src/runtime/component examples/wasip2-async/main.rs
```

#### Correctness and strict-simplicity gates

Correctness passes only when the mandatory prepared-artifact matrix passes
without missing fixtures, required skips, or ignored tests. It covers all four
worlds, Rust P2 and TinyGo P2 compatibility, repeated success/error/trap
cleanup, saturated workers, active-call shutdown, fresh-Store recovery,
reconfigure, hot-swap/rollback, inference, capabilities, memory limits,
metering, logs, metadata, lineage, retry count, and exactly-once forwarding.
Formatting, clippy with warnings denied, and workspace tests are mandatory.

Strict simplicity passes only when:

1. production has zero Wasm-runner `spawn_blocking` and zero guest-call
   `block_in_place` sites;
2. startup, lifecycle, processing, recovery, reconfigure, and hot-swap use one
   async linker-to-call path without a sync fallback, feature switch, or
   duplicate wrapper;
3. every node directly owns one Store and has at most one in-flight guest call;
4. no Store mutex, per-message spawn, normal-path abort, cancellation timeout,
   or Wasm future in `select!` is introduced; and
5. no equivalent scheduling bridge replaces the removed blocking bridge.

The decision receipt records:

```text
blocking_bridge_sites = production Wasm-runner spawn_blocking sites
                      + production guest-call block_in_place sites
production_p2_execution_paths = distinct sync or async linker-to-call paths
```

The candidate requires `blocking_bridge_sites = 0` and
`production_p2_execution_paths = 1`, with both values lower than a nonzero
baseline and no replacement bridge in the source diff.

#### Metrics and fail-closed decision

For run `i`, after warmup:

```text
T_arm,i = measured_unique_messages / measurement_duration_seconds
L_arm,i = p95_ns from the complete latency.hdr
R_arm,i = max in-window rss_bytes from memory.csv
```

For each payload condition, throughput and p95 are the medians of the 10 run
values. RSS is the maximum run-level peak. Deltas use the baseline denominator:

```text
throughput_delta_pct = 100 * (T_candidate - T_baseline) / T_baseline
p95_latency_delta_pct = 100 * (L_candidate - L_baseline) / L_baseline
rss_delta_bytes = R_candidate - R_baseline
rss_allowance_bytes = max(0.05 * R_baseline, 2 * 1024 * 1024)
```

| Correctness | Strict simplicity | Every condition: throughput | Every condition: p95 | Every condition: RSS | Decision |
| --- | --- | --- | --- | --- | --- |
| pass | pass | `throughput_delta_pct >= -5.0` | `p95_latency_delta_pct <= 5.0` | `rss_delta_bytes <= rss_allowance_bytes` | `adopt-async-p2` |
| fail | any | any | any | any | `retain-sync-p2` |
| pass | fail | any | any | any | `retain-sync-p2` |
| pass | pass | fails any condition | any | any | `retain-sync-p2` |
| pass | pass | pass | fails any condition | any | `retain-sync-p2` |
| pass | pass | pass | pass | fails any condition | `retain-sync-p2` |
| missing, malformed, dirty, unpaired, or fewer than 10 valid pairs | any | any | any | any | `retain-sync-p2` |

There is no inconclusive adoption state. Any unproven gate selects
`retain-sync-p2`; the candidate code is reverted while tests and investigation
evidence remain.

#### Matched workload and artifacts

Raw outputs live outside the historical and canonical trees under the active
plan scratch directory:

```text
<scratch>/p2-ab/
├── batch-manifest.json
├── baseline/120b/pair-01/ ... pair-10/
├── candidate/120b/pair-01/ ... pair-10/
├── baseline/1kb/pair-01/ ... pair-10/
├── candidate/1kb/pair-01/ ... pair-10/
├── baseline/100kb/pair-01/ ... pair-10/
├── candidate/100kb/pair-01/ ... pair-10/
└── p2-decision.json
```

Each pair completes before the next pair. Odd pairs run baseline then
candidate; even pairs run candidate then baseline, counterbalancing order while
preserving strict alternation. A pair contributes only when both leaves pass
validation. Each leaf contains the normal run-experiment outputs plus
`p2-ab-manifest.json`:

| File | Required contents |
| --- | --- |
| `metadata.json` | Full source SHA, `git_dirty=false`, tool/runtime provenance, config SHA-256, runtime SHA-256, and plugin SHA-256. |
| `config.toml` | Exact checked-in condition config. Its SHA-256 must match its counterpart in the pair. |
| `latency.hdr` | Complete post-warmup histogram; `wafer-loadgen hdr-summary` must report 60,000 values and p50/p95/p99 nanoseconds. |
| `latency-summary.json` | Machine-readable `hdr-summary` output used for p50/p95/p99 and population validation. |
| `throughput.csv` | Post-warmup throughput samples. The run scalar is 60,000 unique messages divided by the `measurement-window.json` duration. |
| `memory.csv` | Runtime-owned one-second RSS samples. Using `memory-clock.json`, the run scalar is `max(rss_bytes)` within the measurement window. Missing or empty in-window samples fail the leaf. |
| `measurement-window.json` | Exact post-warmup Unix-epoch start/end bounds used for the throughput denominator. |
| `sequence.csv` | Exactly 60,000 measured messages, zero gaps, and zero duplicates. |
| `stdout.log` | Complete runtime stdout/stderr. A panic, required-test skip, or early runtime exit fails the leaf. |
| `runtime-provenance.json` | Runtime-owned Wasmtime, Rust, binary, config, plugin, kernel, and effective-metering provenance. |
| `p2-ab-manifest.json` | Schema below, including hashes of every preceding file. |

`batch-manifest.json` records schema `wafer-p2-async-ab-batch-v1`, generation
UTC, host/kernel/architecture, full baseline and candidate SHAs, both clean git
statuses, Wasmtime revision
`e9f1ea232fd245aea338ab3eb7d73487ae75cab1`, Rust/Cargo/wasm-tools/Python
versions, the WIT tree hash, the three condition config hashes, runtime and
pass-through component hashes per arm, `TOKIO_WORKER_THREADS=4`, queue capacity
1,024, release/locked build mode, 10 pairs per condition, and the counterbalanced
order rule.

`p2-ab-manifest.json` uses schema `wafer-p2-async-ab-run-v1` and contains:

```json
{
  "schema": "wafer-p2-async-ab-run-v1",
  "arm": "baseline",
  "condition": "1kb",
  "pair_index": 1,
  "git_sha": "<40 lowercase hex characters>",
  "git_dirty": false,
  "wasmtime_revision": "e9f1ea232fd245aea338ab3eb7d73487ae75cab1",
  "controlled_factors": {
    "payload_bytes": 1024,
    "payload_pattern": "repeated-byte-0x42",
    "queue_capacity": 1024,
    "tokio_worker_threads": 4,
    "rate_messages_per_second": 1000,
    "warmup_messages": 30000,
    "warmup_seconds": 30,
    "measured_messages": 60000,
    "measurement_seconds": 60,
    "outer_duration_seconds": 120,
    "release": true,
    "locked": true,
    "fuel": {"transform": 10000000, "filter": 500000, "router": 500000},
    "epoch_deadline_ticks": 100,
    "epoch_tick_ms": 10
  },
  "sha256": {
    "config.toml": "<64 lowercase hex characters>",
    "wafer-runtime": "<64 lowercase hex characters>",
    "pass-through.wasm": "<64 lowercase hex characters>",
    "metadata.json": "<64 lowercase hex characters>",
    "runtime-provenance.json": "<64 lowercase hex characters>",
    "latency.hdr": "<64 lowercase hex characters>",
    "latency-summary.json": "<64 lowercase hex characters>",
    "throughput.csv": "<64 lowercase hex characters>",
    "memory.csv": "<64 lowercase hex characters>",
    "memory-clock.json": "<64 lowercase hex characters>",
    "measurement-window.json": "<64 lowercase hex characters>",
    "sequence.csv": "<64 lowercase hex characters>",
    "stdout.log": "<64 lowercase hex characters>"
  }
}
```

The following is the exact preparation and execution procedure. `BASELINE_SHA`
and `CANDIDATE_SHA` must be set to the full P1-T1 and P1-T5 commits before
execution; abbreviated revisions are rejected. It creates detached clean
worktrees and never uses the developer's dirty working tree.

```sh
set -euo pipefail
export DEVELOPER_DIR=/Library/Developer/CommandLineTools
export TOKIO_WORKER_THREADS=4
export ROOT=/Users/i572543/.pi/plans/component-model-shippable-improvements/scratch/p2-ab
export REPO=/Users/i572543/Dev/github.com/PedroKlein/wafer-poc/wasi-0.3-improvements
export BASELINE_SHA=<full-40-character-P1-T1-commit>
export CANDIDATE_SHA=<full-40-character-P1-T5-commit>
export WASMTIME_REV=e9f1ea232fd245aea338ab3eb7d73487ae75cab1

printf '%s\n' "$BASELINE_SHA" | grep -Eq '^[0-9a-f]{40}$'
printf '%s\n' "$CANDIDATE_SHA" | grep -Eq '^[0-9a-f]{40}$'

mkdir -p "$ROOT/worktrees" "$ROOT/logs"
{
  git --version
  rustc --version --verbose
  cargo --version --verbose
  wasm-tools --version
  python3 --version
} >"$ROOT/logs/tool-versions.log" 2>&1
git -C "$REPO" diff --exit-code "$BASELINE_SHA" "$CANDIDATE_SHA" -- wit plugins/pass-through
git -C "$REPO" worktree add --detach "$ROOT/worktrees/baseline" "$BASELINE_SHA"
git -C "$REPO" worktree add --detach "$ROOT/worktrees/candidate" "$CANDIDATE_SHA"

for arm in baseline candidate; do
  worktree="$ROOT/worktrees/$arm"
  test -z "$(git -C "$worktree" status --porcelain)"
  DEVELOPER_DIR=/Library/Developer/CommandLineTools \
    cargo build --manifest-path "$worktree/plugins/pass-through/Cargo.toml" \
      --target wasm32-wasip2 --release --locked \
    >"$ROOT/logs/$arm-build-plugin.log" 2>&1
  DEVELOPER_DIR=/Library/Developer/CommandLineTools \
    cargo build --manifest-path "$worktree/Cargo.toml" --release --locked \
      -p wafer-runtime -p wafer-loadgen \
    >"$ROOT/logs/$arm-build-host.log" 2>&1
  wasm-tools validate --features component-model \
    "$worktree/plugins/pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm" \
    >"$ROOT/logs/$arm-validate-plugin.log" 2>&1
  test -z "$(git -C "$worktree" status --porcelain)"
done

git -C "$ROOT/worktrees/baseline" rev-parse HEAD | grep -Fx "$BASELINE_SHA"
git -C "$ROOT/worktrees/candidate" rev-parse HEAD | grep -Fx "$CANDIDATE_SHA"
cmp \
  "$ROOT/worktrees/baseline/plugins/pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm" \
  "$ROOT/worktrees/candidate/plugins/pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm"

for condition in 120b 1kb 100kb; do
  for pair in $(seq -w 1 10); do
    if [ $((10#$pair % 2)) -eq 1 ]; then
      arms='baseline candidate'
    else
      arms='candidate baseline'
    fi
    for arm in $arms; do
      worktree="$ROOT/worktrees/$arm"
      config="$worktree/eval/configs/canonical/e-perf-4-$condition.toml"
      out="$ROOT/$arm/$condition/pair-$pair"
      test -z "$(git -C "$worktree" status --porcelain)"
      (
        cd "$worktree"
        DEVELOPER_DIR=/Library/Developer/CommandLineTools \
        TOKIO_WORKER_THREADS=4 \
        eval/scripts/run-experiment.sh \
          --config "eval/configs/canonical/e-perf-4-$condition.toml" \
          --experiment p2-async-ab \
          --host shakedown-macos \
          --duration 120 \
          --measurement-secs 60 \
          --warmup-secs 30 \
          --skip-build \
          --output-dir "$out"
      ) >"$ROOT/logs/$arm-$condition-pair-$pair.log" 2>&1
      test -s "$out/latency.hdr"
      test -s "$out/throughput.csv"
      test -s "$out/memory.csv"
      test -s "$out/memory-clock.json"
      test -s "$out/measurement-window.json"
      test -s "$out/sequence.csv"
      "$worktree/target/release/wafer-loadgen" hdr-summary \
        --hdr "$out/latency.hdr" >"$out/latency-summary.json"
      python3 "$REPO/eval/scripts/write-p2-ab-manifest.py" \
        --worktree "$worktree" \
        --arm "$arm" \
        --condition "$condition" \
        --pair "$pair" \
        --output "$out/p2-ab-manifest.json"
    done
  done
done

python3 "$REPO/eval/scripts/analyze-p2-ab.py" \
  --root "$ROOT" \
  --minimum-pairs 10 \
  --output "$ROOT/p2-decision.json"
```

The manifest writer also creates `batch-manifest.json` on its first invocation
and rejects any later invocation whose controlled factors or arm identity do
not match it. The two named Python entry points are delivered with the
measurement task, not this specification task. They must implement this schema
and the formulas in RFC-012 without adding a benchmark dependency. Their
absence, any failed
command, fewer than 10 valid pairs per condition, an arm-order violation, a
hash mismatch, a dirty worktree, a missing/empty mandatory artifact, a sample
count other than 60,000, a sequence gap/duplicate, or a non-finite metric is a
failed experiment and therefore selects `retain-sync-p2`.

`p2-decision.json` uses schema `wafer-p2-async-decision-v1`. It records both
source SHAs, all admitted pair identities, run-level throughput/p95/RSS values,
condition medians, candidate-minus-baseline deltas, correctness and strict-
simplicity results, and exactly one decision: `adopt-async-p2` or
`retain-sync-p2`. Adoption requires every correctness and simplicity gate and,
for every condition, median throughput degradation no greater than 5%, median
p95 latency increase no greater than 5%, and candidate peak RSS increase no
greater than `max(5% of baseline peak RSS, 2 MiB)`. Missing evidence fails
closed to `retain-sync-p2`.

### Final amended contract

The `final_campaign` object in `eval/canonical-matrix.json` is the executable source of truth. It fixes seed 1729, explicit fuel-plus-epoch metering, eKuiper concurrency 1, the five-rate common capacity grid, 2,165 schedule records, and 1,953 executed or static measurement leaves. Every final experiment has `thesis_evidence=true`; diagnostic and focused entries remain in the separate `focused_pilot` object with `thesis_evidence=false`.

Final E-Perf-5 uses explicit transform fuel and epoch protection. Its transform-only pipeline records `filter = null` and `router = null` because those node categories are absent; this is a declared matrix exception, not an unmetered WAFER run.

Final E-Perf-10 requires `capacity-run.json`, `publisher-summary.json`, `subscriber-metadata.json`, `latency.hdr`, `throughput.csv`, `sequence.csv`, `resource-usage.csv`, and `process-audit.json`. `capacity-run.json` uses this counter identity:

```text
intended = rejected + enqueued
enqueued = acked
acked = received_unique + downstream_lost
received_events = received_unique + duplicates
total_undelivered = rejected + downstream_lost
```

It must contain no mandatory per-message traces. `check_capacity_run_result` rejects missing fields, inconsistent counters, non-final evidence labels, unexpected sequences, or trace mode.

Final E-Swap-3 requires `publisher-summary.json`, `subscriber-metadata.json`, 200 contiguous 100 ms buckets in `throughput-buckets.json`, 400 contiguous 10 ms buckets and 40 nested 100 ms parent buckets in `throughput-buckets-10ms.json`, `disruption-timeline.json`, and `disruption-analysis.json`. The 100 ms series stays aligned to the actual action-start `t0`, and the parent buckets equal the matching canonical event-window slice. `disruption-timeline.json` records the actual action start immediately before control issuance, the actual acknowledged/readiness wall-clock end, independent monotonic action duration, the scheduled measured t=60 target, actual event offset, signed alignment error, and a fixed 10 ms tolerance. `swap_timeline.json` is not a final E-Swap-3 artifact and must not remain in the admitted raw leaf. The leaf fails when the actual action start misses the scheduled target by more than 10 ms.

Final E-Swap-4 uses a source-driven 1,000→2,000→1,000 msg/s schedule over measured intervals `[0,55)`, `[55,65)`, and `[65,120)` after 30,000 warmup messages. Each independent run contains exactly 130,000 measured messages and one swap scheduled at measured t=60. `throughput-buckets.json` contains 1,200 contiguous source-origin 100 ms primary buckets over `[0,120s)` plus a separate 100-bucket drain series over `[120s,130s)`. Full-run counters reconcile primary, drain, and O(1) after-drain evidence; any after-drain receive or right-censored drain rejects the run. `swap-actual-t0.json` declares the single source-origin actual-`t0` receipt, and it must reconcile with `swap_requests.json`, `throughput-buckets-10ms.json`, `throughput-buckets.json`, and `burst-timeline.json` on source origin, scheduled t=60, actual request timestamp, alignment error, and the fixed 10 ms tolerance. `burst-timeline.json` records source and actual swap boundaries, monotonic source-completion offset, intended/emitted/received phase populations, sequence loss/duplication, primary/drain completion evidence, the sink-observed gap, and internal swap phases. The semantic verifier rejects wrong clocks, origins, phase rates or populations, zero or multiple swaps, a missing or inconsistent actual-`t0` receipt, a swap outside the burst, a non-centered scheduled swap, bucket gaps, clamped/misclassified tail evidence, completion at or after 130 s, population mismatches, loss, or duplication. Batch analysis admits exactly one event from each of 30 distinct runs before computing the across-run p95 and bootstrap median interval, and separately reports runs with drain arrivals and the maximum drain offset. Missing required files fail through the matrix contract; malformed files fail through experiment-specific semantic checks.

Final E-Swap-1, E-Swap-2, E-Swap-5, and E-Swap-6 use one complete process run as the independent unit and their 50 swap or rollback events as nested observations; E-Swap-4 uses 30 independent runs with one nested swap each. E-Swap-2 and E-Swap-6 are alias views of E-Swap-1 and contribute zero additional independent N.

Final E-Swap-5 contains one independent process run with 50 nested failed-replacement events. Each request must return `rolled_back`; `rollback.json` reconciles all request-indexed internal phases and the lossless full-run sequence. `post-rollback-continuity.json` proves output after the final rollback and explicitly records that no successful v2 transition was observed. A retained or synthesized `swap_timeline.json`, missing continuity, nonzero loss or duplication, or any request/rollback mismatch rejects the leaf.

Final E-Density-1 is a static source-bound measurement. The canonical runner invokes `eval/scripts/collect-binary-sizes.sh` instead of `run-experiment.sh`, requires one `binary-sizes.csv` row for every non-comment entry in `eval/scripts/binary-sizes.index`, and records Pi host, tag, governor, throttling, telemetry, and measurement-window evidence. Its metadata uses `system = "static"` and `exit_codes.collector = 0`; runtime/Wasmtime provenance is intentionally inapplicable because no WAFER runtime process executes.

### v10 enhanced candidate contract

The signed v4 final campaign remains immutable. The signed v5 candidate is retained
as a rejected pre-format artifact because its volume label exceeds exFAT's 11
UTF-16 code-unit limit. The signed v6 candidate is retained as a rejected
pre-workload artifact because its deployment payload omits the results-layout module
required by the canonical runner. The signed v7 candidate is retained as a rejected
targeted-validation artifact because its eKuiper diagnostic validator rejects a
contract-valid bounded terminal partial interval. The signed v8 candidate is retained
as a rejected targeted-validation artifact because its E-Swap-3 fine-bucket producer
omits metadata required by the shared validator. The signed v9 candidate is retained
as a rejected targeted-validation artifact because downstream eKuiper consumers
reject contract-valid bounded terminal partial intervals. Enhanced work uses the
separate `enhanced_candidate` object in `eval/canonical-matrix.json` and the corrective
v10 release lineage. It does not modify or supersede the existing primary estimands.

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
contract for twelve candidate and diagnostic chart/table families. For the
completed expanded N=5 diagnostic, the separate mixed-lineage renderer writes
one deterministic SVG, PNG, PDF, CSV, and HTML table per family under
`derived/expanded-n5/<batch-id>/`, an artifact manifest in the same directory,
and an explanatory local-link report under `reports/expanded-n5/<batch-id>/`.
The pre-results renderer retains `derived/enhanced-n5/<batch-id>/` and
`reports/enhanced-n5/<batch-id>/`. On macOS exFAT mounts, validators treat only
well-formed AppleDouble metadata with a declared counterpart as filesystem
metadata; orphan, malformed, and ordinary undeclared files remain errors. Neither
renderer writes below `raw/` or `manifests/`.

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

Prospective v10 evidence uses one physical exFAT volume labeled
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
verification; every error count must be zero. After a terminal rehearsal PASS,
the `seal` command rejects active evidence writers, validates the immutable
terminal reconciliation, writes one sorted volume-relative SHA-256 manifest for
the complete `raw/` tree, and builds a composite index containing one selected
passed leaf per physical key, immutable aliases, the qualified prerequisite, and
unselected failed attempts. Production defaults require the exact 661-record
composition (623 unique selected raw leaves, 37 aliases, one B00 prerequisite,
and four excluded failed attempts). It then synchronizes and re-verifies the raw
tree before writing the Pi source-host seal. Existing seal paths are never
overwritten; an interrupted seal can resume only from byte-identical manifest
and composite artifacts. macOS and Jetson handoff receipts bind the same UUID, stable
identifier, raw manifest, source-host seal, and composite index before analysis
may open the raw tree. Cross-host checks use path, size, and SHA-256 identity;
source-local modification times remain recorded but are not treated as portable
exFAT identity. The exact signed verifier bytes recorded in the source seal are
required at handoff. The qualification tooling does not format, relabel, mount,
unmount, copy, or delete storage.

Completed expanded-N5 analysis uses the separate
`wafer_analysis.expanded_n5` adapter. It requires the validated composite and
macOS handoff receipt, rehashes the complete raw manifest, and permits only the
exact v10-v13 tag/SHA tuples and accepted control generations recorded by the
composite. Canonical and N=30 consumers remain strict single-release
interfaces. The adapter never treats aliases as extra replicates and never
selects a different attempt by directory scan.

Every normalized result-time row retains its source result key, release tag/SHA,
control generation, relative artifact path, and SHA-256. Cross-release matched
arms are labeled `release-confounded`, excluded from paired estimators, shown as
separate arms, and accompanied by release-stratified sensitivity tables. The
signed v16 tag/SHA describes only the analyzer and derived outputs; it never
relabels v10-v13 evidence. Output is diagnostic and non-poolable under
`derived/` and `reports/` only. Each of the twelve families emits CSV, SVG, PNG,
PDF, and HTML, plus one hash-bound result-time observations artifact. Family
observations cannot retain `PRE-RESULTS` or `PENDING`; genuine external gaps,
including unmatched x86 E-Perf-5, remain report-level pending limitations.

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
- every E-Backpressure policy crossed its queue threshold, drained, reconciled
  its policy-specific counters and sequence evidence, and stayed within the frozen RSS bound;
- E-Iso-4 recorded contained traps with one recovery per trap and no reuse of
  an interrupted component instance;
- E-Iso-7 uses independent source and sink populations, a branch-local
  post-warmup measurement boundary, and lossless branch-A counts rather than
  shared-source or aggregate fan-in values;
- E-Perf-9 records every monotonic startup phase, one processed message, and
  valid compiled-cache state;
- E-Swap-1/2/4/6 keep internal phases and sink-observed gaps as distinct
  nanosecond fields with the frozen event count;
- E-Swap-3 and E-Swap-5 preserve sequence integrity; E-Swap-5 records all
  requested rollbacks, post-rollback output, and no successful-v2 transition;
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

The phases start at `process_started`, the first statement of the async boot
sequence, after the tokio runtime is built. `process_entry` records the earlier
point: `unix_epoch_ns` is taken as the first statement of `main`, before the
runtime exists, for alignment with the harness's pre-exec `runtime_started_ns`;
`to_process_started_ns` is the monotonic gap from entry to `process_started`.
Neither value is part of `phases_ns` or `total_wall_duration_ns`, and exec,
dynamic loading and static constructors before `main` remain outside both.

The runtime performs no provenance work between `launch_completed` and the
first sink collection: during the probe `runtime-provenance.json` is written
only at shutdown, and the runtime-binary hash is computed on the blocking pool
after the run.

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

### Runtime exit status

`exit_codes.wafer_runtime` is the only in-band failure signal the harness
has, so the runtime never exits `0` for a run that failed. The canonical
runner rejects any non-zero code (`RuntimeError: wafer runtime exited with N`).

| Code | Meaning | Artefacts |
| --- | --- | --- |
| `0` | Every node task exited cleanly (natural completion or SIGTERM/SIGINT drain). | All runtime-owned artefacts. |
| `1` | Startup failed for a reason other than configuration: engine creation, plugin compile, control-plane bind, or a `startup.json` write after an otherwise clean run. | Whatever was written before the failure; usually none. |
| `2` | Invalid configuration: TOML load, semantic validation, or a source/sink `validate()` run at launch (for example a bench-source `rate = 0` or an inconsistent burst block, a mistyped HTTP `bind`), a local plugin path that cannot be read, or bad `--swap-*` arguments. Nothing is spawned, except that a `--swap-node` that is not a swappable Wasm node is only found after launch and the pipeline is drained first. Also clap's code for bad arguments. | None. |
| `3` | The pipeline started but failed while running: a node task panicked (the log names the node), a source/sink `init()` failed (the pipeline is cancelled at once), a source hit its poll-error budget, a Wasm node could not be re-instantiated after a trap or was torn down by its error policy (the pipeline is cancelled at once), a sink's final flush or `close()` failed, a node did not stop within the 5 s shutdown deadline and was aborted, the DLQ sink failed or did not stop in time, or a `--swap-after-secs` swap could not be prepared or dispatched. | Bench artefacts and `runtime-provenance.json` are still flushed before exiting, for post-mortem analysis only; the leaf is not a valid sample. |
| `130`, `143` | A second SIGINT (`130`) or SIGTERM (`143`) arrived before the graceful shutdown finished, and the runtime exited at once. | Whatever was written before the second signal; the leaf is not a valid sample. |

A run killed by the harness after its SIGTERM grace period reports the
signal's code (e.g. `137`), not one of the above.

`wafer-loadgen publish --profile hotswap-trigger` follows the same rule: if
the hot-swap POST fails (transport error or non-2xx), it still writes
`--hotswap-result-path` with an `error` field (and `http_status` when a
response arrived) and the summary file, with `hotswap_triggered_at_secs: null`, then exits non-zero. A `--hotswap-swap-at-secs` at or beyond `--duration-secs` is rejected before publishing.

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
| `provenance_written_at` | `wafer-runtime` | `launch`, `swap` or `shutdown`: which write the sidecar holds. A completed run holds `shutdown`. |
| `wasmtime_version`, `wasmtime_source` | `wafer-runtime` build.rs, parsed from workspace `Cargo.lock` | The resolved dep version differs from the Cargo.toml declaration when wasmtime is pulled from git; the `source` line carries the git rev, which the version alone does not identify. |
| `ort_sys_version`, `ort_sys_source`, `ort_link` | `wafer-runtime` build.rs: `Cargo.lock` and `ORT_LIB_LOCATION` | Which ONNX Runtime binding was built and whether the library came from `ort-sys` `download-binaries`, a local `ORT_LIB_LOCATION`, or the `system` (download feature off). |
| `runtime_build` | `wafer-runtime` build.rs: `PROFILE`, `OPT_LEVEL`, `TARGET`, `CARGO_ENCODED_RUSTFLAGS`, `CARGO_FEATURE_*`, `git rev-parse HEAD`, `git status` | Explains two different `wafer_runtime_sha256` values and ties the binary to the commit it was built from (a stale binary cannot inherit the harness's run-time `git_sha`). `git_dirty` reflects the tree when the build script last ran. Values are strings; `unknown` when git is unavailable. |
| `rustc_version` | `wafer-runtime` build.rs, `rustc --version` at compile time | Cross-compilation drops the host rustc; build-time capture keeps provenance intact. |
| `wafer_runtime_version` | `env!("CARGO_PKG_VERSION")` | Semver of the binary that ran, not the workspace. |
| `wafer_runtime_sha256` | `std::env::current_exe()` + SHA256, on the blocking pool after the run | Exact binary bytes so a canonical-run number can be tied to the exact build artefact. `null` in `launch` and `swap` writes. |
| `wafer_plugin_hashes` | `PipelineOrchestrator::plugin_hashes_snapshot()` | Populated at initial launch AND after every adopted hot-swap (API or timed `--swap-after-secs`) by `PipelineHandle::record_plugin_hash`; the P0.12 hot-swap guard reads from the same map, so metadata + guard stay coherent (F2 AC2). The shutdown write therefore names the live replacement. |
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
