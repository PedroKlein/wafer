#!/usr/bin/env bash
# eval/scripts/summarise-e-perf-4.sh
#
# Post-process an E-Perf-4 shakedown directory (or a subset) into
# per-size roll-up JSON containing p50/p95/p99/p999 both per-run and
# aggregated across the N runs. Uses `wafer-loadgen hdr-summary` for
# authoritative percentile extraction (bypasses the Python `hdrh`
# library, whose V2-cookie handling is incompatible with the Rust
# `hdrhistogram` crate's serialiser — see hdr_summary.rs).
#
# Usage:
#   summarise-e-perf-4.sh <shakedown-dir>
#
# Writes per-size `size-percentiles.json` alongside the existing
# `size-summary.json` (produced by run-e-perf-4-shakedown.sh).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

SHAKE_DIR="${1:?Usage: summarise-e-perf-4.sh <shakedown-dir>}"
[ -d "$SHAKE_DIR" ] || { printf 'no such directory: %s\n' "$SHAKE_DIR" >&2; exit 2; }

WAFER_LOADGEN="$REPO_ROOT/target/release/wafer-loadgen"
[ -x "$WAFER_LOADGEN" ] || { printf 'wafer-loadgen missing (run: cargo build --release -p wafer-loadgen)\n' >&2; exit 3; }

for size_dir in "$SHAKE_DIR"/{120b,1kb,10kb,100kb}; do
    [ -d "$size_dir" ] || continue
    size_label="$(basename "$size_dir")"
    per_run_dir="$size_dir/per-run-percentiles"
    mkdir -p "$per_run_dir"

    # Emit one JSON per run and collect their paths for aggregation.
    for run_dir in "$size_dir"/run-*; do
        [ -f "$run_dir/latency.hdr" ] || continue
        run_name="$(basename "$run_dir")"
        "$WAFER_LOADGEN" hdr-summary \
            --hdr "$run_dir/latency.hdr" \
            --output "$per_run_dir/$run_name.json"
    done

    # Aggregate per-run summaries into one file per size.
    python3 - "$size_dir" "$per_run_dir" <<'PY' > "$size_dir/size-percentiles.json"
import json, os, sys, statistics
size_dir = sys.argv[1]
per_run_dir = sys.argv[2]
runs = []
for name in sorted(os.listdir(per_run_dir)):
    if not name.endswith(".json"):
        continue
    with open(os.path.join(per_run_dir, name)) as f:
        runs.append({"run_id": name.removesuffix(".json"), **json.load(f)})

def _agg(field):
    vals = [r[field] for r in runs if r.get(field, 0) > 0]
    if not vals:
        return None
    return {
        "n": len(vals),
        "min": min(vals),
        "max": max(vals),
        "median": statistics.median(vals),
        "mean": statistics.fmean(vals),
        "stdev": statistics.stdev(vals) if len(vals) > 1 else 0.0,
    }

print(json.dumps({
    "size_label": os.path.basename(size_dir),
    "runs_seen": len(runs),
    "aggregate_over_runs": {
        "p50_ns":   _agg("p50_ns"),
        "p90_ns":   _agg("p90_ns"),
        "p95_ns":   _agg("p95_ns"),
        "p99_ns":   _agg("p99_ns"),
        "p999_ns":  _agg("p999_ns"),
        "p9999_ns": _agg("p9999_ns"),
        "max_ns":   _agg("max_ns"),
        "total_count":  _agg("total_count"),
    },
    "per_run": runs,
    "notes": [
        "aggregate.mean is the mean of per-run percentiles (not the percentile of the merged distribution). Use the notebook for the merged-distribution number, but the per-run mean is what you compare to canonical Pi numbers as a shakedown sanity check.",
        "n < runs_seen indicates some runs recorded zero values (BenchSink warmup ate them all, or the pipeline stalled). Investigate before publishing.",
    ],
}, indent=2))
PY
    printf 'size %-6s: %s runs → %s\n' "$size_label" "$(ls "$per_run_dir" | wc -l | tr -d ' ')" "$size_dir/size-percentiles.json" >&2
done

printf 'wrote per-size percentiles under %s\n' "$SHAKE_DIR" >&2
