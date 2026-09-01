#!/usr/bin/env bash

set -euo pipefail

EKUIPER="${EKUIPER_URL:-http://127.0.0.1:9081}"
BROKER_URL="${EKUIPER_BROKER_URL:-tcp://127.0.0.1:1883}"
STREAM_PAYLOAD='{"sql":"CREATE STREAM wafer_telemetry (device_id STRING, temperature FLOAT, humidity FLOAT, ts BIGINT, seq BIGINT) WITH (TYPE=\"mqtt\", DATASOURCE=\"wafer/telemetry\", FORMAT=\"json\", SHARED=\"true\")"}'
dry_run=0
concurrency=1

while [ "$#" -gt 0 ]; do
    case "$1" in
        --concurrency)
            concurrency="${2:?}"
            shift 2
            ;;
        --dry-run)
            dry_run=1
            shift
            ;;
        *)
            echo "usage: $0 [--concurrency <positive-integer>] [--dry-run]" >&2
            exit 2
            ;;
    esac
done

case "$concurrency" in
    ''|*[!0-9]*|0)
        echo "error: --concurrency must be a positive integer" >&2
        exit 2
        ;;
esac

RULE_PAYLOAD="$(cat <<EOF
{
  "id": "pipeline_a",
  "sql": "SELECT device_id, temperature, humidity, ts, seq FROM wafer_telemetry",
  "options": {
    "concurrency": ${concurrency}
  },
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

if [ "$dry_run" -eq 1 ]; then
    printf '{"ekuiper_url":"%s","broker_url":"%s","stream_payload":%s,"rule_payload":%s}\n' \
        "$EKUIPER" "$BROKER_URL" "$STREAM_PAYLOAD" "$RULE_PAYLOAD"
    exit 0
fi

for i in $(seq 1 30); do
    if curl -sf "$EKUIPER/" >/dev/null 2>&1; then break; fi
    [ "$i" -eq 30 ] && { echo "eKuiper never came up at $EKUIPER" >&2; exit 3; }
    sleep 1
done

curl -sf -X DELETE "$EKUIPER/rules/pipeline_a" >/dev/null 2>&1 || true
curl -sf -X DELETE "$EKUIPER/streams/wafer_telemetry" >/dev/null 2>&1 || true
curl -sf -X POST "$EKUIPER/streams" \
    -H "Content-Type: application/json" \
    -d "$STREAM_PAYLOAD"
echo
curl -sf -X POST "$EKUIPER/rules" \
    -H "Content-Type: application/json" \
    -d "$RULE_PAYLOAD"
echo
