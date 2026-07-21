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

- `shakedown-macos` — MacBook laptop shakedown (this plan).
- `rpi4` — Raspberry Pi 4 canonical run (canonical-runs plan).
- `jetson` — Jetson Orin Nano canonical run (canonical-runs plan).
- `x86` — x86 workstation cross-architecture validation (canonical-runs plan).

**Timestamp format**: `YYYY-MM-DDTHH-MM-SSZ` — UTC, colons replaced with
hyphens so the path is `mv`-safe on every filesystem.

## Manifest

### Always-present (every experiment)

| File | Producer | Description |
| --- | --- | --- |
| `config.toml` | `run-experiment.sh` copies the input file | Verbatim copy of the pipeline config used for this run. |
| `metadata.json` | `run-experiment.sh` | Run-provenance JSON — see [metadata.json schema](#metadatajson-schema) below. |
| `stdout.log` | `run-experiment.sh` tees runtime output | Complete stdout+stderr of `wafer-runtime` including trap traces and hot-swap events. |
| `latency.hdr` | Either `BenchSink` (in-process) or `wafer-loadgen subscribe` (E2E) | HdrHistogram V2-serialised latency in nanoseconds. In-process runs measure per-hop end-to-end from source enqueue to sink dequeue; E2E runs measure MQTT publish → MQTT consume with the intended-publish timestamp stamped in payload (avoids coordinated omission). |
| `throughput.csv` | `BenchSink` or `wafer-loadgen subscribe` | Periodic throughput samples: `timestamp_ns,messages_per_sec,total_messages`. |
| `memory.csv` | `run-experiment.sh` external sampler | 1 Hz process-RSS timeline of `wafer-runtime`: `timestamp_ns,rss_bytes,vsz_bytes`. macOS uses `ps -o rss=,vsz= -p <PID>` (KiB × 1024); Linux is expected to prefer `/proc/self/statm` when the canonical-runs plan wires it up. |
| `per_node_metrics.csv` | `run-experiment.sh` scrapes Prometheus `/metrics` at shutdown | One row per pipeline node: `node_id,messages_in,messages_out,traps_total,error_state_seconds,recovery_count`. Columns are best-effort — if the runtime's HTTP server was disabled, this file exists but contains only the header + a `# metrics endpoint unreachable` comment. |

### Conditional (produced by specific experiment shapes)

| File | Present when | Producer | Description |
| --- | --- | --- | --- |
| `sequence.csv` | Loadgen subscribe ran with sequence tracking, or `BenchSink` had `track_sequences = true`. | `wafer-loadgen subscribe` / `BenchSink` | Gap and duplicate accounting: `total_expected,total_received,gaps_count,duplicates_count,first_seq,last_seq`. Zero rows when there were no gaps/dups. |
| `swap_timeline.json` | Config exercised at least one hot-swap (RFC-008 E-Swap-1..6). | `wafer-runtime` orchestrator's `SwapTimeline` serialiser (planned by P5.1). | Per-swap phase decomposition: `{node_id, request_id, compile_ns, instantiate_ns, signal_ns, ack_ns, first_v2_ns, convergence_ns}`. |

## `metadata.json` schema

```json
{
  "experiment": "e-perf-4",
  "host_tag": "shakedown-macos",
  "generated_at": "2026-07-21T14:30:15Z",
  "git_sha": "38252494da5f39ce1e02e5a642fae85d0791a527",
  "hostname": "pkleins-mbp.local",
  "kernel": "24.5.0",
  "arch": "arm64",
  "os": "darwin",
  "rustc": "rustc 1.85.0 (unknown)",
  "wafer_runtime_version": "0.1.0",
  "wafer_loadgen_version": "0.1.0",
  "wasmtime_version": "38.0.2",
  "config_path": "eval/configs/pipeline-c-passthrough.toml",
  "config_sha256": "…",
  "started_at": "2026-07-21T14:30:15Z",
  "finished_at": "2026-07-21T14:32:47Z",
  "duration_seconds": 152,
  "loadgen": {
    "profile_path": "eval/loadgen/generic-1kb.toml",
    "publish_command": ["…"],
    "subscribe_command": ["…"]
  },
  "mosquitto": {
    "container_id": "docker-container-id",
    "image": "eclipse-mosquitto:2.0.18"
  },
  "exit_codes": {
    "wafer_runtime": 0,
    "loadgen_publish": 0,
    "loadgen_subscribe": 0
  }
}
```

Fields are omitted when not applicable:

- `loadgen` is absent when the config is BenchSource/BenchSink only (no MQTT).
- `mosquitto` is absent when the harness reuses an already-running broker
  (see the `WAFER_HARNESS_MQTT` environment variable in
  [`eval/scripts/run-experiment.sh`](./scripts/run-experiment.sh)).

`config_sha256` is the digest of `config.toml` — reproducibility hinge for
notebook cross-references.

## Idempotency and safety

- **Safe to re-run**: `run-experiment.sh` never overwrites an existing
  timestamped directory. Two runs at the same second (rare) get a
  suffixed second timestamp.
- **Never `sudo`** on the dev machine. Only `setup-rpi.sh` and
  `setup-jetson.sh` (canonical-runs plan) require privileged setup.
- **Never `rm -rf`** on a result directory. Cleanup is the operator's job.
- **Docker containers created by the harness are labelled**
  `wafer-harness=1`; `docker ps -f label=wafer-harness=1` lists everything
  the harness owns. Anything without that label is left alone.
- **Broker discovery order**: `--broker` flag → `WAFER_HARNESS_MQTT` env
  var → auto-start eclipse-mosquitto in Docker. In the auto path the
  harness stops the container it created; in the pre-existing-broker path
  it leaves the broker running.

## Reproducibility hinge

Every artefact carries the run's `metadata.json.config_sha256` as its
provenance root. Notebooks that plot a result MUST read `metadata.json`
before reading the artefact, and MUST assert the SHA256 against the
notebook cell that produced the config revision. This is how P7.1
achieves reproducibility across shakedown and canonical runs.
