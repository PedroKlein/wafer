#!/usr/bin/env bash
# eval/scripts/run-e-perf-1-2-shakedown.sh
#
# E-Perf-1 (throughput) + E-Perf-2 (latency) comparator shakedown.
# Drives Pipeline A at 1000 msg/s through three systems: WAFER, native, eKuiper.
# 30 runs per system. Result directories conform to eval/RESULT-CONTRACT.md.
#
# Pre-reqs:
#   - eKuiper compose stack running (docker compose -f eval/ekuiper/docker-compose.yml up -d)
#   - eKuiper seeded (eval/ekuiper/seed-pipeline-a.sh)
#   - Release binaries built (wafer-runtime + wafer-loadgen)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

# Kill any wafer-runtime child on Ctrl-C or unexpected exit. Without this,
# an interrupt mid-run leaves the runtime + MQTT subscribers alive on ports,
# causing bind conflicts on the next invocation.
_wafer_pids=()
_cleanup_perf12() {
    local rc=$?
    for p in "${_wafer_pids[@]:-}"; do
        [ -n "$p" ] && kill -TERM "$p" 2>/dev/null || true
    done
    exit "$rc"
}
trap _cleanup_perf12 EXIT INT TERM

# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------
runs=30
skip_build=0
dry_run=0
systems="wafer,native,ekuiper"

while [ $# -gt 0 ]; do
    case "$1" in
        --runs)       runs="${2:?}"; shift 2 ;;
        --systems)    systems="${2:?}"; shift 2 ;;
        --skip-build) skip_build=1; shift ;;
        --dry-run)    dry_run=1; shift ;;
        -h|--help)    echo "Usage: $0 [--runs N] [--systems wafer,native,ekuiper] [--skip-build] [--dry-run]"; exit 0 ;;
        *) printf 'unknown: %s\n' "$1" >&2; exit 2 ;;
    esac
done

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
_log() { printf '[e-perf-1/2] %s\n' "$*" >&2; }
_now_ns() { python3 -c 'import time; print(int(time.time()*1e9))'; }
_sha256() { shasum -a 256 "$1" | awk '{print $1}'; }

export DOCKER_HOST="${DOCKER_HOST:-unix://<home>/.colima/default/docker.sock}"

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------
WAFER_BIN="$REPO_ROOT/target/release/wafer"
LOADGEN_BIN="$REPO_ROOT/target/release/wafer-loadgen"
if [ "$skip_build" -eq 0 ]; then
    _log "building wafer-runtime + wafer-loadgen (release)"
    cargo build --release -p wafer-runtime -p wafer-loadgen >&2
fi
[ -x "$WAFER_BIN" ]   || { _log "wafer binary missing"; exit 3; }
[ -x "$LOADGEN_BIN" ] || { _log "loadgen binary missing"; exit 3; }

# ---------------------------------------------------------------------------
# Verify eKuiper is running
# ---------------------------------------------------------------------------
if ! docker ps --filter label=wafer-harness=1 --format '{{.Names}}' | grep -q ekuiper; then
    _log "eKuiper not running — starting compose stack"
    docker compose -f eval/ekuiper/docker-compose.yml up -d
    sleep 3
    ./eval/ekuiper/seed-pipeline-a.sh
fi

# Verify rule is active
rule_status=$(curl -sf http://127.0.0.1:9081/rules 2>/dev/null | python3 -c "import json,sys;rules=json.load(sys.stdin);print(next((r['status'] for r in rules if r['id']=='pipeline_a'),'missing'))" 2>/dev/null || echo "unreachable")
if [ "$rule_status" != "running" ]; then
    _log "WARNING: pipeline_a rule status=$rule_status (expected: running)"
    _log "re-seeding..."
    ./eval/ekuiper/seed-pipeline-a.sh
fi
_log "eKuiper pipeline_a: $rule_status"

# ---------------------------------------------------------------------------
# Output directories
# ---------------------------------------------------------------------------
TS="$(date -u +'%Y-%m-%dT%H-%M-%SZ')"
PERF1_ROOT="$REPO_ROOT/eval/results/e-perf-1/shakedown-macos-$TS"
PERF2_ROOT="$REPO_ROOT/eval/results/e-perf-2/shakedown-macos-$TS"
mkdir -p "$PERF1_ROOT" "$PERF2_ROOT"
_log "E-Perf-1 output: $PERF1_ROOT"
_log "E-Perf-2 output: $PERF2_ROOT"

if [ "$dry_run" -eq 1 ]; then _log "DRY RUN"; exit 0; fi

# ---------------------------------------------------------------------------
# Per-run driver
# ---------------------------------------------------------------------------
TOTAL_MESSAGES=9000
DURATION_SECS=10
RATE=1000
TOPIC_IN="wafer/telemetry"
TOPIC_OUT="wafer/telemetry/hot"

_run_one() {
    local system=$1 run_idx=$2
    local run_label; run_label="run-$(printf '%02d' "$run_idx")"
    local perf1_dir="$PERF1_ROOT/$system/$run_label"
    local perf2_dir="$PERF2_ROOT/$system/$run_label"
    mkdir -p "$perf1_dir" "$perf2_dir"

    local started_ns; started_ns=$(_now_ns)
    local started_at; started_at="$(date -u +'%Y-%m-%dT%H:%M:%SZ')"
    local wafer_pid=""

    # Start runtime for wafer/native (eKuiper is already running)
    if [ "$system" = "wafer" ]; then
        "$WAFER_BIN" --config "$REPO_ROOT/eval/configs/pipeline-a-wafer.toml" --no-api \
            >"$perf1_dir/stdout.log" 2>&1 &
        wafer_pid=$!
        _wafer_pids+=("$wafer_pid")
        sleep 1
        if ! kill -0 "$wafer_pid" 2>/dev/null; then
            _log "  [$system/$run_label] runtime died on startup"
            return 1
        fi
    elif [ "$system" = "native" ]; then
        "$WAFER_BIN" --config "$REPO_ROOT/eval/configs/pipeline-a-native.toml" --no-api \
            >"$perf1_dir/stdout.log" 2>&1 &
        wafer_pid=$!
        _wafer_pids+=("$wafer_pid")
        sleep 1
        if ! kill -0 "$wafer_pid" 2>/dev/null; then
            _log "  [$system/$run_label] runtime died on startup"
            return 1
        fi
    fi
    # eKuiper: no runtime to start, it's already processing

    # Start subscriber (writes latency.hdr + subscriber-metadata.json)
    "$LOADGEN_BIN" subscribe \
        --broker "localhost:1883" \
        --topic "$TOPIC_OUT" \
        --output-dir "$perf1_dir" \
        --total-messages "$TOTAL_MESSAGES" \
        >>"$perf1_dir/stdout.log" 2>&1 &
    local sub_pid=$!
    sleep 0.3

    # Start publisher
    "$LOADGEN_BIN" publish \
        --broker-host localhost --broker-port 1883 \
        --topic "$TOPIC_IN" \
        --rate "$RATE" \
        --duration-secs "$DURATION_SECS" \
        --payload-template telemetry-120b \
        >>"$perf1_dir/stdout.log" 2>&1 &
    local pub_pid=$!

    # Wait for subscriber to finish (or timeout at 30s)
    local deadline=$(( $(date +%s) + 30 ))
    while kill -0 "$sub_pid" 2>/dev/null; do
        if [ "$(date +%s)" -ge "$deadline" ]; then
            _log "  [$system/$run_label] timeout waiting for subscriber"
            break
        fi
        sleep 0.5
    done

    # Cleanup
    kill "$pub_pid" 2>/dev/null || true
    kill "$sub_pid" 2>/dev/null || true
    [ -n "$wafer_pid" ] && { kill -TERM "$wafer_pid" 2>/dev/null || true; }
    wait "$pub_pid" 2>/dev/null || true
    wait "$sub_pid" 2>/dev/null || true
    [ -n "$wafer_pid" ] && { wait "$wafer_pid" 2>/dev/null || true; }

    local finished_ns; finished_ns=$(_now_ns)
    local duration_ns=$(( finished_ns - started_ns ))

    # Copy latency artifacts into perf2 dir (same data, different experiment view)
    cp "$perf1_dir/subscriber-metadata.json" "$perf2_dir/" 2>/dev/null || true
    cp "$perf1_dir/latency.hdr" "$perf2_dir/" 2>/dev/null || true
    cp "$perf1_dir/sequence.csv" "$perf2_dir/" 2>/dev/null || true

    # Extract quick stats
    local recorded=0 p50=0 p99=0
    if [ -f "$perf1_dir/subscriber-metadata.json" ]; then
        recorded=$(python3 -c "import json;d=json.load(open('$perf1_dir/subscriber-metadata.json'));print(d.get('total_recorded',0))" 2>/dev/null || echo 0)
        p50=$(python3 -c "import json;d=json.load(open('$perf1_dir/subscriber-metadata.json'));print(d.get('latency_p50_ns',0))" 2>/dev/null || echo 0)
        p99=$(python3 -c "import json;d=json.load(open('$perf1_dir/subscriber-metadata.json'));print(d.get('latency_p99_ns',0))" 2>/dev/null || echo 0)
    fi

    # Write throughput.csv (simple: messages/duration)
    local thr_msg_s=0
    if [ "$recorded" -gt 0 ]; then
        thr_msg_s=$(python3 -c "print(round($recorded / ($duration_ns / 1e9), 1))" 2>/dev/null || echo 0)
    fi
    printf 'timestamp_ns,messages_received,throughput_msg_s,duration_ns\n' > "$perf1_dir/throughput.csv"
    printf '%s,%s,%s,%s\n' "$finished_ns" "$recorded" "$thr_msg_s" "$duration_ns" >> "$perf1_dir/throughput.csv"

    # Write metadata
    python3 -c "
import json
print(json.dumps({
    'experiment': 'e-perf-1',
    'system': '$system',
    'host_tag': 'shakedown-macos',
    'run_index': $run_idx,
    'started_at': '$started_at',
    'duration_ns': $duration_ns,
    'git_sha': '$(git rev-parse --short HEAD 2>/dev/null || echo unknown)',
    'hostname': '$(hostname)',
    'arch': '$(uname -m)',
    'os': 'darwin',
    'total_recorded': $recorded,
    'throughput_msg_s': $thr_msg_s,
    'latency_p50_ns': $p50,
    'latency_p99_ns': $p99,
}, indent=2))
" > "$perf1_dir/metadata.json"

    printf '  %s|%s|recorded=%s|thr=%.0f msg/s|p50=%s ns|p99=%s ns|dur=%d ms\n' \
        "$system" "$run_label" "$recorded" "$thr_msg_s" "$p50" "$p99" "$(( duration_ns / 1000000 ))" >&2
}

# ---------------------------------------------------------------------------
# Stop eKuiper rule to avoid interference during WAFER/native runs
# ---------------------------------------------------------------------------
_stop_ekuiper_rule() {
    curl -sf -X POST "http://127.0.0.1:9081/rules/pipeline_a/stop" >/dev/null 2>&1 || true
}
_start_ekuiper_rule() {
    curl -sf -X POST "http://127.0.0.1:9081/rules/pipeline_a/start" >/dev/null 2>&1 || true
    sleep 1
}

# ---------------------------------------------------------------------------
# Main loop
# ---------------------------------------------------------------------------
IFS=',' read -ra sys_arr <<< "$systems"
for system in "${sys_arr[@]}"; do
    _log "=== $system (${runs} runs) ==="

    if [ "$system" = "ekuiper" ]; then
        _start_ekuiper_rule
    else
        _stop_ekuiper_rule
    fi
    sleep 1

    for i in $(seq 1 "$runs"); do
        _run_one "$system" "$i"
        sleep 0.5
    done
done

# Ensure eKuiper is running again at the end
_start_ekuiper_rule

# ---------------------------------------------------------------------------
# Roll-up manifests
# ---------------------------------------------------------------------------
for exp in "e-perf-1:$PERF1_ROOT" "e-perf-2:$PERF2_ROOT"; do
    IFS=':' read -r exp_id exp_root <<< "$exp"
    python3 -c "
import json
print(json.dumps({
    'experiment': '$exp_id',
    'shakedown': True,
    'host_tag': 'shakedown-macos',
    'generated_at': '$(date -u +%Y-%m-%dT%H:%M:%SZ)',
    'git_sha': '$(git rev-parse --short HEAD 2>/dev/null || echo unknown)',
    'hostname': '$(hostname)',
    'arch': '$(uname -m)',
    'os': 'darwin',
    'systems': $(python3 -c "import json;print(json.dumps('${systems}'.split(',')))" 2>/dev/null),
    'runs_per_system': $runs,
    'rate_msg_s': $RATE,
    'duration_secs': $DURATION_SECS,
    'total_messages': $TOTAL_MESSAGES,
}, indent=2))
" > "$exp_root/shakedown.json"
done

_log "shakedown complete"
_log "  E-Perf-1: $PERF1_ROOT"
_log "  E-Perf-2: $PERF2_ROOT"
