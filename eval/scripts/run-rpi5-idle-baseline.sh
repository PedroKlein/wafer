#!/usr/bin/env bash
# Diagnostic idle-power baseline: the two host sidecars sample the Pi with the
# broker up and no pipeline running, so the campaign's PMIC proxy can be
# reported net of idle. Never thesis evidence and never pooled with a batch.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
samples=10
sample_secs=60
output=""
dry_run=0
support_cpus="${WAFER_LOADGEN_CPUSET:-0}"
sut_cpus="${WAFER_RUNTIME_CPUSET:-1-3}"

usage() {
    cat <<USAGE
Usage: $0 [--samples N] [--sample-secs S] [--output DIR] [--dry-run]

  --samples N        Number of idle samples (default: 10)
  --sample-secs S    Length of each sample in seconds (default: 60)
  --output DIR       Result directory (default: eval/results/idle-baseline/rpi5-<UTC>)
  --dry-run          Print the plan without sampling
USAGE
}

while [ $# -gt 0 ]; do
    case "$1" in
        --samples)      samples="${2:?}"; shift 2 ;;
        --sample-secs)  sample_secs="${2:?}"; shift 2 ;;
        --output)       output="${2:?}"; shift 2 ;;
        --dry-run)      dry_run=1; shift ;;
        -h|--help)      usage; exit 0 ;;
        *) printf 'Unknown flag: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done
[ "$samples" -ge 1 ] && [ "$sample_secs" -ge 5 ] || { echo "error: samples >= 1 and sample-secs >= 5" >&2; exit 2; }
[ -n "$output" ] || output="$ROOT/eval/results/idle-baseline/rpi5-$(date -u +%Y-%m-%dT%H-%M-%SZ)"

if [ "$dry_run" -eq 1 ]; then
    printf 'experiment: idle-baseline\nevidence_class: diagnostic\nsamples: %s\nsample_secs: %s\noutput: %s\n' \
        "$samples" "$sample_secs" "$output"
    printf 'host state: broker up, kuiper.service stopped as in WAFER runs, no wafer or loadgen process\n'
    printf 'sidecars: taskset -c %s pi_telemetry.py; proc_telemetry.py --pin-cpus %s --sut-cpus %s\n' \
        "$support_cpus" "$support_cpus" "$sut_cpus"
    printf 'summary: eval/scripts/summarise-idle-baseline.py %s\n' "$output"
    exit 0
fi

"$ROOT/eval/scripts/preflight-pi5.sh"
for process in wafer wafer-runtime wafer-loadgen; do
    if pgrep -x "$process" >/dev/null; then
        echo "error: $process is running; the idle baseline needs no pipeline or load" >&2
        exit 1
    fi
done
# WAFER and native campaign runs stop eKuiper for their duration, so the
# baseline does too and restarts it however the script ends.
sudo systemctl stop kuiper.service
trap 'sudo systemctl start kuiper.service' EXIT
mkdir -p "$output"
git_sha="$(git -C "$ROOT" rev-parse HEAD)"

for index in $(seq -f '%02g' 1 "$samples"); do
    sample="$output/sample-$index"
    mkdir -p "$sample"
    started_ns="$(date +%s%N)"
    taskset -c "$support_cpus" python3 "$ROOT/eval/scripts/lib/pi_telemetry.py" "$sample" &
    pi_pid=$!
    python3 "$ROOT/eval/scripts/lib/proc_telemetry.py" "$sample" \
        --pin-cpus "$support_cpus" --sut-cpus "$sut_cpus" &
    proc_pid=$!
    sleep "$sample_secs"
    kill -TERM "$pi_pid" "$proc_pid" 2>/dev/null || true
    wait "$pi_pid" 2>/dev/null || true
    wait "$proc_pid" 2>/dev/null || true
    finished_ns="$(date +%s%N)"
    printf '{"started_ns":%s,"finished_ns":%s}\n' "$started_ns" "$finished_ns" >"$sample/measurement-window.json"
    python3 - "$sample/metadata.json" "$index" "$git_sha" "$sample_secs" <<'PY'
import json, platform, sys
path, index, git_sha, secs = sys.argv[1:]
json.dump({
    "experiment": "idle-baseline",
    "evidence_class": "diagnostic",
    "thesis_evidence": False,
    "host_tag": "rpi5",
    "sample_index": int(index),
    "sample_secs": int(secs),
    "git_sha": git_sha,
    "kernel": platform.release(),
    "arch": platform.machine(),
    "system": "idle",
}, open(path, "w"), indent=2)
PY
    echo "idle sample $index done"
done

python3 "$ROOT/eval/scripts/summarise-idle-baseline.py" "$output"
printf 'idle baseline: %s\n' "$output"
