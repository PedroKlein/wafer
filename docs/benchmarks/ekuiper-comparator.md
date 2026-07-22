# eKuiper comparator setup

**Version pin.** eKuiper `2.1.0-alpine` (v2.1 LTS, released 2025-03-11).
Locked at RFC-008 §D6 planning time and re-verified during P1.4
shakedown.

**Role.** Reference stream-processing engine for the RQ1 comparative
experiments (E-Perf-1 throughput, E-Perf-2 latency) and the RQ3
comparator (E-Swap-3 hot-swap dip vs full restart). eKuiper is
treated as a black box driven identically to WAFER via
`wafer-loadgen publish` + `wafer-loadgen subscribe`.

## macOS setup

```sh
cd eval/ekuiper
docker compose up -d
./seed-pipeline-a.sh
curl -s :9081/streams | jq .   # → ["wafer_telemetry"]
curl -s :9081/rules   | jq .   # → [{"id":"pipeline_a","status":"running",...}]
```

Prereqs: Docker Desktop or colima. The compose file uses the compose
default network name (`wafer-ekuiper_default`); scripts referring to
it (loadgen, seed) run inside that network via `--network` on
`docker run`.

Cleanup:

```sh
docker compose -f eval/ekuiper/docker-compose.yml down
```

## Pipeline A definition

Mirrors RFC-008 §D6: MQTT input → JSON parse → `temperature > 50`
filter → MQTT output. Stream + rule are defined in
`pipeline-a-rule.sql` (documentation-only file) and pushed via
`seed-pipeline-a.sh` using the eKuiper REST API on port 9081.

**Stream:** `wafer_telemetry` — MQTT source subscribing to
`wafer/telemetry`, JSON payload with fields `sequence`,
`intended_ns`, `temperature`.

**Rule:** `pipeline_a` — SQL:

```sql
SELECT sequence, intended_ns, temperature
  FROM wafer_telemetry
  WHERE temperature > 50;
```

Action: MQTT sink → `wafer/telemetry/hot` on the same broker
(`tcp://mosquitto:1883` inside the compose network).

`sendSingle: true` ensures one output message per input record
matching the filter (not batched into a JSON array). This matches
how WAFER's MQTT sink emits messages, so latency comparisons stay
apples-to-apples.

## Smoke test

The compose stack ships with mosquitto on the same bridge network as
eKuiper. Verify end-to-end plumbing:

```sh
# Terminal A — subscribe on the sink topic
docker run --rm --network wafer-ekuiper_default eclipse-mosquitto:2 \
    mosquitto_sub -h mosquitto -t wafer/telemetry/hot

# Terminal B — publish two messages, one filtered, one passing
docker run --rm --network wafer-ekuiper_default eclipse-mosquitto:2 sh -c '
    mosquitto_pub -h mosquitto -t wafer/telemetry \
        -m "{\"seq\":1,\"ts\":100,\"temperature\":30}"
    mosquitto_pub -h mosquitto -t wafer/telemetry \
        -m "{\"seq\":2,\"ts\":200,\"temperature\":80}"'
```

Terminal A should show only the `temperature=80` message.

Actual observed during P1.4 shakedown:

```
{"ts":200,"seq":2,"temperature":80}
```

The `temperature=30` record is correctly dropped by the filter; the
`temperature=80` record passes through with all three fields
preserved. Comparator plumbing works end-to-end.

## Driving comparators

`wafer-loadgen publish` publishes to `wafer/telemetry` at a
configured rate; `wafer-loadgen subscribe` reads from
`wafer/telemetry/hot` and records latency to a HdrHistogram log.

To compare WAFER vs eKuiper on Pipeline A:

1. Start eKuiper: `docker compose -f eval/ekuiper/docker-compose.yml up -d`
2. Seed: `./eval/ekuiper/seed-pipeline-a.sh`
3. Run comparator experiment: `./eval/scripts/run-e-perf-1-shakedown.sh`
   (or the equivalent E-Perf-2 / E-Swap-3 script).
4. Stop eKuiper's rule, restart WAFER runtime on the same broker + topics.
5. Re-run the loadgen driver; compare the two `latency.hdr` files.

## Deltas from RFC-008 §D6

- **§D6 called for eKuiper 2.1.x LTS.** We pinned `2.1.0-alpine`
  (the LTS point-release). No behavioural drift from the RFC.
- **§D6 planned mosquitto as a separate service.** Kept as a
  separate service inside the same compose stack. Simpler
  lifecycle; identical behaviour.
- **§D6 did not specify `sendSingle`.** Added explicitly to keep
  one input → one output semantics matching WAFER.

## Known limitations

- **Docker-Desktop / colima overhead.** macOS runs Docker inside a
  Linux VM; MQTT hops incur ~1 ms extra latency vs Linux native.
  Canonical Pi runs on Linux will show tighter tails.
- **JSON parsing.** eKuiper decodes JSON per message; WAFER's
  pass-through moves bytes without deserialising. This is a
  legitimate architectural difference the RQ1 comparators are
  designed to measure — do NOT try to "fix" it.
- **REST API port 9081.** Distinct from WAFER's runtime port 9090
  and mosquitto's 1883. No conflicts.

## Related

- `eval/ekuiper/docker-compose.yml`
- `eval/ekuiper/seed-pipeline-a.sh`
- `eval/ekuiper/pipeline-a-rule.sql`
- `eval/ekuiper/mosquitto.conf`
- `plans/evaluation-infrastructure/plan.json` task P1.4
- RFC-008 §D6
