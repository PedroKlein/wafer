# Result-directory contract

Every WAFER experiment run produces a single self-contained directory under
`eval/results/<experiment-id>/<host-tag>-<UTC-timestamp>/`. This document
is the authoritative manifest — every downstream analysis notebook, every
readiness-matrix row, and every canonical-runs comparison assumes this
shape.

## Path convention

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
| `throughput.csv` | Same as `latency.hdr` | `BenchSink` or `wafer-loadgen subscribe` | Periodic throughput samples: `timestamp_ns,messages_per_sec,total_messages`. |
| `sequence.csv` | Loadgen with sequence tracking, or `BenchSink.track_sequences = true` (E-Perf-1..3, E-Perf-8, E-Perf-10, E-Backpressure, E-Swap-*) | `wafer-loadgen subscribe` / `BenchSink` | Producer-specific sequence accounting. `BenchSink` writes one summary row (`total_expected,total_received,gap_ranges,gap_msgs,duplicates_count`); for `BenchSource` input, its `bench.warmup` marker makes this the exact post-warmup population. `wafer-loadgen subscribe` writes long-form gap/duplicate events (`event_type,seq_start,seq_end,count`), with zero data rows when lossless; its totals live in `subscriber-metadata.json`. |
| `memory.csv` | E-Perf-6, E-Perf-7, E-Perf-8, E-Backpressure (and any run with `WAFER_BENCH_OUTPUT_DIR` set) | `wafer-runtime` via `MemoryRecorder::sample_loop` (1 Hz, cross-platform via `memory-stats` crate). Flush on graceful shutdown. | 1 Hz process-RSS timeline of `wafer-runtime`: `elapsed_ms,rss_bytes`. Runtime-owned since A19 closure (thesis-hardening T4). Legacy harness `ps` polling removed. |
| `per_node_metrics.csv` | All experiments (emitted on graceful shutdown when `WAFER_BENCH_OUTPUT_DIR` set) | `wafer-runtime` via `PipelineOrchestrator::export_per_node_metrics` | One row per pipeline node: `node_id,messages_in,messages_out,traps_total,error_state_seconds,recovery_count`. Runtime-owned since A19 closure (thesis-hardening T4). |
| `queue-depth.csv` | E-Backpressure, when `WAFER_QUEUE_DEPTH_OUTPUT` is set | `wafer-runtime` via `QueueDepthRecorder` | Bounded internal Tokio queue samples at 10 ms intervals: `elapsed_ns,queue,depth,capacity,accepted,dequeued,processed`. Counters are internal pipeline observations; broker backlog is excluded. Collection is capped at 131,072 rows and reports truncation in the runtime log. |
| `backpressure.json` | E-Backpressure | `canonical_runner.py` | Queue threshold crossing and recovery, offered/accepted/processed/drained rates in messages per second, loss/duplication accounting, and peak-RSS bound. A high offered rate without measured occupancy is classified `not-saturated`. |
| `recovery.csv` | E-Iso-8 and any run with recovery events | `wafer-runtime` via `PipelineOrchestrator::export_per_node_metrics` | Exact recovery samples: `node_id,sample_index,duration_ns`. Retains nanosecond precision for trap-to-running percentiles. |
| `measurement-window.json` | Canonical Pi 5 runs | `BenchSink` for in-process runs with post-warmup output; canonical harness for external MQTT subscribers and zero-output containment runs | Exact `started_ns` and `finished_ns` bounds for excluding warmup and teardown from PMIC energy integration. |
| `pi-telemetry.csv` | Canonical Pi 5 runs | `eval/scripts/lib/pi_telemetry.py` | Timestamped temperature, CPU frequency, governor, throttling state, and summed PMIC internal-rail proxy watts. |
| `pmic-rails.csv` | Canonical Pi 5 runs | `eval/scripts/lib/pi_telemetry.py` | Long-form named PMIC rail voltage/current/power samples. Rails without both voltage and current are not included. |
| `power-boundary.json` | Canonical Pi 5 runs | `eval/scripts/lib/pi_telemetry.py` | Declares that telemetry is an internal-rail proxy, not total USB-C input power, and records excluded consumers and the source limitation. |
| `published.csv`, `received.csv` | Historical focused E-Perf-10 diagnostics only | `wafer-loadgen` opt-in tracing | Raw publisher/subscriber timestamp and sequence samples retained for v11-v17 compatibility. Final E-Perf-10 rejects these mandatory traces and uses bounded summaries. |
| `publisher-summary.json` | Final E-Perf-10 | `wafer-loadgen publish` | Bounded `intended`, `rejected`, and `enqueued` counters plus measured duration. `intended = rejected + enqueued` is mandatory. |
| `subscriber-metadata.json` | Final E-Perf-10 and external-MQTT experiments | `wafer-loadgen subscribe` | Bounded receive, duplicate, ignored-warmup, unexpected-sequence, parse, latency, and sequence totals. The HDR count and received-event population must match. |
| `capacity-run.json` | Final E-Perf-10 | `canonical_runner.py` | Trace-free run summary containing intended/rejected/enqueued/received/lost/duplicate counts, offered and achieved rates, loss, p50/p95/p99, CPU, RSS, thermal state, process/config receipts, controlled factors, and `thesis_evidence=true`. |
| `resource-usage.csv` | E-Perf-10 | `canonical_runner.py` | One-second SUT samples: wall-clock timestamp, aggregate process CPU ticks, RSS bytes, and process count. MQTT loopback records the explicit no-SUT zero baseline. |
| `process-audit.json` | E-Perf-10 | `canonical_runner.py` | Active SUT PID, process affinity, exclusivity, and allowed CPU set captured before measurement. |
| `rate-sweep.json` | Historical focused E-Perf-10 diagnostics only | `canonical_runner.py` | Legacy trace-backed run summary, always `thesis_evidence=false`. It remains readable but is not accepted as a final-capacity leaf. |
| `throughput-buckets.json` | Final E-Swap-3 and E-Swap-4 | `wafer-loadgen subscribe` / `canonical_runner.py` | Contiguous 100 ms output-rate buckets with unique/event/duplicate counts. E-Swap-3 uses bounded in-memory receive timestamps, then bins exactly -10 s through +10 s around the actual disruption event. Unix-epoch values are labeled for cross-process alignment only. |
| `disruption-timeline.json` | Final E-Swap-3 | `canonical_runner.py` | Strategy, actual Unix-epoch action start/end for cross-process alignment, monotonic scheduling/duration labels, the scheduled t=60 target, measured offset, signed alignment error, and a 10 ms maximum alignment tolerance. The wall-clock action end is retained independently rather than synthesized from the monotonic duration. |
| `burst-timeline.json` | Final E-Swap-4 | `BenchSource` / `canonical_runner.py` | One 1,000 to 2,000 to 1,000 msg/s burst with boundaries at measured seconds 55 and 65 and exactly one successful swap at second 60. |
| `startup-preparation.json` | E-Perf-9 | `run-experiment.sh` | Filesystem-cache condition and preparation action completed before the timed runtime process starts. |
| `startup.json` | E-Perf-9 | `wafer-runtime` | Monotonic process/config, component load/compile, instantiation, pipeline setup, and first-process durations; total startup duration; exactly-one-message proof; plugin SHA-256; and explicit compiled-component cache state. |
| `branch-a/`, `branch-b/` | E-Iso-7 | `BenchSink` | Independent post-warmup latency histogram, throughput series, sequence accounting, and measurement window for each branch. Each branch has its own `BenchSource`; root-level fan-out/fan-in measurements are forbidden for branch-impact analysis. |
| `branch-isolation.json` | E-Iso-7 | `canonical_runner.py` | Branch-local source identity, configured post-warmup target count, actually offered/received post-warmup counts, target shortfall, throughput samples, latency percentiles, measurement boundaries, and explicit units. |
| `branch-isolation-summary.json` | E-Iso-7 batch ledger | `canonical_runner.py` | Separate branch-A throughput-drop and p95-latency-increase rows for panic and epoch-loop attacks, including run counts and units. |
| `swap_requests.json` | E-Swap-1, E-Swap-2, E-Swap-4, E-Swap-6 | `canonical_runner.py` HTTP client | One record per API request with wall-clock request boundaries, monotonic `request_duration_ns`, HTTP status, and the runtime's internal `compile_ns`, `instantiate_ns`, `signal_ns`, `ack_ns`, and `convergence_ns` phases. |
| `swap_timeline.json` | E-Swap-1..6 (any config that exercises at least one hot-swap) | `BenchSink` | Sink-observed plugin-version transitions. Each `pause_ns` is an output interarrival gap and is not an internal swap duration. |
| `hotswap-analysis.json` | E-Swap-1, E-Swap-2, E-Swap-4, E-Swap-6 | `canonical_runner.py` | Index-matched API and sink observations with explicit `*_ns` names: internal phases, `http_total_ns`, and `sink_observed_output_gap_ns`. Includes the unique measurement source leaf so shared E-Swap-2/6 views do not multiply samples. |
| `summary.json` | E-Val-1 only | `run-e-val-1-shakedown.sh` | Gate-pass summary across runs (p99 range, honesty-window check). Bespoke to the honesty-gate methodology; not consumed by canonical analysis. |

### Ownership summary

- `wafer-runtime` owns `runtime-provenance.json`, E-Perf-9 `startup.json`,
  `latency.hdr` (via `BenchSink`), `throughput.csv` (via `BenchSink`), `sequence.csv`
  (via `BenchSink` when `track_sequences=true`), sink-observed `swap_timeline.json`,
  `memory.csv` (via `MemoryRecorder`), `queue-depth.csv` (via `QueueDepthRecorder`),
  `recovery.csv` (exact recovery samples), and `per_node_metrics.csv`
  (via `PipelineOrchestrator::export_per_node_metrics`).
- `wafer-loadgen subscribe` owns `subscriber-metadata.json`,
  `latency.hdr` (E2E path), `sequence.csv`, bounded `throughput-buckets.json`,
  and historical opt-in `received.csv` traces.
- `wafer-loadgen publish` owns `publisher-summary.json` and historical opt-in
  `published.csv` traces.
- `canonical_runner.py` owns final E-Perf-10 `capacity-run.json`,
  `resource-usage.csv`, and `process-audit.json`; historical diagnostics retain
  `rate-sweep.json`. It also owns final E-Swap `disruption-timeline.json` and
  `burst-timeline.json`, E-Backpressure `backpressure.json`,
  E-Swap `swap_requests.json` and `hotswap-analysis.json`, plus E-Iso-7
  `branch-isolation.json` and the batch-level branch-A impact summary.
- `run-experiment.sh` and the per-experiment shakedown scripts own
  `metadata.json`, `config.toml`, `stdout.log`, and E-Perf-9
  `startup-preparation.json`.

### Final amended contract

The `final_campaign` object in `eval/canonical-matrix.json` is the executable source of truth. It fixes seed 1729, explicit fuel-plus-epoch metering, eKuiper concurrency 1, the five-rate common capacity grid, 2,105 schedule records, and 1,893 executed or static measurement leaves. Every final experiment has `thesis_evidence=true`; diagnostic and focused entries remain in the separate `focused_pilot` object with `thesis_evidence=false`.

Final E-Perf-10 requires `capacity-run.json`, `publisher-summary.json`, `subscriber-metadata.json`, `latency.hdr`, `throughput.csv`, `sequence.csv`, `resource-usage.csv`, and `process-audit.json`. `capacity-run.json` uses this counter identity:

```text
intended = rejected + enqueued
enqueued = received_unique + downstream_lost
received_events = received_unique + duplicates
total_undelivered = rejected + downstream_lost
```

It must contain no mandatory per-message traces. `check_capacity_run_result` rejects missing fields, inconsistent counters, non-final evidence labels, unexpected sequences, or trace mode.

Final E-Swap-3 requires 200 contiguous 100 ms buckets in `throughput-buckets.json` aligned to the actual action-start `t0`. `disruption-timeline.json` records the actual action start immediately before control issuance, the actual acknowledged/readiness wall-clock end, independent monotonic action duration, the scheduled measured t=60 target, actual event offset, signed alignment error, and a fixed 10 ms tolerance. The leaf fails when the actual action start misses the scheduled target by more than 10 ms. Final E-Swap-4 requires `burst-timeline.json` with rates 1,000/2,000/1,000, offsets 55/60/65 seconds, and one successful swap. Missing required files fail through the matrix contract; malformed files fail through experiment-specific semantic checks.

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
