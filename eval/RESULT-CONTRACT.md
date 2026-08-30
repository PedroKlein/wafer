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
        ├── sequence.csv                ← where applicable
        └── swap_timeline.json          ← where applicable
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
| `latency.hdr` | Every experiment with a BenchSink or `wafer-loadgen subscribe` (E-Val-1, E-Perf-1..9, E-Backpressure, E-Iso-*, E-Swap-*) | `BenchSink` (in-process) or `wafer-loadgen subscribe` (E2E) | HdrHistogram V2 latency in nanoseconds. In-process: per-hop source-to-sink. E2E: MQTT publish → MQTT consume with intended-publish timestamp (avoids coordinated omission). |
| `throughput.csv` | Same as `latency.hdr` | `BenchSink` or `wafer-loadgen subscribe` | Periodic throughput samples: `timestamp_ns,messages_per_sec,total_messages`. |
| `sequence.csv` | Loadgen with sequence tracking, or `BenchSink.track_sequences = true` (E-Perf-1..3, E-Perf-8, E-Backpressure, E-Swap-*) | `wafer-loadgen subscribe` / `BenchSink` | Gap and duplicate accounting: `total_expected,total_received,gaps_count,duplicates_count,first_seq,last_seq`. Zero rows when there were no gaps/dups. |
| `memory.csv` | E-Perf-6, E-Perf-7, E-Perf-8, E-Backpressure (and any run with `WAFER_BENCH_OUTPUT_DIR` set) | `wafer-runtime` via `MemoryRecorder::sample_loop` (1 Hz, cross-platform via `memory-stats` crate). Flush on graceful shutdown. | 1 Hz process-RSS timeline of `wafer-runtime`: `elapsed_ms,rss_bytes`. Runtime-owned since A19 closure (thesis-hardening T4). Legacy harness `ps` polling removed. |
| `per_node_metrics.csv` | All experiments (emitted on graceful shutdown when `WAFER_BENCH_OUTPUT_DIR` set) | `wafer-runtime` via `PipelineOrchestrator::export_per_node_metrics` | One row per pipeline node: `node_id,messages_in,messages_out,traps_total,error_state_seconds,recovery_count`. Runtime-owned since A19 closure (thesis-hardening T4). |
| `recovery.csv` | E-Iso-8 and any run with recovery events | `wafer-runtime` via `PipelineOrchestrator::export_per_node_metrics` | Exact recovery samples: `node_id,sample_index,duration_ns`. Retains nanosecond precision for trap-to-running percentiles. |
| `pi-telemetry.csv` | Canonical Pi 5 runs | `eval/scripts/lib/pi_telemetry.py` | Timestamped temperature, CPU frequency, governor, throttling state, and summed PMIC internal-rail proxy watts. |
| `pmic-rails.csv` | Canonical Pi 5 runs | `eval/scripts/lib/pi_telemetry.py` | Long-form named PMIC rail voltage/current/power samples. Rails without both voltage and current are not included. |
| `power-boundary.json` | Canonical Pi 5 runs | `eval/scripts/lib/pi_telemetry.py` | Declares that telemetry is an internal-rail proxy, not total USB-C input power, and records excluded consumers and the source limitation. |
| `swap_timeline.json` | E-Swap-1..6 (any config that exercises at least one hot-swap) | `wafer-runtime` orchestrator's `SwapTimeline` emitter | Per-swap phase decomposition: `{node_id, request_id, compile_ns, instantiate_ns, signal_ns, ack_ns, first_v2_ns, convergence_ns}`. |
| `summary.json` | E-Val-1 only | `run-e-val-1-shakedown.sh` | Gate-pass summary across runs (p99 range, honesty-window check). Bespoke to the honesty-gate methodology; not consumed by canonical analysis. |

### Ownership summary

- `wafer-runtime` owns `runtime-provenance.json`, `latency.hdr` (via
  `BenchSink`), `throughput.csv` (via `BenchSink`), `sequence.csv`
  (via `BenchSink` when `track_sequences=true`), `swap_timeline.json`,
  `memory.csv` (via `MemoryRecorder`), `recovery.csv` (exact recovery samples),
  and `per_node_metrics.csv` (via `PipelineOrchestrator::export_per_node_metrics`).
- `wafer-loadgen subscribe` owns `subscriber-metadata.json`,
  `latency.hdr` (E2E path), `sequence.csv`.
- `run-experiment.sh` and the per-experiment shakedown scripts own
  `metadata.json`, `config.toml`, and `stdout.log`.

## `metadata.json` schema

`metadata.json` merges two sources: **run-experiment.sh** stamps run
metadata and per-invocation context (git_sha, timestamps, loadgen /
mosquitto blocks, exit codes) while **wafer-runtime** emits a
`runtime-provenance.json` sidecar with the fields only the runtime can
produce authoritatively (`wasmtime_version` from the resolved lockfile,
`config_sha256`, `wafer_plugin_hashes` sharing the P0.12 hot-swap guard
cache, `wafer_runtime_sha256`, `rustc_version`, `kernel`). `_write_metadata`
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
