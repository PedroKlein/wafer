# eKuiper comparator setup

**Version pin.** Native eKuiper `2.1.0` Linux ARM64 package. The Raspberry Pi 5 canonical evaluation does not use Docker. The old Compose file remains solely for reproducing historical macOS shakedowns.

**Role.** eKuiper is the reference stream-processing engine for E-Perf-1 throughput, E-Perf-2 latency, and E-Swap-3 rule-restart disruption. It is treated as a black box driven identically to WAFER through `wafer-loadgen publish` and `wafer-loadgen subscribe`.

## Raspberry Pi 5 setup

Install the pinned package and verify its published checksum:

```sh
./eval/ekuiper/install-native.sh --dry-run
./eval/ekuiper/install-native.sh
systemctl is-active kuiper
```

The package installs the daemon under `/usr/lib/kuiper`, configuration under `/etc/kuiper`, mutable data under `/var/lib/kuiper`, and logs under `/var/log/kuiper`. The systemd override assigns eKuiper to CPUs 1–3 and sets the default MQTT broker to `tcp://127.0.0.1:1883`.

Register and test Pipeline A:

```sh
./eval/ekuiper/seed-pipeline-a.sh
./eval/ekuiper/smoke-test.sh
```

## Historical macOS setup

Historical shakedowns used `eval/ekuiper/docker-compose.yml` because eKuiper needed a Linux guest on macOS. Those measurements remain informational and are not mixed with Pi 5 results.

## Pipeline A

Pipeline A mirrors RFC-008 Decision 6:

```text
MQTT wafer/telemetry → JSON decode → temperature > 50 → MQTT wafer/telemetry/hot
```

`seed-pipeline-a.sh` creates the `wafer_telemetry` stream and `pipeline_a` rule through the REST API on port 9081. Its broker is configurable through `EKUIPER_BROKER_URL` and defaults to native Mosquitto at `tcp://127.0.0.1:1883`.

The MQTT sink sets `sendSingle: true`, giving one output per matching input as WAFER does. `smoke-test.sh` proves both directions of the filter: temperature 30 must be absent and temperature 80 must be present.

## Comparator procedure

For every paired WAFER/eKuiper run:

1. Confirm Mosquitto is active and assigned to CPU 0.
2. Assign the active SUT to CPUs 1–3. Run only one SUT at a time.
3. Seed eKuiper before its run and verify `pipeline_a` reports `running`.
4. Use the same `wafer-loadgen` profile, payload, topics, warmup, and measurement window for WAFER and eKuiper.
5. Use the common subscriber to write `latency.hdr`, `throughput.csv`, and sequence accounting.
6. Record the eKuiper package version and SHA256 in run metadata.

## Known limitations

- Native Pi 5 results are not directly comparable to the old Docker Desktop macOS shakedowns. The latter include a Linux VM and bridge-network overhead.
- Raspberry Pi 5 results are not numerically interchangeable with Raspberry Pi 4 results from prior literature. Report absolute values and WAFER/native/eKuiper ratios.
- eKuiper decodes JSON per message while WAFER's minimal pass-through pipeline moves opaque bytes. Pipeline A uses equivalent threshold-filter semantics for the primary engine comparison.
- REST port 9081 is distinct from WAFER's port 9090 and Mosquitto's port 1883.

## Related files

- `eval/ekuiper/install-native.sh`
- `eval/ekuiper/seed-pipeline-a.sh`
- `eval/ekuiper/smoke-test.sh`
- `eval/ekuiper/pipeline-a-rule.sql`
- `docs/status/rpi5-canonical-transition.md`
- `docs/rfcs/RFC-008-evaluation-harness.md`
