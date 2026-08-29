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

for command in curl mosquitto_pub mosquitto_sub; do
    command -v "$command" >/dev/null || { echo "error: $command is required" >&2; exit 1; }
done
curl -fsS "$EKUIPER/rules/pipeline_a" >/dev/null || {
    echo "error: native eKuiper rule pipeline_a is not reachable" >&2
    exit 1
}

output="$(mktemp)"
trap 'rm -f "$output"' EXIT
mosquitto_sub -h "$BROKER_HOST" -p "$BROKER_PORT" -t "$OUTPUT_TOPIC" -W 4 -C 2 >"$output" &
subscriber_pid=$!
sleep 1
mosquitto_pub -h "$BROKER_HOST" -p "$BROKER_PORT" -t "$INPUT_TOPIC" \
    -m '{"seq":1,"ts":1,"temperature":30}'
mosquitto_pub -h "$BROKER_HOST" -p "$BROKER_PORT" -t "$INPUT_TOPIC" \
    -m '{"seq":2,"ts":2,"temperature":80}'
wait "$subscriber_pid" 2>/dev/null || true

if grep -q '"seq":1' "$output"; then
    echo "FAIL: eKuiper forwarded the below-threshold record" >&2
    exit 1
fi
if ! grep -q '"seq":2' "$output"; then
    echo "FAIL: eKuiper did not forward the above-threshold record" >&2
    cat "$output" >&2
    exit 1
fi

echo "native eKuiper smoke test: PASS"
