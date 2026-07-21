#!/usr/bin/env bash
# eval/scripts/run-e-perf-4-shakedown.sh
#
# Drives the E-Perf-4 per-hop-overhead shakedown on macOS: pipeline-c
# (bench-source → wafer-pass-through → bench-sink) × 4 payload sizes
# (120 B, 1 KiB, 10 KiB, 100 KiB) × N runs (default 30).
#
# Layout under eval/results/e-perf-4/shakedown-macos-<UTC-ts>/:
#
#     120b/
#       run-01/
#         config.toml
#         metadata.json
#         stdout.log
#         latency.hdr
#         throughput.csv
#         sequence.csv
#       run-02/...
#       ...
#       run-N/
#       size-summary.json
#     1kb/
#     10kb/
#     100kb/
#     shakedown.json
#
# Each run gets its own directory conforming to the per-run subset of
# eval/RESULT-CONTRACT.md (the memory sampler and Prometheus scrape are
# skipped for shakedown speed — see docs/status/canonical-readiness.md
# for the delta list). The `size-summary.json` aggregates p50/p95/p99
# across the N runs from the HdrHistogram of each run. `shakedown.json`
# rolls up host and config-hash provenance across all four sizes.
#
# This script is *shakedown-only*. Canonical Pi runs go through
# run-experiment.sh with --host rpi4 and the full contract.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

usage() {
    cat <<'USAGE'
Usage: run-e-perf-4-shakedown.sh [options]

Options:
  --runs <N>           Runs per payload size (default: 30). AC requires N ≥ 30.
  --sizes <list>       Comma-separated payload labels. Default: 120b,1kb,10kb,100kb
  --skip-build         Assume wafer-runtime + hdr-summary binaries are current.
  --dry-run            Print plan and exit without launching anything.
  -h, --help           This help.

Environment:
  WAFER_E_PERF_4_ROOT  Override the shakedown result root. Defaults to
                       eval/results/e-perf-4/.
USAGE
}

runs=30
sizes="120b,1kb,10kb,100kb"
skip_build=0
dry_run=0

while [ $# -gt 0 ]; do
    case "$1" in
        --runs)       runs="${2:?}"; shift 2 ;;
        --sizes)      sizes="${2:?}"; shift 2 ;;
        --skip-build) skip_build=1; shift ;;
        --dry-run)    dry_run=1; shift ;;
        -h|--help)    usage; exit 0 ;;
        *) printf 'unknown flag: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done

if [ "$runs" -lt 30 ]; then
    printf 'WARN: --runs=%s < 30 (acceptance criterion). Continuing anyway.\n' "$runs" >&2
fi

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
    python3 -c 'import time; print(int(time.time()*1e9))' 2>/dev/null \
        || perl -MTime::HiRes=time -e 'printf "%d\n", time() * 1e9'
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
# Resolve output root
# ---------------------------------------------------------------------------

ROOT="${WAFER_E_PERF_4_ROOT:-$REPO_ROOT/eval/results/e-perf-4}"
TS="$(date -u +'%Y-%m-%dT%H-%M-%SZ')"
OUT_ROOT="$ROOT/shakedown-macos-$TS"
mkdir -p "$OUT_ROOT"

_log "output root: $OUT_ROOT"
_log "runs per size: $runs"
_log "sizes: $sizes"

if [ "$dry_run" -eq 1 ]; then
    _log "DRY RUN — nothing launched"
    exit 0
fi

# ---------------------------------------------------------------------------
# hdr-summary via cargo (renders p50/p95/p99 from a latency.hdr file)
# ---------------------------------------------------------------------------
# We don't have a standalone binary for this. Percentile aggregation is done
# per-run by parsing throughput.csv (which BenchSink also emits) with awk;
# for the HdrHistogram we rely on hdrhistogram-cli if the operator has it,
# else fall back to a minimal Python parser. Not a canonical parser — the
# canonical analysis lives in eval/analysis/notebooks/02-per-hop-overhead.ipynb.

_hdr_percentiles() {
    local hdr="$1"
    # Try hdr-histogram-cli first
    if command -v hdr-histogram-cli >/dev/null 2>&1; then
        hdr-histogram-cli --input "$hdr" --percentiles 50,95,99 --format json 2>/dev/null && return
    fi
    # Fallback: python parser using the hdrhistogram library if installed,
    # else emit a placeholder record explaining the miss.
    python3 - "$hdr" <<'PY'
import json, sys
path = sys.argv[1]
try:
    from hdrh.histogram import HdrHistogram
    from hdrh.log import HistogramLogReader
    reader = HistogramLogReader(path, HdrHistogram(1, 60_000_000_000, 3))
    agg = HdrHistogram(1, 60_000_000_000, 3)
    while True:
        h = reader.get_next_interval_histogram()
        if h is None:
            break
        agg.add(h)
    total = agg.get_total_count()
    print(json.dumps({
        "hdr_path": path,
        "total_count": total,
        "p50_ns": agg.get_value_at_percentile(50) if total else 0,
        "p95_ns": agg.get_value_at_percentile(95) if total else 0,
        "p99_ns": agg.get_value_at_percentile(99) if total else 0,
        "p999_ns": agg.get_value_at_percentile(99.9) if total else 0,
        "max_ns": agg.get_max_value() if total else 0,
        "min_ns": agg.get_min_value() if total else 0,
    }))
except ImportError:
    print(json.dumps({
        "hdr_path": path,
        "error": "hdrh Python module not installed; run `pip install hdrhistogram` for shakedown summary parsing",
    }))
PY
}

# ---------------------------------------------------------------------------
# Per-run driver
# ---------------------------------------------------------------------------

_run_one() {
    local size_label=$1
    local run_idx=$2
    local out_dir="$OUT_ROOT/$size_label/run-$(printf '%02d' "$run_idx")"
    local cfg="$REPO_ROOT/eval/configs/e-perf-4/pipeline-c-passthrough-${size_label}.toml"

    [ -f "$cfg" ] || { _log "config missing: $cfg"; return 1; }
    mkdir -p "$out_dir"
    cp "$cfg" "$out_dir/config.toml"
    local cfg_sha; cfg_sha="$(_sha256 "$cfg")"

    local started_ns finished_ns duration_ns runtime_exit
    started_ns=$(_now_ns)
    local started_at; started_at="$(date -u +'%Y-%m-%dT%H:%M:%SZ')"

    WAFER_BENCH_OUTPUT_DIR="$out_dir" "$WAFER_BIN" --config "$cfg" \
        >"$out_dir/stdout.log" 2>&1 || runtime_exit=$?
    runtime_exit=${runtime_exit:-0}
    finished_ns=$(_now_ns)
    duration_ns=$(( finished_ns - started_ns ))
    local finished_at; finished_at="$(date -u +'%Y-%m-%dT%H:%M:%SZ')"

    cat > "$out_dir/metadata.json" <<META
{
  "experiment": "e-perf-4",
  "host_tag": "shakedown-macos",
  "payload_size_label": "$size_label",
  "run_index": $run_idx,
  "generated_at": "$finished_at",
  "started_at": "$started_at",
  "finished_at": "$finished_at",
  "duration_ns": $duration_ns,
  "git_sha": "$(git rev-parse HEAD 2>/dev/null || echo unknown)",
  "hostname": "$(hostname)",
  "kernel": "$(uname -r)",
  "arch": "$(uname -m)",
  "os": "$(uname -s | tr '[:upper:]' '[:lower:]')",
  "rustc": "$(rustc --version 2>/dev/null || echo unknown)",
  "config_path": "$cfg",
  "config_sha256": "$cfg_sha",
  "exit_codes": { "wafer_runtime": $runtime_exit }
}
META
    # Detect trap+recovery events in stdout — a real shakedown run must be zero.
    local traps
    traps=$(grep -c "unrecoverable error" "$out_dir/stdout.log" 2>/dev/null | tr -d '[:space:]' || echo 0)
    traps=${traps:-0}
    printf '%s|run-%02d|exit=%s|traps=%s|dur_ns=%s\n' \
        "$size_label" "$run_idx" "$runtime_exit" "$traps" "$duration_ns" >&2
    if [ "$runtime_exit" -ne 0 ] || [ "$traps" != "0" ]; then
        _log "  ↳ non-clean run — investigate $out_dir/stdout.log before trusting these numbers"
    fi
}

# ---------------------------------------------------------------------------
# Main loop
# ---------------------------------------------------------------------------

IFS=',' read -ra sizes_arr <<< "$sizes"
for size_label in "${sizes_arr[@]}"; do
    _log "=== payload $size_label ==="
    mkdir -p "$OUT_ROOT/$size_label"
    for i in $(seq 1 "$runs"); do
        _run_one "$size_label" "$i"
    done

    # Aggregate p50/p95/p99 across runs into size-summary.json.
    _log "aggregating percentiles for $size_label"
    python3 - "$OUT_ROOT/$size_label" "$runs" <<'PY' > "$OUT_ROOT/$size_label/size-summary.json"
import json, os, sys, statistics, subprocess
root = sys.argv[1]
runs = int(sys.argv[2])
per_run = []
for i in range(1, runs + 1):
    rdir = os.path.join(root, f"run-{i:02d}")
    hdr = os.path.join(rdir, "latency.hdr")
    meta = os.path.join(rdir, "metadata.json")
    if not (os.path.isfile(hdr) and os.path.isfile(meta)):
        continue
    try:
        from hdrh.histogram import HdrHistogram
        from hdrh.log import HistogramLogReader
        reader = HistogramLogReader(hdr, HdrHistogram(1, 60_000_000_000, 3))
        agg = HdrHistogram(1, 60_000_000_000, 3)
        while True:
            h = reader.get_next_interval_histogram()
            if h is None:
                break
            agg.add(h)
        total = agg.get_total_count()
        per_run.append({
            "run": i,
            "total_count": total,
            "p50_ns": agg.get_value_at_percentile(50) if total else 0,
            "p95_ns": agg.get_value_at_percentile(95) if total else 0,
            "p99_ns": agg.get_value_at_percentile(99) if total else 0,
            "p999_ns": agg.get_value_at_percentile(99.9) if total else 0,
            "max_ns": agg.get_max_value() if total else 0,
        })
    except ImportError:
        per_run.append({"run": i, "error": "hdrh Python module not installed"})

def _agg(field):
    vals = [r[field] for r in per_run if field in r and r[field] > 0]
    if not vals:
        return None
    return {
        "n": len(vals),
        "median": statistics.median(vals),
        "mean": statistics.fmean(vals),
        "stdev": statistics.stdev(vals) if len(vals) > 1 else 0,
        "min": min(vals),
        "max": max(vals),
    }

print(json.dumps({
    "payload_size_label": os.path.basename(root),
    "runs_recorded": len(per_run),
    "runs_requested": runs,
    "per_run": per_run,
    "aggregate_over_runs": {
        "p50_ns": _agg("p50_ns"),
        "p95_ns": _agg("p95_ns"),
        "p99_ns": _agg("p99_ns"),
        "p999_ns": _agg("p999_ns"),
    },
}, indent=2))
PY
done

# ---------------------------------------------------------------------------
# Roll-up manifest
# ---------------------------------------------------------------------------

cat > "$OUT_ROOT/shakedown.json" <<META
{
  "experiment": "e-perf-4",
  "shakedown": true,
  "host_tag": "shakedown-macos",
  "generated_at": "$(date -u +'%Y-%m-%dT%H:%M:%SZ')",
  "git_sha": "$(git rev-parse HEAD 2>/dev/null || echo unknown)",
  "hostname": "$(hostname)",
  "arch": "$(uname -m)",
  "os": "$(uname -s | tr '[:upper:]' '[:lower:]')",
  "rustc": "$(rustc --version 2>/dev/null || echo unknown)",
  "runs_per_size": $runs,
  "sizes": [$(printf '"%s"' "${sizes_arr[@]}" | sed 's/""/","/g')],
  "notes": "P2.1 shakedown for E-Perf-4 (per-hop overhead ablation, RFC-008). Uses pipeline-c-passthrough × 4 payload sizes. Memory sampler and Prometheus scrape are omitted for shakedown speed — canonical Pi runs use run-experiment.sh for the full contract."
}
META

_log "shakedown complete: $OUT_ROOT"
find "$OUT_ROOT" -maxdepth 2 -type f -name '*.json' | sort
