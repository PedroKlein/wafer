#!/usr/bin/env bash
# Register Pipeline A on eKuiper via REST API.
#
# Idempotent — deletes existing wafer_telemetry stream and pipeline_a
# rule before re-creating. Safe to run repeatedly during shakedown.
#
# Pre-req: native eKuiper is reachable through its REST API and Mosquitto
# is listening at EKUIPER_BROKER_URL (default: localhost).

set -euo pipefail

EKUIPER="${EKUIPER_URL:-http://127.0.0.1:9081}"
BROKER_URL="${EKUIPER_BROKER_URL:-tcp://127.0.0.1:1883}"
STREAM_PAYLOAD='{"sql":"CREATE STREAM wafer_telemetry (device_id STRING, temperature FLOAT, humidity FLOAT, ts BIGINT, seq BIGINT) WITH (TYPE=\"mqtt\", DATASOURCE=\"wafer/telemetry\", FORMAT=\"json\", SHARED=\"true\")"}'
RULE_PAYLOAD="$(cat <<EOF
{
  "id": "pipeline_a",
  "sql": "SELECT device_id, temperature, humidity, ts, seq FROM wafer_telemetry WHERE temperature >= 50 AND temperature <= 99999",
  "actions": [
    {
      "mqtt": {
        "server": "${BROKER_URL}",
        "topic": "wafer/telemetry/hot",
        "protocolVersion": "3.1.1",
        "qos": 1,
        "retained": false,
        "sendSingle": true
      }
    }
  ]
}
EOF
)"
dry_run=0

if [ "${1:-}" = "--dry-run" ]; then
    dry_run=1
elif [ "$#" -ne 0 ]; then
    echo "usage: $0 [--dry-run]" >&2
    exit 2
fi

if [ "$dry_run" -eq 1 ]; then
    printf '{"ekuiper_url":"%s","broker_url":"%s","stream_payload":%s,"rule_payload":%s}\n' \
        "$EKUIPER" "$BROKER_URL" "$STREAM_PAYLOAD" "$RULE_PAYLOAD"
    exit 0
fi

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
# Field names match wafer-loadgen's telemetry-120b template output:
#   {"device_id":"...","temperature":72.5,"humidity":37.2,"ts":N,"seq":N}
curl -sf -X POST "$EKUIPER/streams" \
    -H "Content-Type: application/json" \
    -d "$STREAM_PAYLOAD" \
    | tee /dev/stderr

echo

_log "creating rule pipeline_a"
# Pass ts and seq through unchanged so wafer-loadgen subscribe can
# compute end-to-end latency from the embedded intended-publish timestamp.
curl -sf -X POST "$EKUIPER/rules" \
    -H "Content-Type: application/json" \
    -d "$RULE_PAYLOAD" | tee /dev/stderr

echo
_log "done — verify with: curl -s $EKUIPER/streams | jq"
