#!/usr/bin/env bash
# eval/scripts/run-e-perf-6-8-shakedown.sh
#
# Drives E-Perf-6 (RSS scaling) and E-Perf-8 (latency scaling) shakedowns on
# macOS: linear pipelines of depth 1/3/5/10 × N runs (default 30).
#
# Produces TWO result hierarchies from the same underlying runs:
#   eval/results/e-perf-6/shakedown-macos-<ts>/{depth-1,..,depth-10}/run-NN/
#   eval/results/e-perf-8/shakedown-macos-<ts>/{depth-1,..,depth-10}/run-NN/
#
# E-Perf-6 adds a background `ps` sampler recording RSS/VSZ at 1 Hz into each
# run's memory.csv. macOS ps reports RSS in KiB. The sampler starts before the
# runtime and stops after exit, giving a full lifecycle view including startup.
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

_log() { printf '[shakedown] %s\n' "$*" >&2; }

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
if [ "$skip_build" -eq 0 ]; then
    _log "building wafer-runtime (release)"
    cargo build --release -p wafer-runtime >&2
fi
[ -x "$WAFER_BIN" ] || { _log "wafer binary missing at $WAFER_BIN"; exit 3; }

# ---------------------------------------------------------------------------
# Output roots
# ---------------------------------------------------------------------------

TS="$(date -u +'%Y-%m-%dT%H-%M-%SZ')"
OUT_6="$REPO_ROOT/eval/results/e-perf-6/shakedown-macos-$TS"
OUT_8="$REPO_ROOT/eval/results/e-perf-8/shakedown-macos-$TS"
mkdir -p "$OUT_6" "$OUT_8"

_log "E-Perf-6 output: $OUT_6"
_log "E-Perf-8 output: $OUT_8"
_log "runs per depth: $runs | depths: $depths"

if [ "$dry_run" -eq 1 ]; then _log "DRY RUN"; exit 0; fi

# ---------------------------------------------------------------------------
# Per-run driver
# ---------------------------------------------------------------------------

_run_one() {
    local depth=$1 run_idx=$2
    local depth_label="depth-$depth"
    local run_label; run_label="run-$(printf '%02d' "$run_idx")"
    local out_6="$OUT_6/$depth_label/$run_label"
    local out_8="$OUT_8/$depth_label/$run_label"
    local cfg="$REPO_ROOT/eval/configs/e-perf-6/pipeline-depth-${depth}.toml"

    [ -f "$cfg" ] || { _log "config missing: $cfg"; return 1; }
    mkdir -p "$out_6" "$out_8"
    cp "$cfg" "$out_6/config.toml"
    cp "$cfg" "$out_8/config.toml"

    local cfg_sha; cfg_sha="$(_sha256 "$cfg")"
    local started_ns; started_ns=$(_now_ns)
    local started_at; started_at="$(date -u +'%Y-%m-%dT%H:%M:%SZ')"

    # Launch runtime in background
    WAFER_BENCH_OUTPUT_DIR="$out_6" "$WAFER_BIN" --config "$cfg" \
        >"$out_6/stdout.log" 2>&1 &
    local wafer_pid=$!

    # Memory sampler: 1 Hz ps polling in background subshell
    printf 'sample_ns,rss_kb,vsz_kb\n' > "$out_6/memory.csv"
    (
        while kill -0 "$wafer_pid" 2>/dev/null; do
            ns=$(python3 -c 'import time; print(int(time.time()*1e9))')
            mem=$(ps -o rss=,vsz= -p "$wafer_pid" 2>/dev/null | tr -s ' ')
            if [ -n "$mem" ]; then
                rss=$(echo "$mem" | awk '{print $1}')
                vsz=$(echo "$mem" | awk '{print $2}')
                printf '%s,%s,%s\n' "$ns" "$rss" "$vsz" >> "$out_6/memory.csv"
            fi
            sleep 1
        done
    ) &
    local sampler_pid=$!

    # Wait for runtime to finish
    local runtime_exit=0
    wait "$wafer_pid" || runtime_exit=$?

    # Stop sampler gracefully
    kill "$sampler_pid" 2>/dev/null || true
    wait "$sampler_pid" 2>/dev/null || true

    local finished_ns; finished_ns=$(_now_ns)
    local duration_ns=$(( finished_ns - started_ns ))
    local finished_at; finished_at="$(date -u +'%Y-%m-%dT%H:%M:%SZ')"

    # Copy artefacts to e-perf-8 dir
    for f in latency.hdr throughput.csv sequence.csv stdout.log; do
        [ -f "$out_6/$f" ] && cp "$out_6/$f" "$out_8/$f"
    done

    # Write metadata
    python3 -c "
import json, sys
print(json.dumps({
    'experiment': 'e-perf-6-8',
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
    'exit_codes': {'wafer_runtime': $runtime_exit},
}, indent=2))
" > "$out_6/metadata.json"
    cp "$out_6/metadata.json" "$out_8/metadata.json"

    local traps
    traps=$(grep -c "unrecoverable error" "$out_6/stdout.log" 2>/dev/null || true)
    traps=${traps:-0}
    printf 'depth-%s|%s|exit=%s|traps=%s|dur=%dms\n' \
        "$depth" "$run_label" "$runtime_exit" "$traps" "$(( duration_ns / 1000000 ))" >&2
    if [ "$runtime_exit" -ne 0 ] || [ "$traps" -ne 0 ]; then
        _log "  ↳ non-clean run — investigate $out_6/stdout.log"
    fi
}

# ---------------------------------------------------------------------------
# Main loop
# ---------------------------------------------------------------------------

IFS=',' read -ra depths_arr <<< "$depths"
for depth in "${depths_arr[@]}"; do
    _log "=== depth $depth ==="
    mkdir -p "$OUT_6/depth-$depth" "$OUT_8/depth-$depth"
    for i in $(seq 1 "$runs"); do
        _run_one "$depth" "$i"
    done

    # Emit depth-summary.json
    for out_root in "$OUT_6" "$OUT_8"; do
        python3 - "$out_root/depth-$depth" "$runs" "$depth" <<'PY' > "$out_root/depth-$depth/depth-summary.json"
import json, os, sys
root, runs, depth = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
inv = []
for i in range(1, runs + 1):
    rdir = os.path.join(root, f"run-{i:02d}")
    hdr = os.path.join(rdir, "latency.hdr")
    if not os.path.isfile(hdr):
        inv.append({"run": i, "ok": False, "reason": "missing latency.hdr"})
        continue
    recorded = 0
    with open(hdr) as f:
        for line in f:
            if line.startswith("#Recorded values:"):
                recorded = int(line.split(":")[1].strip())
            if not line.startswith("#"):
                break
    mem = os.path.join(rdir, "memory.csv")
    has_mem = os.path.isfile(mem) and os.path.getsize(mem) > 30
    inv.append({"run": i, "ok": recorded > 0, "recorded_values": recorded, "has_memory_csv": has_mem})
ok = sum(1 for r in inv if r.get("ok"))
print(json.dumps({"depth": depth, "runs_requested": runs, "runs_ok": ok, "per_run": inv}, indent=2))
PY
    done
done

# ---------------------------------------------------------------------------
# Roll-up manifests
# ---------------------------------------------------------------------------

GIT_SHA="$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
for out_root in "$OUT_6" "$OUT_8"; do
    exp="e-perf-6"; [ "$out_root" = "$OUT_8" ] && exp="e-perf-8"
    python3 -c "
import json
print(json.dumps({
    'experiment': '$exp',
    'shakedown': True,
    'host_tag': 'shakedown-macos',
    'generated_at': '$(date -u +%Y-%m-%dT%H:%M:%SZ)',
    'git_sha': '$GIT_SHA',
    'hostname': '$(hostname)',
    'arch': '$(uname -m)',
    'os': '$(uname -s | tr A-Z a-z)',
    'rustc': '$(rustc --version 2>/dev/null | head -1)',
    'runs_per_depth': $runs,
    'depths': [$(echo "${depths_arr[@]}" | tr ' ' ',')],
}, indent=2))
" > "$out_root/shakedown.json"
done

_log "shakedown complete"
_log "E-Perf-6 (RSS): $OUT_6"
_log "E-Perf-8 (latency): $OUT_8"
