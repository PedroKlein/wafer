#!/usr/bin/env bash
# eval/scripts/summarise-e-perf-6-8.sh
#
# Post-process E-Perf-6/8 shakedown directories: emit per-depth percentile
# summaries via `wafer-loadgen hdr-summary`. Produces depth-percentiles.json
# in each depth dir.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

SHAKE_DIR="${1:?Usage: summarise-e-perf-6-8.sh <shakedown-dir>}"
[ -d "$SHAKE_DIR" ] || { printf 'no such directory: %s\n' "$SHAKE_DIR" >&2; exit 2; }

WAFER_LOADGEN="$REPO_ROOT/target/release/wafer-loadgen"
[ -x "$WAFER_LOADGEN" ] || { printf 'wafer-loadgen missing (cargo build --release -p wafer-loadgen)\n' >&2; exit 3; }

for depth_dir in "$SHAKE_DIR"/depth-*; do
    [ -d "$depth_dir" ] || continue
    depth_label="$(basename "$depth_dir")"
    per_run_dir="$depth_dir/per-run-percentiles"
    mkdir -p "$per_run_dir"

    for run_dir in "$depth_dir"/run-*; do
        [ -f "$run_dir/latency.hdr" ] || continue
        run_name="$(basename "$run_dir")"
        "$WAFER_LOADGEN" hdr-summary \
            --hdr "$run_dir/latency.hdr" \
            --output "$per_run_dir/$run_name.json"
    done

    # Aggregate per-run summaries
    python3 - "$depth_dir" "$per_run_dir" <<'PY' > "$depth_dir/depth-percentiles.json"
import json, os, sys, statistics
depth_dir = sys.argv[1]
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
    "depth_label": os.path.basename(depth_dir),
    "runs_seen": len(runs),
    "aggregate_over_runs": {
        "p50_ns":   _agg("p50_ns"),
        "p90_ns":   _agg("p90_ns"),
        "p95_ns":   _agg("p95_ns"),
        "p99_ns":   _agg("p99_ns"),
        "p999_ns":  _agg("p999_ns"),
        "p9999_ns": _agg("p9999_ns"),
        "max_ns":   _agg("max_ns"),
        "total_count": _agg("total_count"),
    },
    "per_run": runs,
}, indent=2))
PY
    printf 'depth %-8s: %s runs → %s\n' "$depth_label" "$(find "$per_run_dir" -maxdepth 1 -type f -name '*.json' | wc -l | tr -d ' ')" "$depth_dir/depth-percentiles.json" >&2
done

printf 'wrote per-depth percentiles under %s\n' "$SHAKE_DIR" >&2
