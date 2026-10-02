# eKuiper comparator setup

**Version pin.** Native eKuiper `2.1.5` Linux package (ARM64, or amd64 on the x86 host), the last patch release of the 2.1 line. The evaluation hosts (Raspberry Pi 5, Jetson and x86) run it natively; no evaluation path uses Docker.

**Role.** eKuiper is the reference stream-processing engine for E-Perf-1 target-load delivery/latency, E-Perf-10 gateway capacity, and E-Swap-3 rule-change disruption. It is treated as a black box driven identically to WAFER through `wafer-loadgen publish` and `wafer-loadgen subscribe`.

## Raspberry Pi 5 setup

Install the pinned package. The script picks the `arm64` or `amd64` package
from `uname -m`, so the same pin serves aarch64 and x86_64 hosts. It refuses a
download, or a published checksum, that differs from the SHA256 pinned for that
architecture:

| Package | SHA256 |
|---|---|
| `kuiper-2.1.5-linux-arm64.deb` | `917579fdd8683e047c43d01a694180ae73ca234bd0727e11deb3a902e1556d49` |
| `kuiper-2.1.5-linux-amd64.deb` | `9ec8f68e23f507d0e02e6bb45ff61ed904443c84ef0f6288b00d6b19ea66f12b` |

```sh
./eval/ekuiper/install-native.sh --dry-run
./eval/ekuiper/install-native.sh
systemctl is-active kuiper
```

The package installs the daemon under `/usr/lib/kuiper`, configuration under `/etc/kuiper`, mutable data under `/var/lib/kuiper`, and logs under `/var/log/kuiper`. The systemd override assigns eKuiper to CPUs 1–3 and sets the default MQTT broker to `tcp://127.0.0.1:1883`. The installer restarts the service, so a host that still runs an older package upgrades in place by running it again.

Register and test Pipeline A:

```sh
./eval/ekuiper/seed-pipeline-a.sh
./eval/ekuiper/smoke-test.sh
```

## Version choice

The comparator was first pinned to 2.1.0, the first release of the 2.1 line. Releases 2.1.1 to 2.1.5 stay on that line and are mostly bug fixes, and some of them change the rule update, stop and start paths that E-Swap-3 drives through the REST API. In 2.1.0, when the run of a manually stopped rule exited, it could still set the rule to stopped and clear its topology, which races with an immediate restart. Other fixes change how a rule on a shared stream, such as `wafer_telemetry`, attaches to and detaches from the shared source when it stops, starts, or is updated. One of them, first released in 2.1.2, is why an update keeps the stream's MQTT subscription; see "E-Swap-3 rule changes".

What the harness depends on did not change between 2.1.0 and 2.1.5:

- the packaged `/etc/kuiper/kuiper.yaml`, `/etc/kuiper/mqtt_source.yaml`, systemd unit and maintainer scripts are byte-identical;
- the REST calls the harness makes (create and delete the stream and rule, read the rule, update it, and read its status) return the same status codes and fields;
- both packages are built with Go 1.23.4 and the same Paho MQTT client, and the MQTT sink still waits for each QoS 1 acknowledgement before the next publish.

Three things did change. 2.1.0 opened an idle MQTT control-channel client to the local broker at startup, and 2.1.5 does not. The packaged `connections/connection.yaml` no longer defines sample connections, which Pipeline A never referenced. 2.1.5 also presizes some per-message maps in the projection path, so its numbers are not interchangeable with earlier 2.1.0 diagnostics.

Results describe eKuiper 2.1.5 with this configuration, not a later release line or a tuned deployment. Before each host's final batch, the [runbook](../eval/pi5-experiment-runbook.md#re-check-the-ekuiper-comparator-before-each-batch) re-runs the smoke test and a short series of each E-Swap-3 arm under load on that host.

## Historical macOS setup

Historical shakedowns ran eKuiper under Docker Compose because it needed a Linux guest on macOS. That Compose file has been removed. Those measurements remain informational and are not mixed with Pi 5 results.

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
3. Seed eKuiper before its run. eKuiper keeps rules across restarts, so the seed script deletes `pipeline_a` and the E-Swap-3 replacement rule `pipeline_a_v2` and creates `pipeline_a` again from its own payload. That also undoes the raised bound an `ekuiper-rule-update` run leaves in `pipeline_a`, and the seed fails if it cannot create the rule. The harness waits up to 10 seconds for `pipeline_a` to report `running` with an empty `message` before warmup, checks that it is the only rule, and fails the attempt as infrastructure otherwise.
4. Use the same `wafer-loadgen` profile, payload, topics, warmup, and measurement window for WAFER and eKuiper.
5. Use the common subscriber to write `latency.hdr` and sequence accounting; the harness derives the E2E `throughput.csv` from the subscriber's metadata. Every system's run ends the same way: warmup messages are numbered from the measured count up, the subscriber declares the measured range, and after the publisher exits it has the matrix's 5-second drain grace before it gets SIGINT. Messages that never arrived are the run's loss, recorded and never a reason to reject or retry the run. See "Final amended contract" in `eval/RESULT-CONTRACT.md`.
6. Record the eKuiper package version and SHA256 in run metadata.
7. Save `ekuiper-audit.json` before warmup. It contains the active rule and stream, effective systemd settings, the SHA-256 of `/etc/kuiper/mqtt_source.yaml` and `/etc/kuiper/kuiper.yaml`, process tree, per-process `Cpus_allowed_list`, and an explicit concurrent-SUT check. The active `pipeline_a` must be the rule that `seed-pipeline-a.sh --dry-run` prints as `rule_payload`; any other definition, such as one an earlier rule update left behind, fails the attempt as infrastructure before warmup. The unit environment must not set `GODEBUG`; only the profiled arm of the [tail-profiling diagnostic](ekuiper-profile-diagnostic.md) traces Go GC, and the runner removes its drop-in before any other eKuiper start.
8. Save `ekuiper-health.json`: `systemctl show kuiper -p NRestarts,ExecMainStatus,MainPID` and the `pipeline_a` status from the REST API, once before warmup and once after the run. If the unit restarted or lost its main process, the run is a `runtime-exit` outcome, as a WAFER crash is. If the rule is not `running` at the end or its status carries a `message`, which eKuiper sets while it retries a failed rule, the run is a `rule-error` outcome. Either outcome stops the run early: it is admitted, not retried, and counts against the experiment's criterion. The capacity scout has its own stop rules, so there either one fails the attempt as infrastructure. `exit_codes.ekuiper` in `metadata.json` records what the snapshots show. The E-Swap-3 rule update is the `ekuiper-rule-update` arm's own action and is not counted: eKuiper starts the rule again inside the PUT, so the snapshot after the run must show a `lastStartTimestamp` within that PUT, and in every other run the rule's `lastStartTimestamp` must not move. The `ekuiper-make-before-break` arm ends with `pipeline_a_v2` in place of `pipeline_a`, so its snapshot after the run reads that rule, which must have started no earlier than the E-Swap-3 action. Per-operator `exceptions_total` counters are not judged. See "eKuiper health" in `eval/RESULT-CONTRACT.md`.

## E-Swap-3 rule changes

E-Swap-3 has two eKuiper arms, and both make the change `threshold-filter-v2` makes on WAFER: Pipeline A with `temperature >= 60` in place of `temperature >= 50`.

`ekuiper-rule-update` is the eKuiper rule update (PUT /rules/pipeline_a):

1. `PUT /rules/pipeline_a` sends the `pipeline_a` rule that `ekuiper-audit.json` recorded before warmup, with only that bound changed. eKuiper plans the new rule, stops the old one and starts the new one before it responds.
2. The harness reads `GET /rules/pipeline_a/status` every 10 ms until the rule reports `running` with an empty `message` and `sink_mqtt_0_0_records_out_total` is above 0, for up to 10 seconds after the PUT returns. The updated rule runs on a new topology whose counters start again from 0, so any count above 0 is output of the updated rule.

`rule-update.json` keeps every call with its method, path, the SQL it sent, HTTP status, body, and monotonic and wall-clock timings. A PUT that does not return 200, or an updated rule that does not publish within 10 seconds, fails the attempt as infrastructure. eKuiper sets `lastStartTimestamp` from the wall clock in whole milliseconds when a rule starts (`internal/topo/rule/state.go`, lines 155 and 457), so the health check accepts a start time from 1 ms before the action started up to the end of the PUT.

The arm updates the rule instead of stopping and starting it because only the update keeps the stream's MQTT subscription. `wafer_telemetry` is a shared stream: every rule on it reads one source subtopology, and that subtopology holds the stream's only MQTT subscription. In eKuiper 2.1.5:

- `UpdateRule` in `internal/server/rule_manager.go` (lines 164 to 198) plans the new rule with `rs.Validate()` before it calls `rs.Stop()` and `rs.Start()`, all inside the PUT.
- Planning a rule on a shared stream calls `GetOrCreateSubTopo` (`internal/topo/planner/planner_source.go`, line 200). It finds the open subtopology and resets the rule's reference to it without an error channel (`internal/topo/subtopo_pool.go`, lines 33 to 55).
- `SrcSubTopo.Close` (`internal/topo/subtopo.go`, lines 197 to 213) skips a reference without an error channel, and otherwise removes it and cancels the source once no reference is left. Stopping the old rule after the new one was planned therefore leaves the subscription open, and the new rule takes its reference back when it starts (`SrcSubTopo.Open`, lines 89 to 91).
- `StopRule` (`rule_manager.go`, line 234) stops a rule whose reference still has its error channel, so stopping the only rule on the stream closes the subscription, and `StartRule` (line 213) plans and opens a new one.

The check in `SrcSubTopo.Close` came with eKuiper commit 6d6df23c, "fix(topo): subtopo ref count error when update" (#3670), first released in 2.1.2. In 2.1.0, `Close` dropped the rule's reference on every stop, including the stop inside an update, so updating the only rule on the stream cancelled the shared source as a stop does. With the update, the arm changes the rule the way eKuiper does in place, and the stream's source keeps its MQTT connection and subscription, which a stop and start closes and opens again.

`ekuiper-make-before-break` changes the rule without stopping it first:

1. `POST /rules` creates `pipeline_a_v2`, the rule that `seed-pipeline-a.sh --dry-run` prints as `replacement_rule_payload`: Pipeline A with `temperature >= 60` in place of `temperature >= 50`, the change `threshold-filter-v2` makes on WAFER. The workload's constant 72.5 passes both rules.
2. The harness reads `GET /rules/pipeline_a_v2/status` every 10 ms until `sink_mqtt_0_0_records_out_total` is above 0, for up to 10 seconds.
3. `DELETE /rules/pipeline_a` retires the old rule.

The emission check reads the replacement's own sink counter because nothing else tells the two rules apart: both read the one `wafer_telemetry` subscription, so the source counters are shared; eKuiper reports `running` before the new rule has received a message; and both rules publish identical payloads to the same topic. While both rules run, a message can be published twice, so the arm can show duplicates; like its loss, they are reported data. Deleting `pipeline_a` only after `pipeline_a_v2` carries data keeps the stream's MQTT subscription open, because eKuiper closes it when the last rule on the stream goes: in eKuiper 2.1.5, `SrcSubTopo.Close` in `internal/topo/subtopo.go` cancels the shared source only when no rule references it any more. Stopping the only rule therefore closes the subscription, and starting it again opens a new one. The smoke run and the comparator re-check exercise both arms on each host before its batch.

`rule-replacement.json` keeps the three calls with their HTTP status, body and timings. The seed script deletes `pipeline_a_v2` and creates `pipeline_a` again before every eKuiper run, so each run starts from the original `pipeline_a` alone, and the audit refuses a run whose `pipeline_a` is any other rule.

## Diagnostic finding

The v11 pilot omitted the eKuiper sink `qos` field, selecting QoS 0 and producing an approximately 20 ms periodic release pattern. A matched Raspberry Pi 5 diagnostic reproduced the pattern with sink QoS 0 and removed it with sink QoS 1. The old v11 eKuiper result is therefore not a valid comparator result and must not be pooled with corrected runs.

See [Why the v11 eKuiper latency tail was misleading](../history/benchmarks/ekuiper-tail-diagnostic.md) for the one-variable evidence, corrected small-N results, and claim boundaries.

## MQTT sink flow control

The two MQTT sinks wait for broker acknowledgements differently. eKuiper's MQTT sink publishes from a single loop and, at QoS 1, waits for each PUBACK before it publishes the next message, so it keeps at most one publish in flight. WAFER's MQTT sink uses rumqttc, which keeps up to 100 unacknowledged publishes in flight by default. At high offered rates eKuiper therefore pays one broker round trip per output message. That may be what sets its delivery ceiling, but it is a hypothesis, not a measured result.

E-Perf-10 compares both systems with these defaults. To test the hypothesis, the capacity scout runs a diagnostic arm, `wafer-max-inflight-1`: the WAFER scout pipeline with `max_inflight = 1` on its MQTT sink, on the same workload and rate search as the other systems. If the arm's delivery ceiling drops to eKuiper's, the publish window accounts for the gap; if it stays near WAFER's default ceiling, it does not. The arm is diagnostic only. It never enters the E-Perf-10 grid or decision and is not thesis evidence. See "Capacity scout" in `eval/RESULT-CONTRACT.md`.

## Known limitations

- Corrected small-N diagnostics characterize the frozen default-style comparator, not eKuiper's best achievable tuning. A two-block diagnostic compared operator concurrency 1 and 3 at 1,000 messages/second. Both settings had a median p95 of 0.327 ms and similar throughput, CPU, and RSS. Concurrency 3 had a lower median p99, 2.411 ms versus 2.652 ms, but N=2 is insufficient to justify selecting a non-default setting after observation. The canonical comparator therefore freezes concurrency 1 regardless of ranking impact.
- Native Pi 5 results are not directly comparable to the old Docker Desktop macOS shakedowns. The latter include a Linux VM and bridge-network overhead.
- Raspberry Pi 5 results are not numerically interchangeable with Raspberry Pi 4 results from prior literature. Report absolute values and WAFER/native/eKuiper ratios.
- eKuiper and WAFER/native Pipeline A all decode the telemetry field used by the filter. Their output schemas, predicate bounds, topics, and QoS are matched; their internal JSON implementations and their MQTT client flow control (see "MQTT sink flow control") remain engine-specific.
- REST port 9081 is distinct from WAFER's port 9090 and Mosquitto's port 1883.

## Related files

- `eval/ekuiper/install-native.sh`
- `eval/ekuiper/gctrace-drop-in.conf`
- `eval/ekuiper/seed-pipeline-a.sh`
- `eval/ekuiper/smoke-test.sh`
- `docs/status/rpi5-canonical-transition.md`
- `docs/rfcs/RFC-008-evaluation-harness.md`
