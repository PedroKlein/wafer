#!/usr/bin/env bash
set -euo pipefail

EKUIPER="${EKUIPER_URL:-http://127.0.0.1:9081}"
BROKER_HOST="${MQTT_HOST:-127.0.0.1}"
BROKER_PORT="${MQTT_PORT:-1883}"
INPUT_TOPIC="wafer/telemetry"
OUTPUT_TOPIC="wafer/telemetry/hot"
dry_run=0

if [ "${1:-}" = "--dry-run" ]; then
    dry_run=1
elif [ "$#" -ne 0 ]; then
    echo "usage: $0 [--dry-run]" >&2
    exit 2
fi

if [ "$dry_run" -eq 1 ]; then
    printf 'ekuiper_url: %s\nbroker: %s:%s\ninput_topic: %s\noutput_topic: %s\n' \
        "$EKUIPER" "$BROKER_HOST" "$BROKER_PORT" "$INPUT_TOPIC" "$OUTPUT_TOPIC"
    exit 0
fi

for command in curl mosquitto_pub mosquitto_sub python3; do
    command -v "$command" >/dev/null || { echo "error: $command is required" >&2; exit 1; }
done
curl -fsS "$EKUIPER/rules/pipeline_a" >/dev/null || {
    echo "error: native eKuiper rule pipeline_a is not reachable" >&2
    exit 1
}

output="$(mktemp)"
trap 'rm -f "$output"' EXIT
mosquitto_sub -h "$BROKER_HOST" -p "$BROKER_PORT" -t "$OUTPUT_TOPIC" -W 4 -C 1 >"$output" &
subscriber_pid=$!
sleep 1
mosquitto_pub -h "$BROKER_HOST" -p "$BROKER_PORT" -t "$INPUT_TOPIC" \
    -m '{"device_id":"below","temperature":49,"humidity":37.2,"ts":1,"seq":1}'
mosquitto_pub -h "$BROKER_HOST" -p "$BROKER_PORT" -t "$INPUT_TOPIC" \
    -m '{"device_id":"above","temperature":100000,"humidity":37.2,"ts":2,"seq":2}'
mosquitto_pub -h "$BROKER_HOST" -p "$BROKER_PORT" -t "$INPUT_TOPIC" \
    -m '{"device_id":"boundary","temperature":50,"humidity":37.2,"ts":3,"seq":3}'
wait "$subscriber_pid" 2>/dev/null || true

python3 - "$output" <<'PY'
import json
import sys
from pathlib import Path

rows = [json.loads(line) for line in Path(sys.argv[1]).read_text().splitlines() if line]
expected = {
    "device_id": "boundary",
    "temperature": 50,
    "humidity": 37.2,
    "ts": 3,
    "seq": 3,
}
if rows != [expected]:
    raise SystemExit(f"FAIL: expected one schema-preserving boundary output, got {rows!r}")
PY

echo "native eKuiper smoke test: PASS"
