#!/usr/bin/env bash
# eval/scripts/run-e-perf-3-shakedown.sh
#
# E-Perf-3: per-hop overhead with MQTT bookends. Compares E2E latency
# (MQTT-source → N×pass-through → MQTT-sink) across 4 depths to isolate
# MQTT bookend cost vs E-Perf-8 in-process baseline.
#
# Layout: eval/results/e-perf-3/shakedown-macos-<ts>/depth-{1,3,5,10}/run-NN/
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

# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

runs=30
depths="1,3,5,10"
skip_build=0
dry_run=0

while [ $# -gt 0 ]; do
    case "$1" in
        --runs)       runs="${2:?}"; shift 2 ;;
        --depths)     depths="${2:?}"; shift 2 ;;
        --skip-build) skip_build=1; shift ;;
        --dry-run)    dry_run=1; shift ;;
        -h|--help)    echo "Usage: $0 [--runs N] [--depths 1,3,5,10] [--skip-build] [--dry-run]"; exit 0 ;;
        *) printf 'unknown flag: %s\n' "$1" >&2; exit 2 ;;
    esac
done

[ "$runs" -lt 30 ] && printf 'WARN: --runs=%s < 30 (AC requires ≥30).\n' "$runs" >&2

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

_log() { printf '[e-perf-3] %s\n' "$*" >&2; }

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
[ -x "$WAFER_BIN" ]    || { _log "wafer binary missing at $WAFER_BIN"; exit 3; }
[ -x "$LOADGEN_BIN" ]  || { _log "wafer-loadgen binary missing at $LOADGEN_BIN"; exit 3; }

# ---------------------------------------------------------------------------
# Docker / mosquitto lifecycle
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

# Kill any wafer-runtime child + stop mosquitto on Ctrl-C or unexpected exit.
# Without this, an interrupt during a run leaves both the runtime and the
# broker alive on their ports.
_wafer_pids=()
_cleanup_perf3() {
    local rc=$?
    for p in "${_wafer_pids[@]:-}"; do
        [ -n "$p" ] && kill -TERM "$p" 2>/dev/null || true
    done
    _stop_mosquitto
    exit "$rc"
}
trap _cleanup_perf3 EXIT INT TERM

_start_mosquitto

# ---------------------------------------------------------------------------
# Output root
# ---------------------------------------------------------------------------

TS="$(date -u +'%Y-%m-%dT%H-%M-%SZ')"
OUT_ROOT="$REPO_ROOT/eval/results/e-perf-3/shakedown-macos-$TS"
mkdir -p "$OUT_ROOT"

_log "output root: $OUT_ROOT"
_log "runs per depth: $runs | depths: $depths"

if [ "$dry_run" -eq 1 ]; then _log "DRY RUN"; exit 0; fi

# ---------------------------------------------------------------------------
# Per-run driver
# ---------------------------------------------------------------------------

LOADGEN_PROFILE="$REPO_ROOT/eval/loadgen/e-perf-3-shakedown.toml"
TOTAL_MESSAGES=5000

_run_one() {
    local depth=$1 run_idx=$2
    local depth_label="depth-$depth"
    local run_label; run_label="run-$(printf '%02d' "$run_idx")"
    local out_dir="$OUT_ROOT/$depth_label/$run_label"
    local cfg="$REPO_ROOT/eval/configs/e-perf-3/pipeline-mqtt-depth-${depth}.toml"

    [ -f "$cfg" ] || { _log "config missing: $cfg"; return 1; }
    mkdir -p "$out_dir"
    cp "$cfg" "$out_dir/config.toml"

    local cfg_sha; cfg_sha="$(_sha256 "$cfg")"
    local started_ns; started_ns=$(_now_ns)
    local started_at; started_at="$(date -u +'%Y-%m-%dT%H:%M:%SZ')"

    # Launch runtime
    "$WAFER_BIN" --config "$cfg" --no-api \
        >"$out_dir/stdout.log" 2>&1 &
    local wafer_pid=$!
    _wafer_pids+=("$wafer_pid")
    sleep 1

    if ! kill -0 "$wafer_pid" 2>/dev/null; then
        _log "runtime died on startup — see $out_dir/stdout.log"
        local finished_ns; finished_ns=$(_now_ns)
        _write_metadata "$out_dir" "$cfg" "$cfg_sha" "$started_at" "$started_ns" "$finished_ns" "$depth" "$run_idx" 1
        return 1
    fi

    # Launch subscriber (records latency.hdr)
    "$LOADGEN_BIN" subscribe \
        --broker "localhost:1883" \
        --topic "wafer/bench/output" \
        --output-dir "$out_dir" \
        --total-messages "$TOTAL_MESSAGES" \
        >>"$out_dir/stdout.log" 2>&1 &
    local sub_pid=$!
    sleep 0.3

    # Launch publisher
    "$LOADGEN_BIN" publish \
        --broker-host "localhost" \
        --broker-port 1883 \
        --topic "wafer/bench/input" \
        --profile-file "$LOADGEN_PROFILE" \
        --rate 1000 \
        --duration-secs 10 \
        --payload-template telemetry-120b \
        >>"$out_dir/stdout.log" 2>&1 &
    local pub_pid=$!

    # Wait for subscriber to finish (it exits after total-messages) or timeout
    local deadline=$(( $(date +%s) + 30 ))
    while kill -0 "$sub_pid" 2>/dev/null; do
        if [ "$(date +%s)" -ge "$deadline" ]; then
            _log "  timeout waiting for subscriber"
            break
        fi
        sleep 0.5
    done

    # Cleanup
    kill "$pub_pid" 2>/dev/null || true
    kill "$sub_pid" 2>/dev/null || true
    kill -TERM "$wafer_pid" 2>/dev/null || true
    wait "$pub_pid" 2>/dev/null || true
    wait "$sub_pid" 2>/dev/null || true

    local runtime_exit=0
    wait "$wafer_pid" 2>/dev/null || runtime_exit=$?

    local finished_ns; finished_ns=$(_now_ns)
    local finished_at; finished_at="$(date -u +'%Y-%m-%dT%H:%M:%SZ')"
    local duration_ns=$(( finished_ns - started_ns ))

    _write_metadata "$out_dir" "$cfg" "$cfg_sha" "$started_at" "$started_ns" "$finished_ns" "$depth" "$run_idx" "$runtime_exit"

    local traps
    traps=$(grep -c "unrecoverable error" "$out_dir/stdout.log" 2>/dev/null || true)
    traps=${traps:-0}
    printf 'depth-%s|%s|exit=%s|traps=%s|dur=%dms\n' \
        "$depth" "$run_label" "$runtime_exit" "$traps" "$(( duration_ns / 1000000 ))" >&2
}

_write_metadata() {
    local out_dir=$1 cfg=$2 cfg_sha=$3 started_at=$4
    local started_ns=$5 finished_ns=$6 depth=$7 run_idx=$8 runtime_exit=$9
    local duration_ns=$(( finished_ns - started_ns ))
    local finished_at; finished_at="$(date -u +'%Y-%m-%dT%H:%M:%SZ')"

    python3 -c "
import json
print(json.dumps({
    'experiment': 'e-perf-3',
    'host_tag': 'shakedown-macos',
    'depth': $depth,
    'run_index': $run_idx,
    'generated_at': '$finished_at',
    'started_at': '$started_at',
    'finished_at': '$finished_at',
    'duration_ns': $duration_ns,
    'git_sha': '$(git rev-parse --short HEAD 2>/dev/null || echo unknown)',
    'hostname': '$(hostname)',
    'kernel': '$(uname -r)',
    'arch': '$(uname -m)',
    'os': '$(uname -s | tr A-Z a-z)',
    'rustc': '$(rustc --version 2>/dev/null | head -1)',
    'config_path': '$cfg',
    'config_sha256': '$cfg_sha',
    'mqtt_bookend': True,
    'exit_codes': {'wafer_runtime': $runtime_exit},
}, indent=2))
" > "$out_dir/metadata.json"
}

# ---------------------------------------------------------------------------
# Main loop
# ---------------------------------------------------------------------------

IFS=',' read -ra depths_arr <<< "$depths"
for depth in "${depths_arr[@]}"; do
    _log "=== depth $depth ==="
    mkdir -p "$OUT_ROOT/depth-$depth"
    for i in $(seq 1 "$runs"); do
        _run_one "$depth" "$i"
    done

    # Emit depth-summary.json (uses subscriber-metadata.json for MQTT runs)
    python3 - "$OUT_ROOT/depth-$depth" "$runs" "$depth" <<'PY' > "$OUT_ROOT/depth-$depth/depth-summary.json"
import json, os, sys
root, runs, depth = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
inv = []
for i in range(1, runs + 1):
    rdir = os.path.join(root, f"run-{i:02d}")
    meta = os.path.join(rdir, "subscriber-metadata.json")
    if not os.path.isfile(meta):
        inv.append({"run": i, "ok": False, "reason": "missing subscriber-metadata.json"})
        continue
    with open(meta) as f:
        d = json.load(f)
    recorded = d.get("total_recorded", 0)
    inv.append({
        "run": i,
        "ok": recorded > 0,
        "total_recorded": recorded,
        "p50_ns": d.get("latency_p50_ns"),
        "p95_ns": d.get("latency_p95_ns"),
        "p99_ns": d.get("latency_p99_ns"),
        "p999_ns": d.get("latency_p999_ns"),
        "mean_ns": d.get("latency_mean_ns"),
    })
ok = sum(1 for r in inv if r.get("ok"))
print(json.dumps({"depth": depth, "runs_requested": runs, "runs_ok": ok, "per_run": inv}, indent=2))
PY
done

# ---------------------------------------------------------------------------
# Roll-up manifest
# ---------------------------------------------------------------------------

python3 -c "
import json
print(json.dumps({
    'experiment': 'e-perf-3',
    'shakedown': True,
    'host_tag': 'shakedown-macos',
    'generated_at': '$(date -u +%Y-%m-%dT%H:%M:%SZ)',
    'git_sha': '$(git rev-parse --short HEAD 2>/dev/null || echo unknown)',
    'hostname': '$(hostname)',
    'arch': '$(uname -m)',
    'os': '$(uname -s | tr A-Z a-z)',
    'rustc': '$(rustc --version 2>/dev/null | head -1)',
    'runs_per_depth': $runs,
    'depths': [$(echo "${depths_arr[@]}" | tr ' ' ',')],
    'mqtt_bookend': True,
    'notes': 'E-Perf-3 shakedown: MQTT source/sink bookend overhead isolation. Compare per-hop cost with E-Perf-8 (in-process) to extract bookend tax.',
}, indent=2))
" > "$OUT_ROOT/shakedown.json"

_log "shakedown complete: $OUT_ROOT"
