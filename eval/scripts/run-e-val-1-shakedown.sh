#!/usr/bin/env bash
# eval/scripts/run-e-val-1-shakedown.sh
#
# E-Val-1 methodology validation on macOS. Runs pipeline-c-with-delay N times
# and asserts every run's p99 lands in the [45,55] ms window. A p99 outside
# that window means the measurement rig is lying about tail latency and every
# downstream RQ number is suspect.
#
# Layout under eval/results/e-val-1/shakedown-macos-<UTC-ts>/:
#     run-01/{config.toml,stdout.log,latency.hdr,throughput.csv,sequence.csv,percentiles.json}
#     ...
#     run-N/
#     summary.json      # p99 range across runs + pass/fail per the AC.
#
# See RFC-008 §D9, plans/evaluation-infrastructure/execution-playbook.md.

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

runs=5
config="eval/configs/pipeline-c-with-delay.toml"
skip_build=0

# Kill the last-launched runtime on Ctrl-C so orphaned wafer processes
# don't hold onto BenchSource threads for the next invocation.
_last_wafer_pid=""
# Invoked indirectly by trap.
# shellcheck disable=SC2329
_cleanup_val() {
    local rc=$?
    [ -n "$_last_wafer_pid" ] && kill -TERM "$_last_wafer_pid" 2>/dev/null || true
    exit "$rc"
}
trap _cleanup_val EXIT INT TERM

while [ $# -gt 0 ]; do
    case "$1" in
        --runs) runs="${2:?}"; shift 2 ;;
        --skip-build) skip_build=1; shift ;;
        -h|--help) grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown flag: $1" >&2; exit 2 ;;
    esac
done

_log() { printf '[%s] %s\n' "$(date -u +%H:%M:%S)" "$*" >&2; }

if [ "$skip_build" -eq 0 ]; then
    _log "building wafer-runtime + wafer-loadgen (release)"
    cargo build --release -p wafer-runtime -p wafer-loadgen >&2
fi

WAFER_BIN="target/release/wafer"
HDR_BIN="target/release/wafer-loadgen"
[ -x "$WAFER_BIN" ] || { _log "runtime binary missing"; exit 3; }
[ -x "$HDR_BIN" ]   || { _log "loadgen binary missing"; exit 3; }

# Ensure the delay-injector plugin exists — refuse to run against a stale
# artefact from a prior compile pass.
DELAY_WASM="plugins/delay-injector/target/wasm32-wasip2/release/wafer_delay_injector.wasm"
if [ ! -f "$DELAY_WASM" ]; then
    _log "delay-injector wasm missing at $DELAY_WASM — build plugins first"
    exit 3
fi

ts=$(date -u +'%Y-%m-%dT%H-%M-%SZ')
out_root="eval/results/e-val-1/shakedown-macos-$ts"
mkdir -p "$out_root"
_log "output dir: $out_root"

pass_count=0
fail_count=0
p99_lo_ms=1000
p99_hi_ms=0

for i in $(seq -w 1 "$runs"); do
    run_dir="$out_root/run-$i"
    mkdir -p "$run_dir"
    cp "$config" "$run_dir/config.toml"

    _log "run $i / $runs"
    # `|| true` is intentional: pipeline-c-with-delay terminates when the
    # BenchSource emits its total_messages, causing a normal-but-non-zero
    # exit code. Removing `|| true` would break `set -e`. Track the PID so
    # the trap can kill orphans on Ctrl-C. See P-Followup-4.
    WAFER_BENCH_OUTPUT_DIR="$run_dir" "$WAFER_BIN" --config "$config" \
        >"$run_dir/stdout.log" 2>&1 &
    _last_wafer_pid=$!
    wait "$_last_wafer_pid" || true
    _last_wafer_pid=""

    if [ ! -f "$run_dir/latency.hdr" ]; then
        _log "  MISSING latency.hdr — recording as failure"
        fail_count=$((fail_count+1))
        continue
    fi

    # Extract percentiles; abort on empty histogram.
    "$HDR_BIN" hdr-summary --hdr "$run_dir/latency.hdr" \
        > "$run_dir/percentiles.json"
    p99_ns=$(python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print(d['p99_ns'])" "$run_dir/percentiles.json")
    count=$(python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print(d['total_count'])" "$run_dir/percentiles.json")
    p99_ms=$(python3 -c "print($p99_ns/1_000_000)")

    if [ "$count" -eq 0 ]; then
        _log "  EMPTY histogram — recording as failure"
        fail_count=$((fail_count+1))
        continue
    fi

    within=$(python3 -c "print(1 if 45 <= $p99_ms <= 55 else 0)")
    if [ "$within" -eq 1 ]; then
        _log "  p99 = ${p99_ms} ms  ✓ within [45,55] window (n=$count)"
        pass_count=$((pass_count+1))
    else
        _log "  p99 = ${p99_ms} ms  ✗ OUTSIDE [45,55] window (n=$count)"
        fail_count=$((fail_count+1))
    fi

    # Track min/max p99 across all runs regardless of individual pass/fail.
    p99_lo_ms=$(python3 -c "print(min($p99_lo_ms, $p99_ms))")
    p99_hi_ms=$(python3 -c "print(max($p99_hi_ms, $p99_ms))")
done

honesty="pass"
if [ "$fail_count" -gt 0 ]; then honesty="fail"; fi

python3 - "$out_root/summary.json" "$runs" "$pass_count" "$fail_count" \
    "$p99_lo_ms" "$p99_hi_ms" "$honesty" <<'PY'
import json, sys, datetime
out, runs, ok, bad, lo, hi, honesty = sys.argv[1:]
json.dump({
    "experiment": "e-val-1",
    "host": "shakedown-macos",
    "generated_at_utc": datetime.datetime.utcnow().isoformat() + "Z",
    "config": "eval/configs/pipeline-c-with-delay.toml",
    "runs_total": int(runs),
    "runs_pass": int(ok),
    "runs_fail": int(bad),
    "p99_ms_min_across_runs": float(lo),
    "p99_ms_max_across_runs": float(hi),
    "honesty_gate": honesty,
    "gate_window_ms": [45, 55],
}, open(out, "w"), indent=2)
print(open(out).read())
PY

_log "done — pass=$pass_count fail=$fail_count honesty=$honesty"
exit $fail_count
