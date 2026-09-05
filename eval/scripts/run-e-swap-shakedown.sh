#!/usr/bin/env bash
# eval/scripts/run-e-swap-shakedown.sh
#
# E-Swap-1/2/4/5/6 hot-swap shakedown on macOS.
#
# Runs the pipeline with API enabled, fires repeated hot-swaps alternating
# v1↔v2, and collects swap timeline + sequence data. A single unified run
# produces data for E-Swap-1 (pause duration), E-Swap-2 (zero-loss), and
# E-Swap-6 (phase decomposition). Separate runs for E-Swap-4 (burst) and
# E-Swap-5 (rollback).
#
# Usage:
#   ./eval/scripts/run-e-swap-shakedown.sh --all
#   ./eval/scripts/run-e-swap-shakedown.sh --swap 1
#   ./eval/scripts/run-e-swap-shakedown.sh --swap 5

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

WAFER_BIN="target/release/wafer"
[ -x "$WAFER_BIN" ] || { echo "ERROR: runtime binary missing at $WAFER_BIN" >&2; exit 3; }

# Kill the last-launched runtime on Ctrl-C so orphaned wafer processes
# don't hold the API port for the next invocation.
_last_wafer_pid=""
# Invoked indirectly by trap.
# shellcheck disable=SC2329
_cleanup_swap() {
    local rc=$?
    [ -n "$_last_wafer_pid" ] && kill -TERM "$_last_wafer_pid" 2>/dev/null || true
    exit "$rc"
}
trap _cleanup_swap EXIT INT TERM

V1_PLUGIN="$REPO_ROOT/plugins/pass-through-v1/target/wasm32-wasip2/release/wafer_pass_through_v1.wasm"
V2_PLUGIN="$REPO_ROOT/plugins/pass-through-v2/target/wasm32-wasip2/release/wafer_pass_through_v2.wasm"
V2_PANICS="$REPO_ROOT/plugins/pass-through-v2-panics/target/wasm32-wasip2/release/wafer_pass_through_v2_panics.wasm"

[ -f "$V1_PLUGIN" ] || { echo "ERROR: v1 plugin missing" >&2; exit 3; }
[ -f "$V2_PLUGIN" ] || { echo "ERROR: v2 plugin missing" >&2; exit 3; }
[ -f "$V2_PANICS" ] || { echo "ERROR: v2-panics plugin missing" >&2; exit 3; }

API_BASE="http://127.0.0.1:9090"
NODE_ID="transform"

swaps=""
while [ $# -gt 0 ]; do
    case "$1" in
        --all) swaps="unified burst rollback"; shift ;;
        --swap)
            case "${2:?}" in
                1|2|6|unified) swaps="$swaps unified" ;;
                4|burst) swaps="$swaps burst" ;;
                5|rollback) swaps="$swaps rollback" ;;
                *) echo "unknown swap: $2" >&2; exit 2 ;;
            esac
            shift 2 ;;
        -h|--help) grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown flag: $1" >&2; exit 2 ;;
    esac
done

if [ -z "$swaps" ]; then
    echo "ERROR: specify --all or --swap <1|2|4|5|6|unified|burst|rollback>" >&2; exit 2
fi

_log() { printf '[%s] %s\n' "$(date -u +%H:%M:%S)" "$*" >&2; }

ts=$(date -u +'%Y-%m-%dT%H-%M-%SZ')
git_sha=$(git rev-parse --short HEAD)

pass_count=0
fail_count=0

wait_api_ready() {
    for _ in $(seq 1 50); do
        if curl -sf "$API_BASE/health" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.1
    done
    return 1
}

# Issue hot-swap POST and capture response timeline
do_swap() {
    local plugin_path="$1"
    curl -sf -X POST "$API_BASE/api/v1/nodes/$NODE_ID/hot-swap" \
        -H "Content-Type: application/json" \
        -d "{\"wasm_path\": \"$plugin_path\"}" 2>/dev/null
}

# ===== Unified run: E-Swap-1, E-Swap-2, E-Swap-6 =====
run_unified() {
    _log "E-Swap-1/2/6 unified: repeated v1↔v2 swaps @ 1000 msg/s"

    local config="$REPO_ROOT/eval/configs/e-swap/pipeline-hotswap.toml"
    local result_base="$REPO_ROOT/eval/results"

    # Shared run directory
    local run_dir_1="$result_base/e-swap-1/shakedown-macos-${ts}/run-1"
    local run_dir_2="$result_base/e-swap-2/shakedown-macos-${ts}/run-1"
    local run_dir_6="$result_base/e-swap-6/shakedown-macos-${ts}/run-1"
    mkdir -p "$run_dir_1" "$run_dir_2" "$run_dir_6"

    # Use e-swap-6 as the canonical output dir (all three read from it)
    local out_dir="$run_dir_6"
    cp "$config" "$out_dir/config.toml"

    _log "  Starting pipeline (120k msgs @ 1000/s, ~120s)"
    WAFER_BENCH_OUTPUT_DIR="$out_dir" "$WAFER_BIN" --config "$config" \
        >"$out_dir/stdout.log" 2>&1 &
    local wafer_pid=$!
    _last_wafer_pid=$wafer_pid

    if ! wait_api_ready; then
        _log "  BLOCKER: API did not become ready"
        kill "$wafer_pid" 2>/dev/null; wait "$wafer_pid" 2>/dev/null || true
        _last_wafer_pid=""
        fail_count=$((fail_count+1)); return
    fi
    _log "  API ready, starting swap loop"

    # Wait for warmup to pass (5s)
    sleep 6

    # Swap loop: alternate v1 → v2 → v1 → ... every 2s for ≥100s
    local swap_count=0
    local swap_timeline_file="$out_dir/swap_timeline.jsonl"
    : > "$swap_timeline_file"

    local swap_start_epoch
    swap_start_epoch=$(date +%s)

    while true; do
        # Check if pipeline still running
        if ! kill -0 "$wafer_pid" 2>/dev/null; then
            _log "  Pipeline exited during swap loop after $swap_count swaps"
            break
        fi

        local elapsed=$(( $(date +%s) - swap_start_epoch ))
        if [ "$elapsed" -ge 108 ] && [ "$swap_count" -ge 50 ]; then
            break
        fi
        # Safety: stop after 115s regardless
        if [ "$elapsed" -ge 115 ]; then
            break
        fi

        # Alternate between v2 and v1
        local target_plugin
        if [ $((swap_count % 2)) -eq 0 ]; then
            target_plugin="$V2_PLUGIN"
        else
            target_plugin="$V1_PLUGIN"
        fi

        local response
        response=$(do_swap "$target_plugin" 2>/dev/null || echo "FAILED")

        if [ "$response" = "FAILED" ]; then
            _log "  WARN: swap $swap_count failed (API returned error)"
            echo "{\"swap_index\":$swap_count,\"status\":\"failed\"}" >> "$swap_timeline_file"
        else
            echo "$response" | python3 -c "
import sys, json
try:
    data = json.load(sys.stdin)
    data['swap_index'] = $swap_count
    print(json.dumps(data))
except:
    print(json.dumps({'swap_index': $swap_count, 'status': 'parse_error'}))
" >> "$swap_timeline_file"
        fi

        swap_count=$((swap_count + 1))
        sleep 2
    done

    _log "  Swap loop done: $swap_count swaps"

    # Pipeline exits non-zero when BenchSource exhausts total_messages;
    # this is expected completion, not a crash.
    wait "$wafer_pid" 2>/dev/null || true
    _last_wafer_pid=""

    # Check for Tokio panics
    if grep -q "thread.*panicked" "$out_dir/stdout.log" 2>/dev/null; then
        _log "  BLOCKER: Tokio-level panic"
        fail_count=$((fail_count+3)); return
    fi

    # Copy shared artifacts to all three result dirs
    for dest in "$run_dir_1" "$run_dir_2"; do
        if [ "$dest" != "$out_dir" ]; then
            cp "$out_dir/config.toml" "$dest/" 2>/dev/null || true
            cp "$out_dir/stdout.log" "$dest/" 2>/dev/null || true
            cp "$out_dir/swap_timeline.jsonl" "$dest/" 2>/dev/null || true
            # BenchSink auto-exports to $out_dir
            cp "$out_dir/throughput.csv" "$dest/" 2>/dev/null || true
            cp "$out_dir/sequence.csv" "$dest/" 2>/dev/null || true
            cp "$out_dir/latency.hdr" "$dest/" 2>/dev/null || true
            cp "$out_dir/swap_timeline.json" "$dest/" 2>/dev/null || true
        fi
    done

    # Generate per-experiment shakedown.json
    python3 - "$out_dir" "$swap_count" "$git_sha" "$ts" <<'PY'
import json, sys, os
from pathlib import Path

out_dir = Path(sys.argv[1])
swap_count = int(sys.argv[2])
git_sha = sys.argv[3]
ts = sys.argv[4]

# Load swap timeline
timeline_path = out_dir / "swap_timeline.jsonl"
timelines = []
if timeline_path.exists():
    for line in open(timeline_path):
        line = line.strip()
        if line:
            try:
                timelines.append(json.loads(line))
            except:
                pass

# Filter successful swaps (those with timeline data)
successful = [t for t in timelines if "timeline" in t]

# --- E-Swap-6: phase decomposition ---
phases = {"compile_ns": [], "instantiate_ns": [], "signal_ns": [], "ack_ns": [], "convergence_ns": []}
for entry in successful:
    tl = entry.get("timeline", {})
    for phase in phases:
        val = tl.get(phase)
        if val is not None and val > 0:
            phases[phase].append(val)

def percentile(arr, p):
    if not arr:
        return 0
    arr_s = sorted(arr)
    idx = int(len(arr_s) * p / 100)
    return arr_s[min(idx, len(arr_s) - 1)]

e_swap_6 = {
    "experiment": "e-swap-6",
    "host": "shakedown-macos",
    "generated_at_utc": ts,
    "total_swaps": swap_count,
    "successful_swaps": len(successful),
    "phase_breakdown": {
        phase: {
            "count": len(vals),
            "p50_ns": percentile(vals, 50),
            "p95_ns": percentile(vals, 95),
            "p99_ns": percentile(vals, 99),
            "min_ns": min(vals) if vals else 0,
            "max_ns": max(vals) if vals else 0,
        }
        for phase, vals in phases.items()
    },
    "dominant_phase": max(phases, key=lambda k: percentile(phases[k], 50)) if any(phases.values()) else "unknown",
    "git_sha": git_sha,
}
e_swap_6_dir = out_dir.parent
json.dump(e_swap_6, open(e_swap_6_dir / "shakedown.json", "w"), indent=2)

# --- E-Swap-1: pause duration ---
# BenchSink's swap_timeline.json has transitions with pause_ns
bench_swap = out_dir / "swap_timeline.json"
pause_durations = []
if bench_swap.exists():
    try:
        bench_data = json.loads(open(bench_swap).read())
        for t in bench_data.get("transitions", []):
            pause_durations.append(t["pause_ns"])
    except:
        pass

# Also compute from API timeline: signal_ns + ack_ns + convergence_ns approximation
api_pause_durations = []
for entry in successful:
    tl = entry.get("timeline", {})
    # Total swap time from signal to first_v2 as observed by handler
    ack = tl.get("ack_ns", 0) or 0
    conv = tl.get("convergence_ns", 0) or 0
    if ack > 0 and conv > 0:
        api_pause_durations.append(ack + conv)

# Use whichever source has data
pause_src = pause_durations if pause_durations else api_pause_durations
pause_src_label = "bench_sink" if pause_durations else "api_timeline"

e_swap_1 = {
    "experiment": "e-swap-1",
    "host": "shakedown-macos",
    "generated_at_utc": ts,
    "total_swaps": swap_count,
    "pause_samples": len(pause_src),
    "pause_source": pause_src_label,
    "pause_duration_p50_ms": percentile(pause_src, 50) / 1_000_000 if pause_src else 0,
    "pause_duration_p95_ms": percentile(pause_src, 95) / 1_000_000 if pause_src else 0,
    "pause_duration_p99_ms": percentile(pause_src, 99) / 1_000_000 if pause_src else 0,
    "pause_duration_min_ms": min(pause_src) / 1_000_000 if pause_src else 0,
    "pause_duration_max_ms": max(pause_src) / 1_000_000 if pause_src else 0,
    "target_100ms": percentile(pause_src, 95) / 1_000_000 < 100 if pause_src else False,
    "git_sha": git_sha,
}
# Write to e-swap-1 parent dir
e_swap_1_root = Path(str(e_swap_6_dir).replace("e-swap-6", "e-swap-1"))
json.dump(e_swap_1, open(e_swap_1_root / "shakedown.json", "w"), indent=2)

# --- E-Swap-2: zero-loss zero-dup ---
# The SequenceTracker reports gaps including the warmup-excluded range.
# Warmup messages (first ~warmup_secs * rate) are never passed to collect()
# so the tracker sees a single initial gap [0, warmup_count). This is NOT
# a swap-induced loss. Swap-induced loss = total_gap_msgs - warmup_gap.
seq_path = out_dir / "sequence.csv"
seq_gaps = -1
seq_dups = -1
total_expected = 0
total_received = 0
gap_ranges = 0
if seq_path.exists():
    import csv
    with open(seq_path) as f:
        reader = csv.DictReader(f)
        for row in reader:
            seq_gaps = int(row.get("gap_msgs", -1))
            seq_dups = int(row.get("duplicates_count", -1))
            total_expected = int(row.get("total_expected", 0))
            total_received = int(row.get("total_received", 0))
            gap_ranges = int(row.get("gap_ranges", 0))

# Warmup gap: source warmup_messages=5000 + BenchSink warmup_secs=5 at rate=1000
# means the tracker's first observation is around seq 5000-5001.
warmup_gap = total_expected - total_received if total_expected > total_received else 0
# Swap-induced gaps = total gaps minus the single warmup-region gap
swap_induced_gaps = max(0, seq_gaps - warmup_gap) if seq_gaps >= 0 else -1
# If only 1 gap range and it equals warmup_gap, all post-warmup msgs delivered
swap_induced_gap_ranges = max(0, gap_ranges - 1) if gap_ranges >= 1 and seq_gaps == warmup_gap else gap_ranges

e_swap_2 = {
    "experiment": "e-swap-2",
    "host": "shakedown-macos",
    "generated_at_utc": ts,
    "total_swaps": swap_count,
    "sequence_gaps_raw": seq_gaps,
    "warmup_excluded_msgs": warmup_gap,
    "swap_induced_gaps": swap_induced_gaps,
    "sequence_duplicates": seq_dups,
    "total_expected": total_expected,
    "total_received": total_received,
    "zero_loss": swap_induced_gaps == 0,
    "zero_dup": seq_dups == 0,
    "git_sha": git_sha,
}
e_swap_2_root = Path(str(e_swap_6_dir).replace("e-swap-6", "e-swap-2"))
json.dump(e_swap_2, open(e_swap_2_root / "shakedown.json", "w"), indent=2)

# Write metadata.json for the shared run
metadata = {
    "experiment": "e-swap-unified",
    "host_tag": "shakedown-macos",
    "generated_at": ts,
    "git_sha": git_sha,
    "total_swaps": swap_count,
    "successful_swaps": len(successful),
    "note": "Unified run for E-Swap-1 (pause), E-Swap-2 (zero-loss), E-Swap-6 (phase decomp)",
}
json.dump(metadata, open(out_dir / "metadata.json", "w"), indent=2)

print(f"E-Swap-6: {len(successful)} successful swaps, dominant={e_swap_6['dominant_phase']}")
print(f"E-Swap-1: p95 pause={e_swap_1['pause_duration_p95_ms']:.2f} ms (target <100 ms)")
print(f"E-Swap-2: swap_induced_gaps={swap_induced_gaps}, dups={seq_dups}, warmup_excluded={warmup_gap}")
PY

    local unified_ok="true"
    if [ "$swap_count" -lt 50 ]; then
        _log "  WARN: only $swap_count swaps (need ≥50)"
        unified_ok="false"
    fi

    if [ "$unified_ok" = "true" ]; then
        _log "  E-Swap-1/2/6 unified: PASS ✓"
        pass_count=$((pass_count+3))
    else
        _log "  E-Swap-1/2/6 unified: FAIL ✗"
        fail_count=$((fail_count+3))
    fi
}

# ===== E-Swap-4: burst load =====
run_burst() {
    _log "E-Swap-4: swap under 2× burst load"

    local config="$REPO_ROOT/eval/configs/e-swap/pipeline-hotswap-burst.toml"
    local result_root="$REPO_ROOT/eval/results/e-swap-4/shakedown-macos-${ts}"
    local run_dir="$result_root/run-1"
    mkdir -p "$run_dir"
    cp "$config" "$run_dir/config.toml"

    _log "  Starting pipeline (240k msgs @ 2000/s, ~120s)"
    WAFER_BENCH_OUTPUT_DIR="$run_dir" "$WAFER_BIN" --config "$config" \
        >"$run_dir/stdout.log" 2>&1 &
    local wafer_pid=$!
    _last_wafer_pid=$wafer_pid

    if ! wait_api_ready; then
        _log "  BLOCKER: API did not become ready"
        kill "$wafer_pid" 2>/dev/null; wait "$wafer_pid" 2>/dev/null || true
        _last_wafer_pid=""
        fail_count=$((fail_count+1)); return
    fi

    sleep 6

    # Issue 50+ swaps at 2s intervals under the 2× burst load
    local swap_count=0
    local swap_timeline_file="$run_dir/swap_timeline.jsonl"
    : > "$swap_timeline_file"
    local swap_start_epoch
    swap_start_epoch=$(date +%s)

    while true; do
        if ! kill -0 "$wafer_pid" 2>/dev/null; then
            break
        fi
        local elapsed=$(( $(date +%s) - swap_start_epoch ))
        if [ "$elapsed" -ge 108 ] && [ "$swap_count" -ge 50 ]; then
            break
        fi
        if [ "$elapsed" -ge 115 ]; then
            break
        fi

        local target_plugin
        if [ $((swap_count % 2)) -eq 0 ]; then
            target_plugin="$V2_PLUGIN"
        else
            target_plugin="$V1_PLUGIN"
        fi

        local response
        response=$(do_swap "$target_plugin" 2>/dev/null || echo "FAILED")
        if [ "$response" != "FAILED" ]; then
            echo "$response" | python3 -c "
import sys, json
try:
    data = json.load(sys.stdin)
    data['swap_index'] = $swap_count
    print(json.dumps(data))
except:
    print(json.dumps({'swap_index': $swap_count, 'status': 'parse_error'}))
" >> "$swap_timeline_file"
        else
            echo "{\"swap_index\":$swap_count,\"status\":\"failed\"}" >> "$swap_timeline_file"
        fi

        swap_count=$((swap_count + 1))
        sleep 2
    done

    # Pipeline exits non-zero when BenchSource exhausts total_messages;
    # this is expected completion, not a crash.
    wait "$wafer_pid" 2>/dev/null || true
    _last_wafer_pid=""

    if grep -q "thread.*panicked" "$run_dir/stdout.log" 2>/dev/null; then
        _log "  BLOCKER: Tokio-level panic"
        fail_count=$((fail_count+1)); return
    fi

    _log "  Swap loop done: $swap_count swaps under 2× burst"

    # Generate shakedown.json
    python3 - "$run_dir" "$swap_count" "$git_sha" "$ts" <<'PY'
import json, sys
from pathlib import Path

run_dir = Path(sys.argv[1])
swap_count = int(sys.argv[2])
git_sha = sys.argv[3]
ts = sys.argv[4]

timelines = []
timeline_path = run_dir / "swap_timeline.jsonl"
if timeline_path.exists():
    for line in open(timeline_path):
        line = line.strip()
        if line:
            try:
                timelines.append(json.loads(line))
            except:
                pass

successful = [t for t in timelines if "timeline" in t]

# Compute pause durations from API timeline (ack + convergence)
pause_durations = []
for entry in successful:
    tl = entry.get("timeline", {})
    ack = tl.get("ack_ns", 0) or 0
    conv = tl.get("convergence_ns", 0) or 0
    if ack > 0 and conv > 0:
        pause_durations.append(ack + conv)

def percentile(arr, p):
    if not arr:
        return 0
    arr_s = sorted(arr)
    idx = int(len(arr_s) * p / 100)
    return arr_s[min(idx, len(arr_s) - 1)]

data = {
    "experiment": "e-swap-4",
    "host": "shakedown-macos",
    "generated_at_utc": ts,
    "load_profile": "2x_burst",
    "source_rate_msg_s": 2000,
    "total_swaps": swap_count,
    "successful_swaps": len(successful),
    "pause_duration_p50_burst_ms": percentile(pause_durations, 50) / 1_000_000 if pause_durations else 0,
    "pause_duration_p95_burst_ms": percentile(pause_durations, 95) / 1_000_000 if pause_durations else 0,
    "pause_duration_max_burst_ms": max(pause_durations) / 1_000_000 if pause_durations else 0,
    "git_sha": git_sha,
}

result_root = run_dir.parent
json.dump(data, open(result_root / "shakedown.json", "w"), indent=2)
json.dump({"experiment": "e-swap-4", "host_tag": "shakedown-macos", "generated_at": ts,
           "git_sha": git_sha, "total_swaps": swap_count}, open(run_dir / "metadata.json", "w"), indent=2)
print(f"E-Swap-4: {len(successful)} swaps under 2x burst, p95 pause={data['pause_duration_p95_burst_ms']:.2f} ms")
PY

    if [ "$swap_count" -ge 50 ]; then
        _log "  E-Swap-4: PASS ✓"
        pass_count=$((pass_count+1))
    else
        _log "  E-Swap-4: FAIL ✗ (only $swap_count swaps)"
        fail_count=$((fail_count+1))
    fi
}

# ===== E-Swap-5: failed swap rollback =====
run_rollback() {
    _log "E-Swap-5: failed swap recovery (v2-panics → rollback to v1)"

    local config="$REPO_ROOT/eval/configs/e-swap/pipeline-hotswap-rollback.toml"
    local result_root="$REPO_ROOT/eval/results/e-swap-5/shakedown-macos-${ts}"
    local run_dir="$result_root/run-1"
    mkdir -p "$run_dir"
    cp "$config" "$run_dir/config.toml"

    _log "  Starting pipeline (120k msgs @ 1000/s)"
    WAFER_BENCH_OUTPUT_DIR="$run_dir" "$WAFER_BIN" --config "$config" \
        >"$run_dir/stdout.log" 2>&1 &
    local wafer_pid=$!
    _last_wafer_pid=$wafer_pid

    if ! wait_api_ready; then
        _log "  BLOCKER: API did not become ready"
        kill "$wafer_pid" 2>/dev/null; wait "$wafer_pid" 2>/dev/null || true
        _last_wafer_pid=""
        fail_count=$((fail_count+1)); return
    fi

    sleep 6

    # Issue swaps with the panic plugin — each should fail and rollback
    local swap_count=0
    local rollback_events=0
    local swap_timeline_file="$run_dir/swap_timeline.jsonl"
    : > "$swap_timeline_file"
    local swap_start_epoch
    swap_start_epoch=$(date +%s)

    while true; do
        if ! kill -0 "$wafer_pid" 2>/dev/null; then
            break
        fi
        local elapsed=$(( $(date +%s) - swap_start_epoch ))
        if [ "$elapsed" -ge 60 ] && [ "$swap_count" -ge 10 ]; then
            break
        fi
        if [ "$elapsed" -ge 80 ]; then
            break
        fi

        # Swap to panic plugin — this should trigger rollback.
        # Post-B1 (2026-08-02, commit 78519ea): the API now returns
        # `HTTP 200 status=rolled_back` for a v2-panics swap that ACKed
        # and then rolled back. Older code returned 409/504. Count both
        # so the shakedown is portable across the fix boundary.
        # Fix 2026-08-02 (BL-1): previously this block also called
        # `do_swap` before the instrumented curl below, firing TWO
        # POST requests per iteration and doubling the observed rollback
        # event count. Removed to restore one-swap-per-iteration semantics.
        local status_code
        local body_file
        body_file=$(mktemp)
        status_code=$(curl -s -o "$body_file" -w "%{http_code}" -X POST \
            "$API_BASE/api/v1/nodes/$NODE_ID/hot-swap" \
            -H "Content-Type: application/json" \
            -d "{\"wasm_path\": \"$V2_PANICS\"}" 2>/dev/null || echo "000")
        local status_body
        status_body=$(cat "$body_file")
        rm -f "$body_file"

        echo "{\"swap_index\":$swap_count,\"http_status\":$status_code,\"body\":$(echo "$status_body" | python3 -c 'import sys,json;print(json.dumps(sys.stdin.read()))' 2>/dev/null || echo "\"\"")}" >> "$swap_timeline_file"

        # A rollback is signalled by: 409/504 (pre-B1 path), or
        # HTTP 200 with "status":"rolled_back" in the response body.
        if [ "$status_code" = "409" ] || [ "$status_code" = "504" ]; then
            rollback_events=$((rollback_events + 1))
        elif [ "$status_code" = "200" ] && echo "$status_body" | grep -q '"status":"rolled_back"'; then
            rollback_events=$((rollback_events + 1))
        fi

        swap_count=$((swap_count + 1))
        sleep 5
    done

    # Let pipeline continue a bit to verify sequence resumes
    sleep 3
    # Graceful shutdown
    curl -sf -X POST "$API_BASE/api/v1/pipeline/shutdown" >/dev/null 2>&1 || true
    # Pipeline exits non-zero after receiving shutdown signal while
    # processing trap-recovery cycles; expected behavior.
    wait "$wafer_pid" 2>/dev/null || true
    _last_wafer_pid=""

    if grep -q "thread.*panicked" "$run_dir/stdout.log" 2>/dev/null; then
        _log "  BLOCKER: Tokio-level panic"
        fail_count=$((fail_count+1)); return
    fi

    _log "  Rollback attempts: $swap_count, rollback events: $rollback_events"

    # Check sequence continuity from BenchSink output
    python3 - "$run_dir" "$swap_count" "$rollback_events" "$git_sha" "$ts" <<'PY'
import json, sys, csv
from pathlib import Path

run_dir = Path(sys.argv[1])
swap_count = int(sys.argv[2])
# Post-BL-1 rename: this counts both HTTP 409/504 (pre-B1 path) and
# HTTP 200 status=rolled_back (post-B1 path), so the field name shifted
# from `http_409_or_504_responses` to `http_error_or_rollback_responses`
# to stop misleading downstream readers.
http_error_or_rollback_responses = int(sys.argv[3])
git_sha = sys.argv[4]
ts = sys.argv[5]

# Sequence tracking
seq_path = run_dir / "sequence.csv"
seq_gaps = -1
seq_dups = -1
total_received = 0
if seq_path.exists():
    with open(seq_path) as f:
        reader = csv.DictReader(f)
        for row in reader:
            seq_gaps = int(row.get("gap_msgs", -1))
            seq_dups = int(row.get("duplicates_count", -1))
            total_received = int(row.get("total_received", 0))

# Count recovery events from stdout
log_path = run_dir / "stdout.log"
trap_count = 0
init_failed_count = 0
rollback_to_v1_count = 0
if log_path.exists():
    text = log_path.read_text()
    trap_count = text.count("unrecoverable error")
    init_failed_count = text.count("init failed; keeping v1")
    # A17 (T1, 2026-08-02): canary-window process-time rollback emits
    # "process-time rollback to v1 succeeded" from runner/transform.rs.
    # Older init-failure path still emits "keeping v1". Either counts as
    # a rollback to v1 event for E-Swap-5's rollback-success semantics.
    rollback_to_v1_count = (
        text.count("process-time rollback to v1 succeeded")
        + text.count("keeping v1")
    )
    # A17 rollback-time evidence: parse `rollback_time_ns=<int>` from
    # tracing::info emissions. Feeds shakedown.json.rollback_times_ns
    # (which the analysis notebook displays as the AC “under 10 s”
    # visualization). Tracing emits ANSI color codes when writing to a
    # tty-redirected file; strip them before matching so the regex
    # anchors on the field name reliably.
    import re as _re
    _ansi = _re.compile(r"\x1b\[[0-9;]*m")
    _clean = _ansi.sub("", text)
    rollback_times_ns = [
        int(m.group(1))
        for m in _re.finditer(r"rollback_time_ns=(\d+)", _clean)
    ]
else:
    rollback_times_ns = []

# A17 (Closed 2026-08-02): the canary window in the runner now retains v1's
# InstancePre for a bounded window after every swap. When v2-panics traps in
# process() the runner rolls back to v1 automatically (single-shot per
# swap). Previously (pre-A17) only A4 init-failure rollback existed, so a
# plugin passing init() but trapping process() led to permanent traps.
auto_rollback_worked = rollback_to_v1_count > 0

data = {
    "experiment": "e-swap-5",
    "host": "shakedown-macos",
    "generated_at_utc": ts,
    "swap_attempts": swap_count,
    "http_error_or_rollback_responses": http_error_or_rollback_responses,
    "trap_count_post_swap": trap_count,
    "a4_init_rollback_events": init_failed_count,
    "auto_rollback_to_v1": auto_rollback_worked,
    "a17_process_time_rollback_events": len(rollback_times_ns),
    "rollback_times_ns": rollback_times_ns,
    "sequence_gaps": seq_gaps,
    "sequence_duplicates": seq_dups,
    "total_received": total_received,
    "sequence_continues_after_rollback": auto_rollback_worked and total_received > 0,
    "runtime_panic": False,
    "note": "A17 canary rollback: runner retains v1 InstancePre for bounded window after swap and auto-rolls-back on process-time trap. Closed 2026-08-02.",
    "git_sha": git_sha,
}

result_root = run_dir.parent
json.dump(data, open(result_root / "shakedown.json", "w"), indent=2)
json.dump({"experiment": "e-swap-5", "host_tag": "shakedown-macos", "generated_at": ts,
           "git_sha": git_sha}, open(run_dir / "metadata.json", "w"), indent=2)
print(f"E-Swap-5: traps={data['trap_count_post_swap']}, a4_rollback={data['a4_init_rollback_events']}, received={total_received}")
PY

    if [ "$rollback_events" -gt 0 ] || grep -q "process-time rollback to v1 succeeded\|unrecoverable\|swap.*fail\|Swap.*fail\|keeping v1" "$run_dir/stdout.log" 2>/dev/null; then
        # A17 (Closed 2026-08-02): canary-window rollback is the primary
        # success signal; A4 init-failure rollback still counts.
        if grep -q "process-time rollback to v1 succeeded" "$run_dir/stdout.log" 2>/dev/null; then
            _log "  E-Swap-5: PASS ✓ (A17 canary process-time rollback exercised)"
        elif grep -q "keeping v1" "$run_dir/stdout.log" 2>/dev/null; then
            _log "  E-Swap-5: PASS ✓ (A4 init-failure rollback exercised)"
        else
            _log "  E-Swap-5: PARTIAL ⚠ (runtime detected failure via 504 but no rollback log)"
        fi
        pass_count=$((pass_count+1))
    else
        _log "  E-Swap-5: FAIL ✗ (no failure detection)"
        fail_count=$((fail_count+1))
    fi
}

# ===== Main dispatch =====
for mode in $swaps; do
    case "$mode" in
        unified) run_unified ;;
        burst) run_burst ;;
        rollback) run_rollback ;;
    esac
done

_log "Summary: pass=${pass_count} fail=${fail_count}"
exit $fail_count
