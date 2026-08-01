#!/usr/bin/env bash
# Smoke test for eKuiper Pipeline A: asserts BOTH pass AND drop cases.
#
# Negation-blind smoke tests (pass-only) silently succeed when a rule
# regresses to forwarding everything. This script publishes one record
# that must be dropped (temperature=30 < 50) and one that must pass
# (temperature=80 > 50), then asserts exactly the pass record arrives.
#
# Pre-req: eval/ekuiper compose stack running + seed-pipeline-a.sh executed.
#
# Manual regression toggle (to verify this test actually catches regressions):
#   curl -X PUT http://127.0.0.1:9081/rules/pipeline_a \
#     -H 'Content-Type: application/json' \
#     -d '{"id":"pipeline_a","sql":"SELECT ts, seq, temperature FROM wafer_telemetry WHERE temperature > 20","actions":[{"mqtt":{"server":"tcp://mosquitto:1883","topic":"wafer/telemetry/hot","sendSingle":true}}]}'
# Re-run this script — should exit non-zero. Restore via ./seed-pipeline-a.sh.

set -euo pipefail

NETWORK="wafer-ekuiper_default"
BROKER="mosquitto"
SUB_TOPIC="wafer/telemetry/hot"
PUB_TOPIC="wafer/telemetry"
# Timeout for mosquitto_sub. Must be >= 2s because eKuiper's internal
# processing introduces non-trivial latency on cold paths.
SUB_TIMEOUT=3

_log() { printf '[smoke] %s\n' "$*" >&2; }

# --- Cleanup on interruption ---
# Without a trap, Ctrl-C leaves orphaned docker containers running
# mosquitto_sub in the background.
_sub_cid=""
_cleanup() {
    if [ -n "$_sub_cid" ]; then
        docker rm -f "$_sub_cid" >/dev/null 2>&1 || true
    fi
}
trap _cleanup EXIT INT TERM

# --- Validate eKuiper reachable ---
if ! curl -sf http://127.0.0.1:9081/rules/pipeline_a >/dev/null 2>&1; then
    _log "ERROR: eKuiper REST API unreachable (pipeline_a rule not found). Is the stack up?"
    exit 3
fi

# --- Run pub+sub in a single container to avoid inter-container timing races ---
# Subscribe in background, wait for it to connect, then publish both
# records. mosquitto_sub -W exits after SUB_TIMEOUT seconds of silence
# or -C max messages (whichever first).
_sub_cid=$(docker run -d --network "$NETWORK" --name wafer-smoke-sub-$$ \
    eclipse-mosquitto:2 sh -c "
mosquitto_sub -h $BROKER -t '$SUB_TOPIC' -W $SUB_TIMEOUT -C 10 &
SUB_PID=\$!
sleep 1
mosquitto_pub -h $BROKER -t '$PUB_TOPIC' -m '{\"seq\":1,\"ts\":100,\"temperature\":30}'
mosquitto_pub -h $BROKER -t '$PUB_TOPIC' -m '{\"seq\":2,\"ts\":200,\"temperature\":80}'
wait \$SUB_PID
")

_log "container $_sub_cid launched — publishing drop (seq=1,temp=30) + pass (seq=2,temp=80)"

# --- Wait for the container to finish (sub timeout fires after SUB_TIMEOUT + 1s settle) ---
_log "waiting for subscriber timeout (${SUB_TIMEOUT}s)..."
docker wait "$_sub_cid" >/dev/null 2>&1 || true

# --- Collect output ---
output=$(docker logs "$_sub_cid" 2>/dev/null || true)

if [ -z "$output" ]; then
    _log "FAIL: no messages received on $SUB_TOPIC — rule may not be running"
    exit 1
fi

# --- Assert exactly one record received ---
line_count=$(echo "$output" | wc -l | tr -d ' ')
if [ "$line_count" -ne 1 ]; then
    _log "FAIL: expected 1 message, got $line_count — drop case leaked through"
    _log "output:"
    echo "$output" >&2
    exit 1
fi

# --- Assert the surviving record is the pass case (seq=2, temperature=80) ---
if ! echo "$output" | grep -q '"seq":2'; then
    _log "FAIL: received record does not contain seq=2 (expected the pass record)"
    _log "output: $output"
    exit 1
fi

if ! echo "$output" | grep -q '"temperature":80'; then
    _log "FAIL: received record does not contain temperature=80"
    _log "output: $output"
    exit 1
fi

_log "PASS: exactly 1 record received with seq=2, temperature=80"
_log "drop case (seq=1, temperature=30) correctly filtered out"
