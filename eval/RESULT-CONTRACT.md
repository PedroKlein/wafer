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

Each batch ledger under `manifests/canonical-batches/` or `manifests/candidate-batches/`
holds `schedule.json`, `progress.jsonl`, and `batch.json`. The runner writes `batch.json`
once, when the batch starts: `schema_version`, `batch_id`, `host`, `source_git_sha`,
`source_dirty`, `canonical_matrix_sha256`, `seed`, sorted `experiments`, `repetitions`
(the diagnostic override or `null`), `thesis_evidence` (true only for a
`canonical-batches` ledger without `repetitions`), and `started_at`. A resume from another
source SHA or matrix hash is refused. `mise run approve-batch` adds `raw.sha256`: one
`sha256sum` line per file of the batch under `raw/`, its alias receipts, and its ledger,
sorted, with volume-relative paths. It records the batch and the SHA-256 of `raw.sha256`
in the repository file `eval/final-batches.json`, which canonical analysis reads.

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
| `service.hdr`, `source-lag.hdr` | In-process runs whose source is `BenchSource` | `BenchSink` | Two parts of the `latency.hdr` value, same interval-log encoding and bounds. `service.hdr` (tag `service_ns`): arrival minus the time the message left the source. `source-lag.hdr` (tag `source_lag_ns`): time the message left the source minus its scheduled time. Lag grows when the pipeline pushes back on the source; a run whose lag is close to its latency is limited by the source, not by the stage under test. Both are absent when no message carried `BenchSource` stamps. |
| `interval-latency.json` | Timed runs with latency output | `BenchSink` or `wafer-loadgen subscribe` | Bounded producer fragment containing one-second latency histograms reduced to p50/p95/p99 and event/unique/duplicate counts. This is an implementation input to the composer, not a replacement for `latency.hdr`. Both producers write the same keys: `schema_version`, `interval_clock` (`monotonic-elapsed`), `alignment_clock` (`unix-epoch`), `alignment_clock_purpose`, `measurement_start_unix_epoch_ns`, `declared_measurement_duration_ns`, `bucket_width_ns`, `maximum_rows`, `row_count`, `late_arrivals`, `aggregate_latency_count` (the `latency.hdr` population) and `rows`. Each row has `interval_start_ns`, `interval_end_ns`, `interval_start_unix_epoch_ns`, `interval_end_unix_epoch_ns`, `latency_count`, `latency_p50_ns`, `latency_p95_ns`, `latency_p99_ns` (null for an empty bucket), `received_events`, `throughput_messages` and `duplicates`. |
| `interval-metrics.json` | Timed runs with latency or throughput output | `canonical_runner.py` or `interval_metrics.py` | Bounded one-second rows combining latency, throughput, CPU, RSS, PMIC internal-rail proxy, temperature, and queue observations. Missing observations use an explicit `unavailable` status and reason. |
| `throughput.csv` | Same as `latency.hdr` | `BenchSink` (in-process) or `run-experiment.sh` (E2E) | The two producers write different files under this name. `BenchSink`: `elapsed_secs,msg_count,bytes`, one row per fixed 1 s bucket counted from the first post-warmup arrival up to the last arrival. `elapsed_secs` is the bucket's end in whole seconds, a second with no arrivals between two that have some is a zero row, and the last row is the second in which the last message arrived. `msg_count` counts every post-warmup arrival, duplicates included. E2E: `run-experiment.sh` derives one summary row from `subscriber-metadata.json` with `eval/scripts/lib/write_throughput.py`: `timestamp_ns,messages_received,throughput_msg_s,duration_ns` (end of the subscriber run, `total_recorded`, their ratio, and the subscriber's run duration). |
| `sequence.csv` | Loadgen with sequence tracking, or `BenchSink.track_sequences = true` (E-Perf-1..3, E-Perf-8, E-Perf-10, E-Backpressure, E-Swap-*) | `wafer-loadgen subscribe` / `BenchSink` | Exact per-message sequence accounting: every sequence number in the declared population is marked when it arrives, so a late arrival is a reorder (not a gap plus a duplicate), a duplicate is a number seen twice, and a number never seen is missing whether or not a later one arrived. `BenchSink` writes one summary row (`total_expected,total_received,received_unique,gap_ranges,gap_msgs,duplicates_count,out_of_order,out_of_range`) over the population from the measurement start sequence up to the sequence end that `BenchSource` stamps on every message (its per-message warmup stamp excludes the warmup population), so `gap_msgs` includes messages missing at the tail; without those stamps the population runs from the first measured number to the highest one seen. `total_expected = received_unique + gap_msgs` and `total_received = received_unique + duplicates_count + out_of_range`; `out_of_range` (a number outside the population) must be zero. `wafer-loadgen subscribe` writes long-form events (`event_type,seq_start,seq_end,count`): one `gap` row per run of missing numbers (tail included when `--sequence-end-exclusive` declares the population) and one `duplicate` row per retained example, bounded by `--sequence-example-limit`; zero data rows when lossless. Its totals live in `subscriber-metadata.json`. |
| `memory.csv` | E-Perf-6, E-Perf-7, E-Perf-8, E-Backpressure (and any run with `WAFER_BENCH_OUTPUT_DIR` set) | `wafer-runtime` via `MemoryRecorder::sample_loop` (1 Hz, cross-platform via `memory-stats` crate; on Linux it reads `/proc/self/statm`). Flush on graceful shutdown. | 1 Hz process-RSS timeline of `wafer-runtime`: `elapsed_ms,rss_bytes`. Runtime-owned since A19 closure (thesis-hardening T4). Legacy harness `ps` polling removed. |
| `memory-clock.json` | Runs with `memory.csv` | `wafer-runtime` | Pairs the memory recorder's monotonic zero with one Unix-epoch anchor for cross-process interval composition: `schema_version`, `elapsed_clock` (`monotonic`), `alignment_clock` (`unix-epoch`), `alignment_clock_purpose` and `start_unix_epoch_ns`. |
| `per_node_metrics.csv` | All experiments (emitted on graceful shutdown when `WAFER_BENCH_OUTPUT_DIR` set) | `wafer-runtime` via `PipelineOrchestrator::export_per_node_metrics` | One row per pipeline node: `node_id,messages_in,messages_out,filtered_out,traps_total,traps_memory_out_of_bounds,traps_unreachable,traps_interrupt,traps_out_of_fuel,traps_memory_limit,traps_other,guest_bad_input,guest_dependency_failed,guest_processing_failed,guest_timed_out,guest_unrecoverable,attempts_failed,retries,dlq_sent,dlq_lost,skipped,retry_exhausted_skips,dropped_on_recovery,dropped_on_teardown,error_state_seconds,recovery_count`. `messages_in` is the node's input-queue dequeue count (0 for sources). `traps_*` count calls the host aborted, by kind; `guest_*` count errors the guest returned, by WIT category; `attempts_failed` counts every failed call, retries included. Every processing node satisfies `messages_in = messages_out + filtered_out + skipped + retry_exhausted_skips + dlq_sent + dlq_lost + dropped_on_recovery + dropped_on_teardown` (pending retries are flushed to the DLQ at shutdown). Every config under `eval/configs/` has a file DLQ, so a message the error policy removes, or whose call trapped, is counted as `dlq_sent` and has a record in `dlq.jsonl`; `dlq_lost` counts records refused by a full or closed DLQ, and `dropped_on_recovery` stays zero unless a pipeline runs without a DLQ. Runtime-owned since A19 closure (thesis-hardening T4). |
| `dlq.jsonl` | All experiments (every config under `eval/configs/` sets `[dead_letter] kind = "file"`, `path = "dlq.jsonl"`, which the runtime resolves under `WAFER_BENCH_OUTPUT_DIR`) | `wafer-runtime` DLQ sink | One JSON object per dead-lettered message, written as it arrives: `timestamp` (ms since the Unix epoch), `source_node`, `error_category` (`bad_input`, `dependency_failed`, `processing_failed`, `timed_out`, `unrecoverable`, or null for a trap or queue overflow), `error_message`, `retry_count`, `reason` (`type` one of `bad_input`, `timed_out`, `retries_exhausted`, `retry_buffer_full`, `hot_swap_drain`, `shutdown`, `queue_full`, `trapped` with `kind`, `unrecoverable`, `recovery_failed`), `original` (id, timestamp, source, metadata, base64 payload, retry_count), `trace_id`, `parent_id`. The file exists, possibly empty, for every graceful run; the sink drains until every node has exited, so the record count equals the sum of `dlq_sent` over `per_node_metrics.csv` plus the edges' `dead_lettered` counts. |
| `queue-depth.csv` | E-Backpressure, when `WAFER_QUEUE_DEPTH_OUTPUT` is set | `wafer-runtime` via `QueueDepthRecorder` | Bounded internal Tokio queue samples at 10 ms intervals: `elapsed_ns,queue,depth,capacity,accepted,dequeued,processed,dropped,dead_lettered,downstream_closed,dlq_full,dlq_closed`. The five policy counters expose existing R1 `QueueSnapshot` state without changing dispatch behavior. Counters are internal pipeline observations; broker backlog is excluded. Collection is capped at 131,072 rows and reports truncation in the runtime log. |
| `queue-depth-clock.json` | Runs with `queue-depth.csv` | `wafer-runtime` | Pairs the queue recorder's monotonic zero with one Unix-epoch anchor for cross-process interval composition. Same keys as `memory-clock.json`. |
| `backpressure.json` | E-Backpressure | `canonical_runner.py` | Schema v2 records condition/policy, run index, measured queue, threshold crossing and recovery, offered/accepted/processed/drained rates, exact R1 queue counters, sequence counts, policy equation, producer-progress mode, explicit DLQ-full/closed failures, and peak-RSS bound. Attempted messages come from the frozen BenchSource population; `sequence.csv` validates its observed span because a trailing overflow disposition is not visible at the sink. `slow` requires attempted = delivered with no overflow disposition; `drop` requires attempted = delivered + dropped; `dead-letter` requires attempted = delivered + dead_lettered + dlq_full + dlq_closed. `dead_lettered` is the runtime's successful enqueue to the configured DLQ delivery path; full/closed failures are never counted as success. A high offered rate without measured occupancy is `not-saturated`. |
| `recovery.csv` | E-Iso-8 and any run with recovery events | `wafer-runtime` via `PipelineOrchestrator::export_per_node_metrics` | Exact recovery samples: `node_id,sample_index,duration_ns`. Retains nanosecond precision for trap-to-running percentiles. |
| `measurement-window.json` | Canonical Pi 5 runs | `BenchSink` for in-process runs with post-warmup output; canonical harness for external MQTT subscribers and zero-output containment runs | Exact `started_ns` and `finished_ns` bounds (wall clock) for excluding warmup and teardown from PMIC energy integration. The `BenchSink` copy adds `latency_clamps`: for each of `latency`, `service` and `source_lag`, the `negative` and `above_highest` sample counts recorded at a histogram bound. The verifier rejects a canonical leaf where any of them is non-zero. It also adds `wall_clock_step_ns`: how far the wall clock moved away from the in-process monotonic clock between the process start and the export, signed. In-process latencies do not depend on it; it tells whether `started_ns` and `finished_ns` can be aligned with other processes' telemetry. |
| `pi-telemetry.csv` | Canonical runs on every host | `eval/scripts/lib/pi_telemetry.py` | Timestamped temperature, CPU frequency, governor, throttling state, and summed rail watts from the host's backend (`rail_proxy_watts`). `throttled` is `0x0` when the host is not throttled on every backend: the Pi writes the `vcgencmd get_throttled` value, Jetson and x86 write `cpuN-below-pinned-clock` when a core runs under 95% of its pinned clock, and x86 adds `cpuN-thermal-throttle` when the kernel's throttle counter moved. The temperature comes from the `cpu-thermal` (Pi, Jetson; `CPU-therm` on L4T R32) or `x86_pkg_temp` zone, falling back to `k10temp`/`coretemp` hwmon, and is 0 when none exists. The power column is 0 while the backend has no reading (the first x86 RAPL sample, or an x86 host without RAPL), which `interval_metrics.py` reports as unavailable. |
| `pmic-rails.csv` | Canonical runs on every host | `eval/scripts/lib/pi_telemetry.py` | Long-form named rail samples (`timestamp_ns,rail,current_a,voltage_v,power_w`). Pi: PMIC rails, without rails lacking either voltage or current. Jetson: INA3221 channels from hwmon. x86: one row per RAPL package domain with `power_w` from the energy delta since the previous sample and empty current and voltage; header only when the host has no RAPL. |
| `power-boundary.json` | Canonical runs on every host | `eval/scripts/lib/pi_telemetry.py` | Names the `backend` (`pi`, `jetson`, `x86`) and its `measurement` (`rpi5-pmic-internal-rail-proxy`, `jetson-ina3221-rail-proxy`, `x86-rapl-package-energy`, or `unavailable`), states that none is total input power, and records the excluded consumers and the source. Power numbers are comparable only between leaves with the same `measurement`. |
| `cpu-cores.csv` | Canonical runs (every leaf the canonical runner or `run-experiment.sh --canonical` produces) | `eval/scripts/lib/proc_telemetry.py` | One row per CPU per second from `/proc/stat`: `timestamp_ns,cpu,user,nice,system,idle,iowait,irq,softirq,steal,frequency_hz`. Jiffies are cumulative since boot, so a reader takes the difference between consecutive rows and divides by `clock_ticks_per_second` from `host-sidecar.json`; `frequency_hz` is that core's `scaling_cur_freq` (empty where cpufreq is absent). This is what tells SUT-core utilisation on CPUs 1-3 apart from support-core utilisation on CPU 0. |
| `host-sched.csv` | Same as `cpu-cores.csv` | `eval/scripts/lib/proc_telemetry.py` | One row per second of system-wide scheduler and memory-pressure counters: `timestamp_ns,ctxt,processes,procs_running,procs_blocked,mem_available_bytes,psi_cpu_some_avg10,psi_memory_some_avg10,psi_memory_full_avg10,psi_io_some_avg10,psi_io_full_avg10,sampler_cpu_ms`. `ctxt` and `processes` are cumulative; PSI columns are empty on kernels without `/proc/pressure`; `sampler_cpu_ms` is the CPU time the sampler itself spent since its previous row, so every leaf carries its own overhead evidence. |
| `sut-processes.csv` | Same as `cpu-cores.csv` | `eval/scripts/lib/proc_telemetry.py` | One row per second per tracked process (`wafer`, `wafer-runtime`, `wafer-loadgen`, `kuiperd`, `mosquitto`, matched on `/proc/<pid>/comm`): `timestamp_ns,pid,comm,utime,stime,minflt,majflt,voluntary_ctxt_switches,nonvoluntary_ctxt_switches,threads,rss_bytes,cpus_allowed_list`. Counters are cumulative for the process's lifetime. Support processes are recorded next to the SUT so the leaf shows where the broker and the load generator ran. |
| `host-sidecar.json` | Same as `cpu-cores.csv` | `eval/scripts/lib/proc_telemetry.py` | Written when the sampler stops: `schema_version`, `sampler`, `interval_secs`, `started_unix_epoch_ns`, `finished_unix_epoch_ns`, `samples`, `sampler_affinity` (the CPU list it pinned itself to, the support cpuset under the canonical runner), `sut_cpus`, `sampler_cpu_seconds` and `sampler_max_rss_bytes` (its total cost), `clock_ticks_per_second`, `page_size_bytes`, `cpu_count`, `boot_id`, `kernel_cmdline`, `uptime_secs_at_start` and `tracked_comms`. If the sampler dies it writes `host-sidecar-error.json` (`error`, `timestamp_ns`) instead; the four files are supplementary host evidence, so their absence does not reject a leaf. |
| `published.csv`, `received.csv` | Historical E-Perf-10 diagnostics only | `wafer-loadgen` opt-in tracing | Raw publisher/subscriber timestamp and sequence samples retained for v11-v17 compatibility. Final E-Perf-10 rejects these mandatory traces and uses bounded summaries. |
| `publisher-summary.json` | Final E-Perf-10 and Final E-Swap-3 | `wafer-loadgen publish` | `schema_version`, bounded `intended`, `rejected`, `enqueued`, `acked`, and `unacked_at_exit` counters plus the scheduled `measurement_duration_ns`. `intended = rejected + enqueued` and `enqueued = acked + unacked_at_exit` are mandatory: `enqueued` is what was handed to the MQTT client, `acked` is what the broker acknowledged (QoS 1 PUBACK), and `unacked_at_exit` is what was still unacknowledged when the 5 s drain window after a completed schedule closed (a run stopped by a signal skips the drain). `connects` counts successful CONNACKs and `connection_errors` counts event-loop errors; the clock starts only after the first CONNACK (10 s timeout, then the run fails without a summary). The verifier rejects `connects != 1` or `unacked_at_exit != 0`, and loss downstream of the publisher is `acked - received_unique`. `published` and `errors` repeat `intended` and `rejected` under their older names. `elapsed_ms` and `actual_rate` are the wall time the schedule took and `intended` divided by it; `deadline_misses` counts messages whose hand-off to the client ended after the next message was due; `hotswap_triggered_at_secs` is the swap offset for the `hotswap-trigger` profile and null otherwise. `source_lag_ns` (`count`, `p50`, `p99`, `p999`, `max`) is how late each message was handed to the MQTT client relative to its scheduled time; payload `ts` is the scheduled time, so this lag is part of the measured latency. `exit_reason` is `duration` when the schedule ran to its end, `sigterm`/`sigint` when a signal stopped it, or `broker-lost` when the MQTT session ended for good mid-run; the counters then cover the messages offered before the stop, and the verifier rejects the run. A second signal exits at once without a summary. |
| `subscriber-metadata.json` | Final E-Perf-10 and external-MQTT experiments | `wafer-loadgen subscribe` | Bounded receive, duplicate, ignored-warmup, parse, latency, and sequence totals: `broker`, `topic`, `started_at_ns`, `ended_at_ns`, `git_sha`, `host_tag`, `sequence_end_exclusive`, `total_messages` (every message read), `total_recorded` (the `latency.hdr` population), `ignored_sequence_count` (warmup), `parse_errors`, `latency_min_ns`, `latency_max_ns`, `latency_mean_ns`, `latency_p50_ns`, `latency_p95_ns`, `latency_p99_ns`, `latency_p999_ns`, the histogram range `histogram_lowest_ns`, `histogram_highest_ns`, `histogram_sig_digits`, and `sequence` (`expected`, `total_received`, `received_unique`, `total_gaps` (missing, tail included when `sequence_end_exclusive` is set), `total_duplicates`, `out_of_order`, `out_of_range`, plus bounded `gap_ranges` and `duplicate_seqs` examples and `examples_truncated`). The HDR count and received-event population must match. `negative_latency_count` and `above_highest_latency_count` count samples recorded at a histogram bound; `clock_steps` counts wall-clock steps larger than 100 ms seen between received messages (the gap between the wall clock and the monotonic clock changed). Latency subtracts the publisher's wall-clock schedule from the subscriber's wall-clock receive time, so a step shifts every later sample; the verifier rejects the run when any of the three is non-zero. `exit_reason` is `total-messages`, `sigterm`, `sigint`, or `eof`. `status` is `complete`, or `partial` when `interval-latency.json` or `throughput-buckets.json` overflowed its bound or failed its checks; `partial_reasons` names each failure. A partial run keeps `latency.hdr`, `sequence.csv` and this file, leaves out the failed artifact, exits non-zero, and is rejected by the verifier. Written last, so its presence means the other subscriber artifacts are complete. A second SIGTERM or SIGINT exits at once without flushing. |
| `export-errors.json` | Any run with a `BenchSink` output directory, only when an artifact failed | `BenchSink` | `{"schema_version":1,"errors":[{"artifact":…,"error":…}]}`. `BenchSink` writes `sequence.csv`, `measurement-window.json`, `swap_timeline.json`, `latency.hdr`, `service.hdr`, `source-lag.hdr` and `throughput.csv` first and the derived artifacts after, and one failed artifact does not stop the rest. Its presence rejects the leaf. |
| `capacity-run.json` | Final E-Perf-10 and candidate E-Perf-Capacity-Knee | `canonical_runner.py` | Trace-free run summary containing intended/rejected/enqueued/acked/received/lost/duplicate counts, offered and achieved rates, loss, p50/p95/p99, CPU, RSS, thermal state, process/config receipts, and controlled factors. Final E-Perf-10 sets `thesis_evidence=true`; capacity-knee sets `evidence_class=candidate-supplementary`, `thesis_evidence=false`, and `n30_admitted=false`. |
| `resource-usage.csv` | E-Perf-10 and E-Perf-Capacity-Knee | `canonical_runner.py` | One-second SUT samples: wall-clock timestamp, aggregate process CPU ticks, RSS bytes, and process count. MQTT loopback records the explicit no-SUT zero baseline. |
| `process-audit.json` | E-Perf-10 and E-Perf-Capacity-Knee | `canonical_runner.py` | Active SUT PID, process affinity, exclusivity, and allowed CPU set captured before measurement. |
| `rate-sweep.json` | Historical E-Perf-10 diagnostics only | Earlier `canonical_runner.py` versions | Legacy trace-backed run summary, always `thesis_evidence=false`. It remains readable but is not accepted as a final-capacity leaf. |
| `throughput-buckets.json` | Final E-Swap-3 and E-Swap-4 | `wafer-loadgen subscribe` / `BenchSink` / `canonical_runner.py` | Contiguous 100 ms output-rate buckets with unique/event/duplicate counts. E-Swap-3 bins exactly -10 s through +10 s around the actual disruption event. E-Swap-4 keeps 1,200 source-origin primary buckets over `[0,120s)` and a separate 100-bucket drain series over `[120s,130s)`, with O(1) after-drain evidence. Unix-epoch values are labeled for source/sink or cross-process alignment only. |
| `throughput-buckets-10ms.json` | Final E-Swap-3 and E-Swap-4 | `wafer-loadgen subscribe` / `BenchSink` | Exactly 400 contiguous 10 ms buckets over `[-2s,+2s)` around actual `t0`, plus 40 nested actual-t0-aligned 100 ms parent buckets. Both levels carry unique/event/duplicate counts and reconcile exactly. E-Swap-3 parent buckets also match the canonical event-window slice; E-Swap-4 retains its separate source-origin primary/drain series as the loss-accounting authority. No message identity or timestamp trace is retained. |
| `disruption-timeline.json` | Final E-Swap-3 | `canonical_runner.py` | Strategy, actual Unix-epoch action start/end for cross-process alignment, monotonic scheduling/duration labels, the scheduled t=60 target, measured offset, signed alignment error, and a 10 ms maximum alignment tolerance. This is the single retained runner-owned final action timeline for E-Swap-3. |
| `disruption-analysis.json` | Final E-Swap-3 | `canonical_runner.py` | Derived disruption summary containing baseline and event-window rates, dip percentage, interruption and recovery timings, action duration, loss, duplicates, and latency percentiles. |
| `burst-source-timing.json`, `burst-source-summary.json` | Final E-Swap-4 | `BenchSource` | Source-origin timestamp, monotonic source-completion offset, fixed phase boundaries, and exact intended/emitted populations for the 1,000 to 2,000 to 1,000 msg/s burst. |
| `swap-actual-t0.json` | Final E-Swap-4 | `canonical_runner.py` | Runner-owned actual-`t0` receipt declaring the source origin, scheduled swap timestamp, actual request timestamp, signed alignment error, and fixed 10 ms tolerance for the single measured swap. |
| `burst-timeline.json` | Final E-Swap-4 | `BenchSource` / `canonical_runner.py` | One 1,000 to 2,000 to 1,000 msg/s burst with boundaries at measured seconds 55 and 65 and exactly one successful swap scheduled at second 60, plus actual alignment, phase populations, primary/drain completion evidence, sequence integrity, sink gap, and internal swap phases. |
| `startup-preparation.json` | E-Perf-9 | `run-experiment.sh` | Filesystem-cache condition and preparation action completed before the timed runtime process starts. |
| `startup.json` | E-Perf-9 | `wafer-runtime` | Monotonic process/config, component load/compile, instantiation, pipeline setup, and first-process durations; total startup duration; exactly-one-message proof; plugin SHA-256; and explicit compiled-component cache state. |
| `containment.json` | E-Iso-1..8 | `canonical_runner.py` | Containment verdict for one attack condition: expected condition, attack node, expected mechanism (the `per_node_metrics.csv` column that must count the attack, see `eval/scripts/lib/containment.py`), its count, unexpected outcomes on the attack node, trap total, runtime-panic flag, healthy-node output count, per-node runtime metrics, and the dead-letter evidence: `dlq_sent_total` (sum over nodes) and `dlq_records` (lines in `dlq.jsonl`, null when the file is absent). The verifier rejects an E-Iso-1..6 leaf whose `dlq.jsonl` exists but holds a different number of records than its nodes sent to the dead-letter queue. An attack counts as contained only when the expected mechanism stopped it at least once and nothing else happened on that node: no other trap kind or guest error, and no message passed on. `contained` is null for conditions without an attack. The runner fails the run when `per_node_metrics.csv` is malformed or lacks the attack node's counters. The analysis never counts a record whose condition differs from the attack, and it counts runs that were not contained or recorded a runtime panic instead of dropping them. |
| `branch-a/`, `branch-b/` | E-Iso-7 | `BenchSink` | Independent post-warmup latency histogram, throughput series, sequence accounting, and measurement window for each branch. Each branch has its own `BenchSource`; root-level fan-out/fan-in measurements are forbidden for branch-impact analysis. |
| `branch-isolation.json` | E-Iso-7 | `canonical_runner.py` | Branch-local source identity, configured post-warmup target count, actually offered/received post-warmup counts, target shortfall, throughput samples, latency percentiles, measurement boundaries, and explicit units. |
| `branch-isolation-summary.json` | E-Iso-7 batch ledger | `canonical_runner.py` | Separate branch-A throughput-drop and p95-latency-increase rows for panic and epoch-loop attacks, including run counts and units. |
| `host-load-ladder.json` | E-Host-Thermal-Storage | `characterize-rpi5-host.sh` | Append-only clean-boot session receipt with the exact eight-phase order, per-phase pass/fail/not-run status, 75 °C stop limit, boot identity, bounded sample counts, diagnostic/final admission decisions, source state, and the PMIC internal-rail boundary. |
| `host-telemetry.csv` | E-Host-Thermal-Storage | `characterize-rpi5-host.sh` | One-second phase-labeled temperature, CPU frequency, throttling, PMIC internal-rail proxy, memory availability/pressure, USB throughput, boot ID, wall-clock, and monotonic samples. No per-message data. |
| `kernel-io.log`, `usb-integrity.json` | E-Host-Thermal-Storage | `characterize-rpi5-host.sh` | Bounded matching kernel I/O errors and per-USB-phase byte/duration/SHA-256 reconciliation. Any recorded kernel I/O error or hash mismatch stops the ladder and blocks final admission. |
| `ekuiper-runtime-summary.json` | E-Compare-eKuiper-Profile | `canonical_runner.py` | Diagnostic run identity, interval alignment, latency percentiles, bounded external `/proc` process summary when available, and explicit eKuiper 2.1.5 GC/runtime-unavailability status. It never infers GC events from RSS or latency. |
| `profiler-overhead.json` | E-Compare-eKuiper-Profile | `canonical_runner.py` | Profiler state, matched rate/run pair, collection-enabled flag, and the run-level profiled-minus-control estimator label. It declares association-only interpretation and is not primary evidence. |
| `swap_requests.json` | E-Swap-1, E-Swap-2, E-Swap-4, E-Swap-5, E-Swap-6 | `canonical_runner.py` HTTP client | One record per API request with request boundaries, monotonic `request_duration_ns`, HTTP status, and the runtime's typed internal outcome phases. E-Swap-5 requires 50 process-trap requests whose response status is `rolled_back`. |
| `swap_timeline.json` | E-Swap-1, E-Swap-2, E-Swap-4, E-Swap-6 | `BenchSink` | Sink-observed successful plugin-version transitions. Each `pause_ns` is an output interarrival gap and is not an internal swap duration. E-Swap-5 forbids this artifact because the rejected v2 never becomes sink-observed. |
| `hotswap-analysis.json` | E-Swap-1, E-Swap-2, E-Swap-4, E-Swap-6 | `canonical_runner.py` | Index-matched API and sink observations with explicit `*_ns` names: internal phases, `http_total_ns`, and `sink_observed_output_gap_ns`. Includes the unique measurement source leaf so shared E-Swap-2/6 views do not multiply samples. |
| `rollback.json` | E-Swap-5 and candidate rollback sessions | `canonical_runner.py` | Request-indexed compile, instantiate, signal, and rollback durations plus exact attempt/success counts and lossless sequence evidence. It contains no successful-v2 sink transition. |
| `post-rollback-continuity.json` | E-Swap-5 | `canonical_runner.py` | Explicit output observed after the final rollback, with final request identity, bounded observation interval, message count, interval-metrics provenance, and lossless full-run sequence evidence. |

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
- `run-experiment.sh` owns
  `metadata.json`, `config.toml`, `stdout.log`, the E2E `throughput.csv`, and E-Perf-9
  `startup-preparation.json`.
- `eval/scripts/lib/pi_telemetry.py` owns `pi-telemetry.csv`, `pmic-rails.csv`
  and `power-boundary.json`; `eval/scripts/lib/proc_telemetry.py` owns
  `cpu-cores.csv`, `host-sched.csv`, `sut-processes.csv` and
  `host-sidecar.json`. Both are sidecar processes the canonical runner and
  `run-experiment.sh --canonical` start before the runtime and stop after it
  exits; neither reads or changes anything the runtime measures.

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

The runtime adopted this async P2 path. The A/B tooling that produced
`p2-decision.json` has been retired.

### Final amended contract

The `final_campaign` object in `eval/canonical-matrix.json` is the executable source of truth. It fixes seed 1729, explicit fuel-plus-epoch metering, eKuiper concurrency 1, the five-rate common capacity grid, 2,165 schedule records, and 1,953 executed or static measurement leaves. Every final experiment has `thesis_evidence=true`.

Final E-Perf-5 uses explicit transform fuel and epoch protection. Its transform-only pipeline records `filter = null` and `router = null` because those node categories are absent; this is a declared matrix exception, not an unmetered WAFER run.

Final E-Perf-10 requires `capacity-run.json`, `publisher-summary.json`, `subscriber-metadata.json`, `latency.hdr`, `throughput.csv`, `sequence.csv`, `resource-usage.csv`, and `process-audit.json`. `capacity-run.json` uses this counter identity:

```text
intended = rejected + enqueued
enqueued = acked
acked = received_unique + downstream_lost
received_events = received_unique + duplicates
total_undelivered = rejected + downstream_lost
```

It must contain no mandatory per-message traces. `check_capacity_run_result` rejects missing fields, inconsistent counters, non-final evidence labels, or trace mode.

Final E-Swap-3 requires `publisher-summary.json`, `subscriber-metadata.json`, 200 contiguous 100 ms buckets in `throughput-buckets.json`, 400 contiguous 10 ms buckets and 40 nested 100 ms parent buckets in `throughput-buckets-10ms.json`, `disruption-timeline.json`, and `disruption-analysis.json`. The 100 ms series stays aligned to the actual action-start `t0`, and the parent buckets equal the matching canonical event-window slice. `disruption-timeline.json` records the actual action start immediately before control issuance, the actual acknowledged/readiness wall-clock end, independent monotonic action duration, the scheduled measured t=60 target, actual event offset, signed alignment error, and a fixed 10 ms tolerance. `swap_timeline.json` is not a final E-Swap-3 artifact and must not remain in the admitted raw leaf. The leaf fails when the actual action start misses the scheduled target by more than 10 ms.

Final E-Swap-4 uses a source-driven 1,000→2,000→1,000 msg/s schedule over measured intervals `[0,55)`, `[55,65)`, and `[65,120)` after 30,000 warmup messages. Each independent run contains exactly 130,000 measured messages and one swap scheduled at measured t=60. `throughput-buckets.json` contains 1,200 contiguous source-origin 100 ms primary buckets over `[0,120s)` plus a separate 100-bucket drain series over `[120s,130s)`. Full-run counters reconcile primary, drain, and O(1) after-drain evidence; any after-drain receive or right-censored drain rejects the run. `swap-actual-t0.json` declares the single source-origin actual-`t0` receipt, and it must reconcile with `swap_requests.json`, `throughput-buckets-10ms.json`, `throughput-buckets.json`, and `burst-timeline.json` on source origin, scheduled t=60, actual request timestamp, alignment error, and the fixed 10 ms tolerance. `burst-timeline.json` records source and actual swap boundaries, monotonic source-completion offset, intended/emitted/received phase populations, sequence loss/duplication, primary/drain completion evidence, the sink-observed gap, and internal swap phases. The semantic verifier rejects wrong clocks, origins, phase rates or populations, zero or multiple swaps, a missing or inconsistent actual-`t0` receipt, a swap outside the burst, a non-centered scheduled swap, bucket gaps, clamped/misclassified tail evidence, completion at or after 130 s, population mismatches, loss, or duplication. Batch analysis admits exactly one event from each of 30 distinct runs before computing the across-run p95 and bootstrap median interval, and separately reports runs with drain arrivals and the maximum drain offset. Missing required files fail through the matrix contract; malformed files fail through experiment-specific semantic checks.

Final E-Swap-1, E-Swap-2, E-Swap-5, and E-Swap-6 use one complete process run as the independent unit and their 50 swap or rollback events as nested observations; E-Swap-4 uses 30 independent runs with one nested swap each. E-Swap-2 and E-Swap-6 are alias views of E-Swap-1 and contribute zero additional independent N.

Final E-Swap-5 contains one independent process run with 50 nested failed-replacement events. Each request must return `rolled_back`; `rollback.json` reconciles all request-indexed internal phases and the lossless full-run sequence. `post-rollback-continuity.json` proves output after the final rollback and explicitly records that no successful v2 transition was observed. A retained or synthesized `swap_timeline.json`, missing continuity, nonzero loss or duplication, or any request/rollback mismatch rejects the leaf.

Final E-Density-1 is a static source-bound measurement. The canonical runner invokes `eval/scripts/collect-binary-sizes.sh` instead of `run-experiment.sh`, requires one `binary-sizes.csv` row for every non-comment entry in `eval/scripts/binary-sizes.index`, and records Pi host, source provenance, governor, throttling, telemetry, and measurement-window evidence. Its metadata uses `system = "static"` and `exit_codes.collector = 0`; runtime/Wasmtime provenance is intentionally inapplicable because no WAFER runtime process executes.

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
transition. Each event's label comes from the `compile_cache` value the runtime
returns in its swap response: `compiled` is labeled `first-use-aot`, and
`memory_hit` or `disk_hit` is labeled `cached`. Event 0 in each run must be
`first-use-aot` and events 1–49 must be `cached`; any other outcome fails the
run. Event-level rows stay nested
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
canonical eKuiper 2.1.5 config, load-generator profile, QoS, operator
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

Campaign evidence uses one physical exFAT volume labeled
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
verification; every error count must be zero. The qualification tooling does
not format, relabel, mount, unmount, copy, or delete storage.

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
| `isolated_cpus` | `/sys/devices/system/cpu/isolated` | `1-3` | `1-3` | `1-3` |
| `throttled` | `pi_telemetry.py` backend | `0x0` | `0x0` | `0x0` |
| `online_cpus` | `/sys/devices/system/cpu/online` | any | `0-3` | any |
| `power_mode` | `nvpmodel -q` | any | `25W` | any |
| `smt` | `/sys/devices/system/cpu/smt/control` | any | any | `off`, `forceoff` or `notsupported` |
| `turbo` | `intel_pstate/no_turbo` or `cpufreq/boost` | any | any | `off` |

The SUT runs on CPUs `1-3` and the load generator, broker and samplers on CPU
`0` on every host. `memory_total_kib` (`/proc/meminfo`) and
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
