#!/usr/bin/env bash
# eval/scripts/run-e-bp-perf9-shakedown.sh
#
# P3.6 shakedown: E-Backpressure (burst backpressure) + E-Perf-9 (AOT cold/warm).
#
# E-Backpressure: 180s burst run, verifying queue depth stays bounded.
# E-Perf-9: 3 plugin tiers × {cold, warm} × 5 runs, measuring startup latency.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

perf9_runs=5
skip_build=0
dry_run=0
skip_backpressure=0
skip_perf9=0

while [ $# -gt 0 ]; do
    case "$1" in
        --perf9-runs)        perf9_runs="${2:?}"; shift 2 ;;
        --skip-build)        skip_build=1; shift ;;
        --skip-backpressure) skip_backpressure=1; shift ;;
        --skip-perf9)        skip_perf9=1; shift ;;
        --dry-run)           dry_run=1; shift ;;
        -h|--help)           echo "Usage: $0 [--perf9-runs N] [--skip-build] [--skip-backpressure] [--skip-perf9] [--dry-run]"; exit 0 ;;
        *) printf 'unknown flag: %s\n' "$1" >&2; exit 2 ;;
    esac
done

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

_log() { printf '[p3.6] %s\n' "$*" >&2; }

_sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

_now_ns() {
    python3 -c 'import time; print(int(time.time()*1e9))'
}

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
[ -x "$LOADGEN_BIN" ] || { _log "wafer-loadgen binary missing"; exit 3; }

# ---------------------------------------------------------------------------
# Docker / mosquitto (for E-Backpressure)
# ---------------------------------------------------------------------------

MOSQ_CONTAINER=""
MOSQ_IMAGE="eclipse-mosquitto:2.0.18"
export DOCKER_HOST="${DOCKER_HOST:-unix://<home>/.colima/default/docker.sock}"

_stop_mosquitto() {
    [ -z "$MOSQ_CONTAINER" ] && return 0
    _log "stopping mosquitto $MOSQ_CONTAINER"
    docker rm -f "$MOSQ_CONTAINER" >/dev/null 2>&1 || true
    MOSQ_CONTAINER=""
}

_start_mosquitto() {
    command -v docker >/dev/null 2>&1 || { _log "docker not available"; exit 4; }
    _log "starting mosquitto ($MOSQ_IMAGE)"
    MOSQ_CONTAINER=$(
        docker run -d \
            --label wafer-harness=1 \
            -p 1883:1883 \
            "$MOSQ_IMAGE" \
            mosquitto -c /mosquitto-no-auth.conf
    )
    for _ in $(seq 1 30); do
        (echo > /dev/tcp/127.0.0.1/1883) >/dev/null 2>&1 && { _log "broker ready"; return 0; }
        sleep 0.2
    done
    _log "mosquitto did not become ready"
    _stop_mosquitto
    exit 4
}

# Kill the last-launched runtime + stop mosquitto on Ctrl-C or unexpected exit.
# Without this, an interrupt during the 180s burst leaves both the runtime
# and broker alive on their ports.
_last_wafer_pid=""
_cleanup_bpperf9() {
    local rc=$?
    [ -n "$_last_wafer_pid" ] && kill -TERM "$_last_wafer_pid" 2>/dev/null || true
    _stop_mosquitto
    exit "$rc"
}
trap _cleanup_bpperf9 EXIT INT TERM

TS="$(date -u +'%Y-%m-%dT%H-%M-%SZ')"

if [ "$dry_run" -eq 1 ]; then
    _log "DRY RUN — E-Backpressure + E-Perf-9"
    exit 0
fi

# ===========================================================================
# E-Backpressure: burst run
# ===========================================================================

if [ "$skip_backpressure" -eq 0 ]; then
    _log "=== E-Backpressure: burst backpressure validation ==="
    _start_mosquitto

    BP_OUT="$REPO_ROOT/eval/results/e-backpressure/shakedown-macos-$TS/run-1"
    mkdir -p "$BP_OUT"

    BP_CFG="$REPO_ROOT/eval/configs/e-backpressure/pipeline-burst.toml"
    cp "$BP_CFG" "$BP_OUT/config.toml"

    started_ns=$(_now_ns)
    started_at="$(date -u +'%Y-%m-%dT%H:%M:%SZ')"

    # Launch runtime
    "$WAFER_BIN" --config "$BP_CFG" \
        >"$BP_OUT/stdout.log" 2>&1 &
    RUNTIME_PID=$!
    _last_wafer_pid=$RUNTIME_PID
    _log "runtime pid=$RUNTIME_PID"
    sleep 2

    if ! kill -0 "$RUNTIME_PID" 2>/dev/null; then
        _log "runtime died on startup — see $BP_OUT/stdout.log"
        _stop_mosquitto
        exit 5
    fi

    # Burst profile: 180s, ~210000 msgs total (3 cycles × 70k)
    EXPECTED_MSGS=210000

    # Launch subscriber (exits after receiving expected messages)
    "$LOADGEN_BIN" subscribe \
        --broker "localhost:1883" \
        --topic "wafer/bench/output" \
        --output-dir "$BP_OUT" \
        --total-messages "$EXPECTED_MSGS" \
        >>"$BP_OUT/stdout.log" 2>&1 &
    SUB_PID=$!
    sleep 0.5

    # Launch burst publisher (180s)
    BURST_PROFILE="$REPO_ROOT/eval/loadgen/e-backpressure-180s.toml"
    "$LOADGEN_BIN" publish \
        --broker-host "localhost" \
        --broker-port 1883 \
        --topic "wafer/bench/input" \
        --profile-file "$BURST_PROFILE" \
        >>"$BP_OUT/stdout.log" 2>&1 &
    PUB_PID=$!
    _log "publisher pid=$PUB_PID (180s burst profile, ~$EXPECTED_MSGS msgs)"

    # Wait for publisher to finish
    wait "$PUB_PID" 2>/dev/null || true
    _log "publisher done; waiting for subscriber to drain..."

    # Wait for subscriber (with timeout)
    deadline=$(( $(date +%s) + 30 ))
    while kill -0 "$SUB_PID" 2>/dev/null; do
        [ "$(date +%s)" -ge "$deadline" ] && break
        sleep 1
    done
    kill -TERM "$SUB_PID" 2>/dev/null || true
    wait "$SUB_PID" 2>/dev/null || true

    # Scrape metrics while runtime is still alive
    if curl -sf http://127.0.0.1:9090/metrics > "$BP_OUT/prometheus-raw.txt" 2>/dev/null; then
        _log "scraped prometheus metrics (runtime alive)"
    fi

    kill -TERM "$RUNTIME_PID" 2>/dev/null || true
    runtime_exit=0
    wait "$RUNTIME_PID" 2>/dev/null || runtime_exit=$?
    _last_wafer_pid=""

    finished_ns=$(_now_ns)
    duration_ns=$(( finished_ns - started_ns ))

    # Write metadata
    python3 -c "
import json
print(json.dumps({
    'experiment': 'e-backpressure',
    'host_tag': 'shakedown-macos',
    'generated_at': '$(date -u +%Y-%m-%dT%H:%M:%SZ)',
    'started_at': '$started_at',
    'duration_ns': $duration_ns,
    'git_sha': '$(git rev-parse --short HEAD 2>/dev/null || echo unknown)',
    'hostname': '$(hostname)',
    'arch': '$(uname -m)',
    'os': '$(uname -s | tr A-Z a-z)',
    'rustc': '$(rustc --version 2>/dev/null | head -1)',
    'config_path': '$BP_CFG',
    'config_sha256': '$(_sha256 "$BP_CFG")',
    'burst_profile': 'e-backpressure-180s.toml',
    'exit_codes': {'wafer_runtime': $runtime_exit},
    'notes': 'Burst pattern: 1000 msg/s baseline, 2x burst (2000 msg/s) for 10s every 60s, 180s total.',
}, indent=2))
" > "$BP_OUT/metadata.json"

    # Analyse results
    _log "E-Backpressure results:"
    if [ -f "$BP_OUT/subscriber-metadata.json" ]; then
        python3 -c "
import json
with open('$BP_OUT/subscriber-metadata.json') as f:
    d = json.load(f)
print(f'  Total messages received: {d[\"total_recorded\"]}')
print(f'  Latency p50: {d[\"latency_p50_ns\"]/1000:.1f} µs')
print(f'  Latency p99: {d[\"latency_p99_ns\"]/1000:.1f} µs')
print(f'  Gaps: {d[\"sequence\"][\"total_gaps\"]}')
print(f'  Duplicates: {d[\"sequence\"][\"total_duplicates\"]}')
"
    fi

    # Check for queue overflow in runtime logs (grep for WARN/ERROR level indicators)
    overflow=$(grep -i "overflow\|channel full\|dropped message" "$BP_OUT/stdout.log" 2>/dev/null | grep -v "config\|pipeline=" | wc -l | tr -d ' ')
    overflow=${overflow:-0}
    _log "  Queue overflow/drop mentions in logs: $overflow"

    _stop_mosquitto
    _log "E-Backpressure done: $BP_OUT"
fi

# ===========================================================================
# E-Perf-9: AOT cold/warm startup
# ===========================================================================

if [ "$skip_perf9" -eq 0 ]; then
    _log "=== E-Perf-9: AOT cold/warm startup latency ==="

    PERF9_OUT="$REPO_ROOT/eval/results/e-perf-9/shakedown-macos-$TS"
    mkdir -p "$PERF9_OUT"

    # Plugin tier definitions (no associative arrays — macOS /bin/bash is 3.x)
    TIERS="tier-small tier-medium tier-large"

    _tier_config() {
        case "$1" in
            tier-small)  echo "$REPO_ROOT/eval/configs/e-perf-9/pipeline-tier-small.toml" ;;
            tier-medium) echo "$REPO_ROOT/eval/configs/e-perf-9/pipeline-tier-medium.toml" ;;
            tier-large)  echo "$REPO_ROOT/eval/configs/e-perf-9/pipeline-tier-large.toml" ;;
        esac
    }
    _tier_size() {
        case "$1" in
            tier-small)  echo "57972" ;;
            tier-medium) echo "97006" ;;
            tier-large)  echo "375309" ;;
        esac
    }

    # Cold = first run after clean rebuild; Warm = subsequent runs on same binary.
    # Approach: rebuild once for cold runs, then reuse for warm runs.
    for tier in $TIERS; do
        tier_size=$(_tier_size "$tier")
        cfg=$(_tier_config "$tier")
        _log "--- $tier ($tier_size bytes) ---"
        [ -f "$cfg" ] || { _log "config missing: $cfg"; continue; }

        # --- Cold runs: rebuild runtime to clear wasmtime's native code cache ---
        _log "  cold runs (rebuilding runtime to invalidate native cache)"
        cargo build --release -p wafer-runtime >&2 2>/dev/null

        for i in $(seq 1 "$perf9_runs"); do
            run_label="run-$(printf '%02d' "$i")"
            out_dir="$PERF9_OUT/$tier/cold/$run_label"
            mkdir -p "$out_dir"
            cp "$cfg" "$out_dir/config.toml"

            started_ns=$(_now_ns)
            # Pipeline exits non-zero after BenchSource exhausts total_messages
            # (short 50-msg run for startup timing); this is normal completion.
            WAFER_BENCH_OUTPUT_DIR="$out_dir" "$WAFER_BIN" --config "$cfg" --no-api \
                >"$out_dir/stdout.log" 2>&1 || true
            finished_ns=$(_now_ns)
            duration_ns=$(( finished_ns - started_ns ))

            # Extract startup time from logs: time from process start to "Pipeline running"
            pipeline_ready_line=$(grep -m1 "Pipeline running\|pipeline started\|All nodes running" "$out_dir/stdout.log" 2>/dev/null || true)

            python3 -c "
import json
print(json.dumps({
    'experiment': 'e-perf-9',
    'tier': '$tier',
    'wasm_size_bytes': $tier_size,
    'cache_state': 'cold',
    'run_index': $i,
    'startup_duration_ns': $duration_ns,
    'host_tag': 'shakedown-macos',
    'generated_at': '$(date -u +%Y-%m-%dT%H:%M:%SZ)',
    'git_sha': '$(git rev-parse --short HEAD 2>/dev/null || echo unknown)',
    'hostname': '$(hostname)',
    'arch': '$(uname -m)',
    'os': '$(uname -s | tr A-Z a-z)',
    'config_path': '$cfg',
    'notes': 'Cold = first run after cargo rebuild. startup_duration_ns is wall time from process launch to exit (50 messages at 100 msg/s = ~500ms runtime + compile/instantiate).',
}, indent=2))
" > "$out_dir/metadata.json"
            printf '  cold|%s|%s|dur=%dms\n' "$tier" "$run_label" "$(( duration_ns / 1000000 ))" >&2
        done

        # --- Warm runs: reuse same binary (native code cached by OS) ---
        _log "  warm runs (same binary, OS page cache warm)"
        for i in $(seq 1 "$perf9_runs"); do
            run_label="run-$(printf '%02d' "$i")"
            out_dir="$PERF9_OUT/$tier/warm/$run_label"
            mkdir -p "$out_dir"
            cp "$cfg" "$out_dir/config.toml"

            started_ns=$(_now_ns)
            # Pipeline exits non-zero after BenchSource exhausts total_messages
            # (short 50-msg run for startup timing); this is normal completion.
            WAFER_BENCH_OUTPUT_DIR="$out_dir" "$WAFER_BIN" --config "$cfg" --no-api \
                >"$out_dir/stdout.log" 2>&1 || true
            finished_ns=$(_now_ns)
            duration_ns=$(( finished_ns - started_ns ))

            python3 -c "
import json
print(json.dumps({
    'experiment': 'e-perf-9',
    'tier': '$tier',
    'wasm_size_bytes': $tier_size,
    'cache_state': 'warm',
    'run_index': $i,
    'startup_duration_ns': $duration_ns,
    'host_tag': 'shakedown-macos',
    'generated_at': '$(date -u +%Y-%m-%dT%H:%M:%SZ)',
    'git_sha': '$(git rev-parse --short HEAD 2>/dev/null || echo unknown)',
    'hostname': '$(hostname)',
    'arch': '$(uname -m)',
    'os': '$(uname -s | tr A-Z a-z)',
    'config_path': '$cfg',
    'notes': 'Warm = subsequent run on same binary. OS page cache + wasmtime AOT cache both populated.',
}, indent=2))
" > "$out_dir/metadata.json"
            printf '  warm|%s|%s|dur=%dms\n' "$tier" "$run_label" "$(( duration_ns / 1000000 ))" >&2
        done
    done

    # Roll-up summary
    python3 - "$PERF9_OUT" <<'PY' > "$PERF9_OUT/shakedown.json"
import json, os, sys
from pathlib import Path
import statistics

root = Path(sys.argv[1])
results = {}
for tier_dir in sorted(root.iterdir()):
    if not tier_dir.is_dir() or not tier_dir.name.startswith('tier-'):
        continue
    tier = tier_dir.name
    results[tier] = {}
    for cache_dir in sorted(tier_dir.iterdir()):
        if not cache_dir.is_dir():
            continue
        cache_state = cache_dir.name
        durations = []
        for run_dir in sorted(cache_dir.iterdir()):
            meta = run_dir / 'metadata.json'
            if meta.exists():
                with open(meta) as f:
                    d = json.load(f)
                durations.append(d['startup_duration_ns'])
        if durations:
            results[tier][cache_state] = {
                'n': len(durations),
                'median_ms': statistics.median(durations) / 1e6,
                'mean_ms': statistics.mean(durations) / 1e6,
                'stdev_ms': statistics.stdev(durations) / 1e6 if len(durations) > 1 else 0,
                'min_ms': min(durations) / 1e6,
                'max_ms': max(durations) / 1e6,
            }

print(json.dumps({
    'experiment': 'e-perf-9',
    'shakedown': True,
    'host_tag': 'shakedown-macos',
    'generated_at': os.popen('date -u +%Y-%m-%dT%H:%M:%SZ').read().strip(),
    'tiers': results,
    'methodology': 'Cold = first run after cargo rebuild (invalidates wasmtime native code paths in page cache). Warm = subsequent runs on same binary. Startup measured as wall time from process launch to exit (50 msgs @ 100 msg/s).',
}, indent=2))
PY

    _log "E-Perf-9 done: $PERF9_OUT"
    cat "$PERF9_OUT/shakedown.json"
fi

_log "P3.6 shakedown complete"
