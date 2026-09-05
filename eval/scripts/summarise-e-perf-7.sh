#!/usr/bin/env bash
# eval/scripts/summarise-e-perf-7.sh
#
# Post-process E-Perf-7 shakedown directory: emit per-config percentile
# summaries via `wafer-loadgen hdr-summary`. Produces config-percentiles.json
# in each config dir.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

SHAKE_DIR="${1:?Usage: summarise-e-perf-7.sh <shakedown-dir>}"
[ -d "$SHAKE_DIR" ] || { printf 'no such directory: %s\n' "$SHAKE_DIR" >&2; exit 2; }

WAFER_LOADGEN="$REPO_ROOT/target/release/wafer-loadgen"
[ -x "$WAFER_LOADGEN" ] || { printf 'wafer-loadgen missing (cargo build --release -p wafer-loadgen)\n' >&2; exit 3; }

for config_dir in "$SHAKE_DIR"/{fuel-only,epoch-only,neither,passthrough}; do
    [ -d "$config_dir" ] || continue
    config_label="$(basename "$config_dir")"
    per_run_dir="$config_dir/per-run-percentiles"
    mkdir -p "$per_run_dir"

    for run_dir in "$config_dir"/run-*; do
        [ -f "$run_dir/latency.hdr" ] || continue
        run_name="$(basename "$run_dir")"
        "$WAFER_LOADGEN" hdr-summary \
            --hdr "$run_dir/latency.hdr" \
            --output "$per_run_dir/$run_name.json"
    done

    # Aggregate per-run summaries
    python3 - "$config_dir" "$per_run_dir" <<'PY' > "$config_dir/config-percentiles.json"
import json, os, sys, statistics
config_dir = sys.argv[1]
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
    "config_label": os.path.basename(config_dir),
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
    printf 'config %-12s: %s runs → %s\n' "$config_label" "$(find "$per_run_dir" -maxdepth 1 -type f -name '*.json' | wc -l | tr -d ' ')" "$config_dir/config-percentiles.json" >&2
done

printf 'wrote per-config percentiles under %s\n' "$SHAKE_DIR" >&2
