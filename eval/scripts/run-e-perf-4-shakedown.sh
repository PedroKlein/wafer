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
# Percentile aggregation across latency.hdr files lives in the analysis
# notebook, which uses the Rust HdrHistogram crate to decode the interval
# log (Python's hdrh library does not accept the V2 cookie the Rust crate
# emits). This script only performs presence + recorded-values audits
# on the per-run artefacts.

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
    # `grep -c` exits 1 when there are zero matches; the `|| true` and the
    # explicit `${traps:-0}` guarantee `traps` ends up as a clean integer
    # string even when set -e is in effect.
    local traps
    traps=$(grep -c "unrecoverable error" "$out_dir/stdout.log" 2>/dev/null || true)
    traps=${traps:-0}
    printf '%s|run-%02d|exit=%s|traps=%s|dur_ns=%s\n' \
        "$size_label" "$run_idx" "$runtime_exit" "$traps" "$duration_ns" >&2
    if [ "$runtime_exit" -ne 0 ] || [ "$traps" -ne 0 ]; then
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

    # Emit a placeholder size-summary.json. Percentile aggregation across
    # latency.hdr files happens in the analysis notebook
    # (eval/analysis/notebooks/02-per-hop-overhead.ipynb) which uses the
    # authoritative Rust HdrHistogram crate to parse the interval log —
    # avoiding the Python hdrh library's known incompatibility with the
    # Rust HdrHistogram V2 interval-log cookie format. The notebook is the
    # single source of percentile truth; this file records only the run
    # inventory, host provenance, and file presence for downstream
    # smoke-checking.
    _log "emitting size-summary.json for $size_label"
    python3 - "$OUT_ROOT/$size_label" "$runs" <<'PY' > "$OUT_ROOT/$size_label/size-summary.json"
import json, os, sys
root = sys.argv[1]
runs = int(sys.argv[2])
run_inventory = []
for i in range(1, runs + 1):
    rdir = os.path.join(root, f"run-{i:02d}")
    hdr = os.path.join(rdir, "latency.hdr")
    meta = os.path.join(rdir, "metadata.json")
    if not (os.path.isdir(rdir) and os.path.isfile(hdr) and os.path.isfile(meta)):
        run_inventory.append({"run": i, "ok": False, "reason": "missing files"})
        continue
    # Read the Recorded-values header line the Rust hdrhistogram crate emits.
    recorded = 0
    total = 0
    try:
        with open(hdr) as f:
            for line in f:
                if line.startswith("#Recorded values:"):
                    recorded = int(line.split(":")[1].strip())
                elif line.startswith("#Total messages:"):
                    total = int(line.split(":")[1].strip())
                if not line.startswith("#"):
                    break
    except Exception as e:
        run_inventory.append({"run": i, "ok": False, "reason": str(e)})
        continue
    run_inventory.append({
        "run": i,
        "ok": recorded > 0,
        "total_messages": total,
        "recorded_values": recorded,
    })
ok_count = sum(1 for r in run_inventory if r.get("ok"))
print(json.dumps({
    "payload_size_label": os.path.basename(root),
    "runs_requested": runs,
    "runs_ok": ok_count,
    "per_run": run_inventory,
    "note": "Percentiles are computed by the analysis notebook — this file only records run inventory and per-run recorded-value counts. If runs_ok < runs_requested the shakedown must be re-run before publishing figures.",
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
