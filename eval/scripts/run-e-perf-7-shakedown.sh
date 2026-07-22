#!/usr/bin/env bash
# eval/scripts/run-e-perf-7-shakedown.sh
#
# E-Perf-7 metering-overhead decomposition shakedown on macOS.
# 4 configs (fuel-only, epoch-only, neither, passthrough) × N runs.
#
# Layout under eval/results/e-perf-7/shakedown-macos-<ts>/:
#   {fuel-only,epoch-only,neither,passthrough}/
#     run-01/ ... run-N/
#     config-summary.json
#   shakedown.json
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

usage() {
    cat <<'USAGE'
Usage: run-e-perf-7-shakedown.sh [options]

Options:
  --runs <N>           Runs per config (default: 30).
  --configs <list>     Comma-separated config labels. Default: fuel-only,epoch-only,neither,passthrough
  --skip-build         Assume wafer binary is current.
  --dry-run            Print plan and exit.
  -h, --help           This help.
USAGE
}

runs=30
configs="fuel-only,epoch-only,neither,passthrough"
skip_build=0
dry_run=0

while [ $# -gt 0 ]; do
    case "$1" in
        --runs)       runs="${2:?}"; shift 2 ;;
        --configs)    configs="${2:?}"; shift 2 ;;
        --skip-build) skip_build=1; shift ;;
        --dry-run)    dry_run=1; shift ;;
        -h|--help)    usage; exit 0 ;;
        *) printf 'unknown flag: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done

if [ "$runs" -lt 30 ]; then
    printf 'WARN: --runs=%s < 30 (AC requires ≥30).\n' "$runs" >&2
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
# Output root
# ---------------------------------------------------------------------------

TS="$(date -u +'%Y-%m-%dT%H-%M-%SZ')"
OUT_ROOT="$REPO_ROOT/eval/results/e-perf-7/shakedown-macos-$TS"
mkdir -p "$OUT_ROOT"

_log "output root: $OUT_ROOT"
_log "runs per config: $runs"
_log "configs: $configs"

if [ "$dry_run" -eq 1 ]; then
    _log "DRY RUN — nothing launched"
    exit 0
fi

# ---------------------------------------------------------------------------
# Per-run driver
# ---------------------------------------------------------------------------

_run_one() {
    local config_label=$1
    local run_idx=$2
    local out_dir="$OUT_ROOT/$config_label/run-$(printf '%02d' "$run_idx")"
    local cfg="$REPO_ROOT/eval/configs/e-perf-7/pipeline-c-${config_label}.toml"

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
  "experiment": "e-perf-7",
  "host_tag": "shakedown-macos",
  "config_label": "$config_label",
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

    local traps
    traps=$(grep -c "unrecoverable error" "$out_dir/stdout.log" 2>/dev/null || true)
    traps=${traps:-0}
    printf '%s|run-%02d|exit=%s|traps=%s|dur_ns=%s\n' \
        "$config_label" "$run_idx" "$runtime_exit" "$traps" "$duration_ns" >&2
    if [ "$runtime_exit" -ne 0 ] || [ "$traps" -ne 0 ]; then
        _log "  ↳ non-clean run — investigate $out_dir/stdout.log"
    fi
}

# ---------------------------------------------------------------------------
# Main loop
# ---------------------------------------------------------------------------

IFS=',' read -ra configs_arr <<< "$configs"
for config_label in "${configs_arr[@]}"; do
    _log "=== config: $config_label ==="
    mkdir -p "$OUT_ROOT/$config_label"
    for i in $(seq 1 "$runs"); do
        _run_one "$config_label" "$i"
    done

    # Emit config-summary.json
    python3 - "$OUT_ROOT/$config_label" "$runs" "$config_label" <<'PY' > "$OUT_ROOT/$config_label/config-summary.json"
import json, os, sys
root = sys.argv[1]
runs = int(sys.argv[2])
config_label = sys.argv[3]
run_inventory = []
for i in range(1, runs + 1):
    rdir = os.path.join(root, f"run-{i:02d}")
    hdr = os.path.join(rdir, "latency.hdr")
    meta = os.path.join(rdir, "metadata.json")
    if not (os.path.isdir(rdir) and os.path.isfile(hdr) and os.path.isfile(meta)):
        run_inventory.append({"run": i, "ok": False, "reason": "missing files"})
        continue
    recorded = 0
    try:
        with open(hdr) as f:
            for line in f:
                if line.startswith("#Recorded values:"):
                    recorded = int(line.split(":")[1].strip())
                if not line.startswith("#"):
                    break
    except Exception as e:
        run_inventory.append({"run": i, "ok": False, "reason": str(e)})
        continue
    run_inventory.append({
        "run": i,
        "ok": recorded > 0,
        "recorded_values": recorded,
    })
ok_count = sum(1 for r in run_inventory if r.get("ok"))
print(json.dumps({
    "config_label": config_label,
    "runs_requested": runs,
    "runs_ok": ok_count,
    "per_run": run_inventory,
    "note": "Percentile aggregation via analysis notebook.",
}, indent=2))
PY
done

# ---------------------------------------------------------------------------
# Roll-up manifest
# ---------------------------------------------------------------------------

cat > "$OUT_ROOT/shakedown.json" <<META
{
  "experiment": "e-perf-7",
  "shakedown": true,
  "host_tag": "shakedown-macos",
  "generated_at": "$(date -u +'%Y-%m-%dT%H:%M:%SZ')",
  "git_sha": "$(git rev-parse HEAD 2>/dev/null || echo unknown)",
  "hostname": "$(hostname)",
  "arch": "$(uname -m)",
  "os": "$(uname -s | tr '[:upper:]' '[:lower:]')",
  "rustc": "$(rustc --version 2>/dev/null || echo unknown)",
  "runs_per_config": $runs,
  "configs": [$(printf '"%s",' "${configs_arr[@]}" | sed 's/,$//')]
}
META

_log "shakedown complete: $OUT_ROOT"
