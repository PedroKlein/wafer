#!/usr/bin/env bash
# Register Pipeline A on eKuiper via REST API.
#
# Idempotent — deletes existing wafer_telemetry stream and pipeline_a
# rule before re-creating. Safe to run repeatedly during shakedown.
#
# Pre-req: `docker compose -f eval/ekuiper/docker-compose.yml up -d`
# and eKuiper's healthcheck is passing.

set -euo pipefail

EKUIPER="${EKUIPER_URL:-http://127.0.0.1:9081}"
BROKER_INTERNAL="tcp://mosquitto:1883"   # DNS name inside the compose network

_log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }

# Wait for eKuiper REST to come up before pushing definitions.
for i in $(seq 1 30); do
    if curl -sf "$EKUIPER/" >/dev/null 2>&1; then break; fi
    [ "$i" -eq 30 ] && { _log "eKuiper never came up at $EKUIPER"; exit 3; }
    sleep 1
done

# Drop existing definitions (ignore 404).
curl -sf -X DELETE "$EKUIPER/rules/pipeline_a"       >/dev/null 2>&1 || true
curl -sf -X DELETE "$EKUIPER/streams/wafer_telemetry" >/dev/null 2>&1 || true

_log "creating stream wafer_telemetry"
curl -sf -X POST "$EKUIPER/streams" \
    -H "Content-Type: application/json" \
    -d '{"sql":"CREATE STREAM wafer_telemetry (sequence BIGINT, intended_ns BIGINT, temperature FLOAT) WITH (TYPE=\"mqtt\", DATASOURCE=\"wafer/telemetry\", FORMAT=\"json\", SHARED=\"true\")"}' \
    | tee /dev/stderr

echo

_log "creating rule pipeline_a"
curl -sf -X POST "$EKUIPER/rules" \
    -H "Content-Type: application/json" \
    -d "{
  \"id\": \"pipeline_a\",
  \"sql\": \"SELECT sequence, intended_ns, temperature FROM wafer_telemetry WHERE temperature > 50\",
  \"actions\": [
    {
      \"mqtt\": {
        \"server\": \"${BROKER_INTERNAL}\",
        \"topic\": \"wafer/telemetry/hot\",
        \"sendSingle\": true
      }
    }
  ]
}" | tee /dev/stderr

echo
_log "done — verify with: curl -s $EKUIPER/streams | jq"
