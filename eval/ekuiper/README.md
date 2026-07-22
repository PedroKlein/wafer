# eval/ekuiper — eKuiper 2.1.x LTS comparator stack

Docker Compose stack for the RQ1/RQ3 comparator experiments. Reads:
`../../docs/benchmarks/ekuiper-comparator.md` for the full write-up.

## Quick start

```sh
docker compose up -d
./seed-pipeline-a.sh
```

## Files

- `docker-compose.yml` — mosquitto + eKuiper stack (label
  `wafer-harness=1`).
- `mosquitto.conf` — anonymous auth, all-interfaces listener.
- `pipeline-a-rule.sql` — human-readable Pipeline A definition.
- `seed-pipeline-a.sh` — idempotent stream + rule registration.

## Verification

```sh
curl -s :9081/streams | jq       # ["wafer_telemetry"]
curl -s :9081/rules   | jq       # pipeline_a running
```

## Teardown

```sh
docker compose down
```
