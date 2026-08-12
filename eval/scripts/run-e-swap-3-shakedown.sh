#!/usr/bin/env bash
# eval/scripts/run-e-swap-3-shakedown.sh
#
# E-Swap-3: throughput dip comparison across three strategies:
#   1. WAFER hot-swap (drain-and-flip, zero-downtime)
#   2. WAFER full-restart (kill + restart with new config)
#   3. eKuiper rule-restart (POST /rules/pipeline_a/restart)
#
# Measures throughput time-series around the swap/restart event.
# Reports throughput_dip_pct per strategy.
#
# Pre-reqs: release binaries, eKuiper stack running, plugins built.
#
# Metadata provenance (T8, thesis-hardening plan, 2026-08-02):
#   This legacy shakedown script writes a bespoke per-experiment `metadata.json`
#   with only the fields its analysis notebook needs. It does NOT source
#   `eval/scripts/lib/write_metadata.py` (which merges the runtime-emitted
#   `runtime-provenance.json` sidecar to produce keys like `wasmtime_version`,
#   `wafer_runtime_sha256`, `wafer_plugin_hashes`).
#
#   Rationale: shakedowns exist to sanity-check RFC-008 experiment claims on
#   macOS before the canonical Pi runs. Full-provenance metadata belongs in
#   the canonical harness (`eval/scripts/run-experiment.sh`) which always
#   sources the merger. Retro-fitting the merger into this script would
#   require re-running every shakedown baseline (out of T8 scope by design).
#
#   `verify-result-contract.py` emits a WARN (not a violation) when a
#   shakedown `metadata.json` lacks the merged provenance keys, so the gap
#   is surfaced without breaking existing baseline dirs.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

# Kill any wafer-runtime child on Ctrl-C or unexpected exit. Without this,
# an interrupt during the swap-at-t=10s wait leaves the runtime holding
# the MQTT subscriptions, breaking subsequent runs.
_wafer_pids=()
_cleanup_swap3() {
    local rc=$?
    for p in "${_wafer_pids[@]:-}"; do
        [ -n "$p" ] && kill -TERM "$p" 2>/dev/null || true
    done
    exit "$rc"
}
trap _cleanup_swap3 EXIT INT TERM

# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------
runs=5
skip_build=0
dry_run=0

while [ $# -gt 0 ]; do
    case "$1" in
        --runs)       runs="${2:?}"; shift 2 ;;
        --skip-build) skip_build=1; shift ;;
        --dry-run)    dry_run=1; shift ;;
        -h|--help)    echo "Usage: $0 [--runs N] [--skip-build] [--dry-run]"; exit 0 ;;
        *) printf 'unknown: %s\n' "$1" >&2; exit 2 ;;
    esac
done

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
_log() { printf '[e-swap-3] %s\n' "$*" >&2; }
_now_ns() { python3 -c 'import time; print(int(time.time()*1e9))'; }

export DOCKER_HOST="${DOCKER_HOST:-unix://${HOME}/.colima/default/docker.sock}"

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------
WAFER_BIN="$REPO_ROOT/target/release/wafer"
LOADGEN_BIN="$REPO_ROOT/target/release/wafer-loadgen"
if [ "$skip_build" -eq 0 ]; then
    _log "building release binaries"
    cargo build --release -p wafer-runtime -p wafer-loadgen >&2
fi
[ -x "$WAFER_BIN" ]   || { _log "wafer binary missing"; exit 3; }
[ -x "$LOADGEN_BIN" ] || { _log "loadgen binary missing"; exit 3; }

# Plugins for hot-swap
V1_PLUGIN="$REPO_ROOT/plugins/pass-through-v1/target/wasm32-wasip2/release/wafer_pass_through_v1.wasm"
V2_PLUGIN="$REPO_ROOT/plugins/pass-through-v2/target/wasm32-wasip2/release/wafer_pass_through_v2.wasm"
[ -f "$V1_PLUGIN" ] || { _log "v1 plugin missing"; exit 3; }
[ -f "$V2_PLUGIN" ] || { _log "v2 plugin missing"; exit 3; }

# ---------------------------------------------------------------------------
# eKuiper lifecycle helpers
# ---------------------------------------------------------------------------
_stop_ekuiper_rule() {
    curl -sf -X POST "http://127.0.0.1:9081/rules/pipeline_a/stop" >/dev/null 2>&1 || true
}
_start_ekuiper_rule() {
    curl -sf -X POST "http://127.0.0.1:9081/rules/pipeline_a/start" >/dev/null 2>&1 || true
    sleep 1
}
_restart_ekuiper_rule() {
    curl -sf -X POST "http://127.0.0.1:9081/rules/pipeline_a/restart" >/dev/null 2>&1 || true
}

# ---------------------------------------------------------------------------
# Output root
# ---------------------------------------------------------------------------
TS="$(date -u +'%Y-%m-%dT%H-%M-%SZ')"
OUT_ROOT="$REPO_ROOT/eval/results/e-swap-3/shakedown-macos-$TS"
mkdir -p "$OUT_ROOT"/{hotswap,restart,ekuiper-restart}
_log "output: $OUT_ROOT"

if [ "$dry_run" -eq 1 ]; then _log "DRY RUN"; exit 0; fi

TOPIC_IN="wafer/telemetry"
TOPIC_OUT="wafer/telemetry/hot"
RATE=1000
DURATION_SECS=20
SWAP_AT_SECS=10
# Expected messages: rate × (duration - setup overhead). Leave headroom.
SUB_TOTAL_MESSAGES=15000

# ---------------------------------------------------------------------------
# Strategy 1: WAFER hot-swap (v1 → v2 at t=10s, measure dip)
# ---------------------------------------------------------------------------
_run_hotswap() {
    local run_idx=$1
    local run_label; run_label="run-$(printf '%02d' "$run_idx")"
    local out_dir="$OUT_ROOT/hotswap/$run_label"
    mkdir -p "$out_dir"

    _stop_ekuiper_rule
    local started_ns; started_ns=$(_now_ns)

    # Start WAFER with API enabled for hot-swap POST
    "$WAFER_BIN" --config "$REPO_ROOT/eval/configs/e-swap/pipeline-swap3-mqtt.toml" \
        --swap-output-dir "$out_dir" \
        >"$out_dir/stdout.log" 2>&1 &
    local wafer_pid=$!
    _wafer_pids+=("$wafer_pid")
    sleep 2

    if ! kill -0 "$wafer_pid" 2>/dev/null; then
        _log "  [hotswap/$run_label] runtime died"
        return 1
    fi

    # Subscriber
    "$LOADGEN_BIN" subscribe \
        --broker "localhost:1883" --topic "$TOPIC_OUT" \
        --output-dir "$out_dir" --total-messages "$SUB_TOTAL_MESSAGES" \
        >>"$out_dir/stdout.log" 2>&1 &
    local sub_pid=$!
    sleep 0.5

    # Publisher with hotswap-trigger profile
    "$LOADGEN_BIN" publish \
        --broker-host localhost --broker-port 1883 \
        --topic "$TOPIC_IN" --rate "$RATE" --duration-secs "$DURATION_SECS" \
        --payload-template telemetry-120b \
        --profile hotswap-trigger \
        --hotswap-target-node transform \
        --hotswap-wasm-path "$V2_PLUGIN" \
        --hotswap-swap-at-secs "$SWAP_AT_SECS" \
        --hotswap-api-url "http://localhost:9090" \
        >>"$out_dir/stdout.log" 2>&1 &
    local pub_pid=$!

    # Wait for publisher to finish
    wait "$pub_pid" 2>/dev/null || true
    sleep 1

    kill "$sub_pid" 2>/dev/null || true
    kill -TERM "$wafer_pid" 2>/dev/null || true
    wait "$sub_pid" 2>/dev/null || true
    wait "$wafer_pid" 2>/dev/null || true

    local finished_ns; finished_ns=$(_now_ns)
    _write_swap_metadata "$out_dir" "hotswap" "$run_idx" "$started_ns" "$finished_ns"
    _log "  [hotswap/$run_label] done"
}

# ---------------------------------------------------------------------------
# Strategy 2: WAFER full-restart (kill at t=10s, restart, measure dip)
# ---------------------------------------------------------------------------
_run_restart() {
    local run_idx=$1
    local run_label; run_label="run-$(printf '%02d' "$run_idx")"
    local out_dir="$OUT_ROOT/restart/$run_label"
    mkdir -p "$out_dir"

    _stop_ekuiper_rule
    local started_ns; started_ns=$(_now_ns)

    # Start WAFER
    "$WAFER_BIN" --config "$REPO_ROOT/eval/configs/e-swap/pipeline-swap3-mqtt.toml" --no-api \
        >"$out_dir/stdout.log" 2>&1 &
    local wafer_pid=$!
    _wafer_pids+=("$wafer_pid")
    sleep 2

    # Subscriber (runs for full duration)
    "$LOADGEN_BIN" subscribe \
        --broker "localhost:1883" --topic "$TOPIC_OUT" \
        --output-dir "$out_dir" --total-messages "$SUB_TOTAL_MESSAGES" \
        >>"$out_dir/stdout.log" 2>&1 &
    local sub_pid=$!
    sleep 0.5

    # Publisher for full duration
    "$LOADGEN_BIN" publish \
        --broker-host localhost --broker-port 1883 \
        --topic "$TOPIC_IN" --rate "$RATE" --duration-secs "$DURATION_SECS" \
        --payload-template telemetry-120b \
        >>"$out_dir/stdout.log" 2>&1 &
    local pub_pid=$!

    # At t=SWAP_AT_SECS, kill and restart WAFER
    sleep "$SWAP_AT_SECS"
    local kill_ns; kill_ns=$(_now_ns)
    kill -KILL "$wafer_pid" 2>/dev/null || true
    wait "$wafer_pid" 2>/dev/null || true
    printf '%s\n' "$kill_ns" > "$out_dir/kill_timestamp_ns.txt"

    # Restart immediately
    "$WAFER_BIN" --config "$REPO_ROOT/eval/configs/e-swap/pipeline-swap3-mqtt.toml" --no-api \
        >>"$out_dir/stdout.log" 2>&1 &
    wafer_pid=$!
    _wafer_pids+=("$wafer_pid")
    local restart_ns; restart_ns=$(_now_ns)
    printf '%s\n' "$restart_ns" > "$out_dir/restart_timestamp_ns.txt"

    # Wait for publisher to finish
    wait "$pub_pid" 2>/dev/null || true
    sleep 1

    kill "$sub_pid" 2>/dev/null || true
    kill -TERM "$wafer_pid" 2>/dev/null || true
    wait "$sub_pid" 2>/dev/null || true
    wait "$wafer_pid" 2>/dev/null || true

    local finished_ns; finished_ns=$(_now_ns)
    _write_swap_metadata "$out_dir" "restart" "$run_idx" "$started_ns" "$finished_ns"
    _log "  [restart/$run_label] done"
}

# ---------------------------------------------------------------------------
# Strategy 3: eKuiper rule-restart
# ---------------------------------------------------------------------------
_run_ekuiper_restart() {
    local run_idx=$1
    local run_label; run_label="run-$(printf '%02d' "$run_idx")"
    local out_dir="$OUT_ROOT/ekuiper-restart/$run_label"
    mkdir -p "$out_dir"

    _start_ekuiper_rule
    local started_ns; started_ns=$(_now_ns)

    # Subscriber
    "$LOADGEN_BIN" subscribe \
        --broker "localhost:1883" --topic "$TOPIC_OUT" \
        --output-dir "$out_dir" --total-messages "$SUB_TOTAL_MESSAGES" \
        >>"$out_dir/stdout.log" 2>&1 &
    local sub_pid=$!
    sleep 0.5

    # Publisher
    "$LOADGEN_BIN" publish \
        --broker-host localhost --broker-port 1883 \
        --topic "$TOPIC_IN" --rate "$RATE" --duration-secs "$DURATION_SECS" \
        --payload-template telemetry-120b \
        >>"$out_dir/stdout.log" 2>&1 &
    local pub_pid=$!

    # At t=SWAP_AT_SECS, restart eKuiper rule
    sleep "$SWAP_AT_SECS"
    local restart_ns; restart_ns=$(_now_ns)
    _restart_ekuiper_rule
    printf '%s\n' "$restart_ns" > "$out_dir/restart_timestamp_ns.txt"

    # Wait for publisher to finish
    wait "$pub_pid" 2>/dev/null || true
    sleep 1

    kill "$sub_pid" 2>/dev/null || true
    wait "$sub_pid" 2>/dev/null || true

    local finished_ns; finished_ns=$(_now_ns)
    _write_swap_metadata "$out_dir" "ekuiper-restart" "$run_idx" "$started_ns" "$finished_ns"
    _log "  [ekuiper-restart/$run_label] done"
}

# ---------------------------------------------------------------------------
# Metadata writer
# ---------------------------------------------------------------------------
_write_swap_metadata() {
    local out_dir=$1 strategy=$2 run_idx=$3 started_ns=$4 finished_ns=$5
    local duration_ns=$(( finished_ns - started_ns ))
    python3 -c "
import json
print(json.dumps({
    'experiment': 'e-swap-3',
    'strategy': '$strategy',
    'host_tag': 'shakedown-macos',
    'run_index': $run_idx,
    'started_ns': $started_ns,
    'finished_ns': $finished_ns,
    'duration_ns': $duration_ns,
    'swap_at_secs': $SWAP_AT_SECS,
    'rate_msg_s': $RATE,
    'git_sha': '$(git rev-parse --short HEAD 2>/dev/null || echo unknown)',
    'hostname': '$(hostname)',
    'arch': '$(uname -m)',
    'os': 'darwin',
}, indent=2))
" > "$out_dir/metadata.json"
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
_log "=== hotswap (${runs} runs) ==="
for i in $(seq 1 "$runs"); do _run_hotswap "$i"; sleep 1; done

_log "=== restart (${runs} runs) ==="
for i in $(seq 1 "$runs"); do _run_restart "$i"; sleep 1; done

_log "=== ekuiper-restart (${runs} runs) ==="
for i in $(seq 1 "$runs"); do _run_ekuiper_restart "$i"; sleep 1; done

# Ensure eKuiper is left running
_start_ekuiper_rule

# Roll-up
python3 -c "
import json
print(json.dumps({
    'experiment': 'e-swap-3',
    'shakedown': True,
    'host_tag': 'shakedown-macos',
    'generated_at': '$(date -u +%Y-%m-%dT%H:%M:%SZ)',
    'git_sha': '$(git rev-parse --short HEAD 2>/dev/null || echo unknown)',
    'strategies': ['hotswap', 'restart', 'ekuiper-restart'],
    'runs_per_strategy': $runs,
    'rate_msg_s': $RATE,
    'swap_at_secs': $SWAP_AT_SECS,
    'duration_secs': $DURATION_SECS,
}, indent=2))
" > "$OUT_ROOT/shakedown.json"

_log "shakedown complete: $OUT_ROOT"
