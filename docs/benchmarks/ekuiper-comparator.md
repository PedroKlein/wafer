# eKuiper comparator setup

**Version pin.** Native eKuiper `2.1.0` Linux ARM64 package. The Raspberry Pi 5 canonical evaluation does not use Docker. The old Compose file remains solely for reproducing historical macOS shakedowns.

**Role.** eKuiper is the reference stream-processing engine for E-Perf-1 target-load delivery/latency, E-Perf-10 gateway capacity, and E-Swap-3 rule-restart disruption. It is treated as a black box driven identically to WAFER through `wafer-loadgen publish` and `wafer-loadgen subscribe`.

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
MQTT source -> threshold filter -> MQTT sink
```

The threshold filter decodes the telemetry record and applies `50 <= temperature <= 99999`; JSON decode is not a separate Pipeline A stage.

`seed-pipeline-a.sh` creates the `wafer_telemetry` stream and `pipeline_a` rule through the REST API on port 9081. Its broker is configurable through `EKUIPER_BROKER_URL` and defaults to native Mosquitto at `tcp://127.0.0.1:1883`.

The MQTT sink explicitly sets MQTT 3.1.1, QoS 1, `retained: false`, and `sendSingle: true`. Rule operator concurrency is explicitly frozen at 1. The stream projects the same five telemetry fields used by WAFER and native Rust. `smoke-test.sh` proves the inclusive lower bound, exclusive out-of-range records, field preservation, and unchanged `ts`/`seq` values.

## Comparator procedure

For every paired WAFER/eKuiper run:

1. Confirm Mosquitto is active and assigned to CPU 0.
2. Assign the active SUT to CPUs 1–3. Run only one SUT at a time.
3. Seed eKuiper before its run and verify `pipeline_a` reports `running`.
4. Use the same `wafer-loadgen` profile, payload, topics, warmup, and measurement window for WAFER and eKuiper.
5. Use the common subscriber to write `latency.hdr`, `throughput.csv`, and sequence accounting.
6. Record the eKuiper package version and SHA256 in run metadata.
7. Save `ekuiper-audit.json` before warmup. It contains the active rule and stream, effective systemd settings, MQTT source configuration, process tree, per-process `Cpus_allowed_list`, and an explicit concurrent-SUT check.

## Diagnostic finding

The v11 pilot omitted the eKuiper sink `qos` field, selecting QoS 0 and producing an approximately 20 ms periodic release pattern. A matched Raspberry Pi 5 diagnostic reproduced the pattern with sink QoS 0 and removed it with sink QoS 1. The old v11 eKuiper result is therefore not a valid comparator result and must not be pooled with corrected runs.

See [Why the v11 eKuiper latency tail was misleading](ekuiper-tail-diagnostic.md) for the one-variable evidence, corrected small-N results, and claim boundaries.

## Known limitations

- Corrected small-N diagnostics characterize the frozen default-style comparator, not eKuiper's best achievable tuning. A two-block diagnostic compared operator concurrency 1 and 3 at 1,000 messages/second. Both settings had a median p95 of 0.327 ms and similar throughput, CPU, and RSS. Concurrency 3 had a lower median p99, 2.411 ms versus 2.652 ms, but N=2 is insufficient to justify selecting a non-default setting after observation. The canonical comparator therefore freezes concurrency 1 regardless of ranking impact.
- Native Pi 5 results are not directly comparable to the old Docker Desktop macOS shakedowns. The latter include a Linux VM and bridge-network overhead.
- Raspberry Pi 5 results are not numerically interchangeable with Raspberry Pi 4 results from prior literature. Report absolute values and WAFER/native/eKuiper ratios.
- eKuiper and WAFER/native Pipeline A all decode the telemetry field used by the filter. Their output schemas, predicate bounds, topics, and QoS are matched; their internal JSON implementations remain engine-specific.
- REST port 9081 is distinct from WAFER's port 9090 and Mosquitto's port 1883.

## Related files

- `eval/ekuiper/install-native.sh`
- `eval/ekuiper/seed-pipeline-a.sh`
- `eval/ekuiper/smoke-test.sh`
- `eval/ekuiper/pipeline-a-rule.sql`
- `docs/status/rpi5-canonical-transition.md`
- `docs/rfcs/RFC-008-evaluation-harness.md`
