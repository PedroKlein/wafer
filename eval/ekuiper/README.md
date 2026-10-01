# Native eKuiper comparator

The evaluation hosts run eKuiper 2.1.5 directly from its official Linux Debian package (`install-native.sh` picks the ARM64 or amd64 build). No evaluation path uses Docker.

## Install on the Pi

```sh
./eval/ekuiper/install-native.sh --dry-run
./eval/ekuiper/install-native.sh
```

The installer downloads the pinned package and its published SHA256 file and checks both against the SHA256 pinned in the script for that architecture. It then installs the package, installs the canonical MQTT source configuration, writes an installation receipt, assigns the service to CPUs 1–3, and restarts the service so an upgraded host runs the new package.

## Register Pipeline A

```sh
./eval/ekuiper/seed-pipeline-a.sh --dry-run
./eval/ekuiper/seed-pipeline-a.sh
```

Overrides are available when needed:

```sh
EKUIPER_URL=http://127.0.0.1:9081 \
EKUIPER_BROKER_URL=tcp://127.0.0.1:1883 \
  ./eval/ekuiper/seed-pipeline-a.sh
```

Pipeline A implements the comparator path:

```text
MQTT wafer/telemetry → JSON decode → 50 ≤ temperature ≤ 99999 → MQTT wafer/telemetry/hot
```

## Diagnostic tail profiling

The optional `e-compare-ekuiper-profile` batch is isolated from canonical comparisons. It schedules five profiled and five unprofiled-control host runs at each of 1,000, 4,000, and 8,000 messages per second. Paired runs share the same rate, run index, Pipeline A config, QoS 1 transport, operator concurrency 1, 30-second warmup, and 60-second measurement.

Only the profiled arm starts the bounded one-second external `/proc` sampler. If those process files are unavailable, the run retains latency, throughput, interval, and host telemetry while marking process metrics unavailable. The profiled arm also starts eKuiper with `GODEBUG=gctrace=1` through the runtime drop-in `gctrace-drop-in.conf`, which the runner removes when eKuiper stops, and keeps the Go GC lines from the `kuiper.service` journal in `ekuiper-gctrace.log`. Missing GC lines are recorded as unavailable, not inferred. Results describe run-level associations and profiler overhead; they do not establish GC causality and are never pooled with E-Perf-1, E-Perf-10, or prior diagnostics. See `docs/benchmarks/ekuiper-profile-diagnostic.md`.

## Smoke test

```sh
./eval/ekuiper/smoke-test.sh --dry-run
./eval/ekuiper/smoke-test.sh
```

The smoke test publishes below-range, boundary, and above-range records. It requires exactly the boundary record with the full five-field schema and unchanged `ts`/`seq`. It uses the native `mosquitto_pub` and `mosquitto_sub` clients.

## Verify

```sh
systemctl is-active mosquitto kuiper
curl -fsS http://127.0.0.1:9081/
taskset -pc "$(systemctl show -p MainPID --value kuiper)"
./eval/ekuiper/test-native-scripts.sh
```

## Files

- `install-native.sh` — pinned package download, checksum verification, installation, and systemd affinity.
- `mqtt-source-default.yaml` — canonical MQTT source settings installed on the Pi.
- `gctrace-drop-in.conf` — systemd drop-in that the profiled tail-profiling runs install under `/run` to set `GODEBUG=gctrace=1`.
- `seed-pipeline-a.sh` — idempotent REST registration.
- `pipeline-a-rule.sql` — human-readable rule definition.
- `smoke-test.sh` — native pass/drop behavior check.
