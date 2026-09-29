#!/usr/bin/env bash
# Diagnostic control pair for the host sidecars: E-Perf-1 WAFER runs with the
# Pi and /proc telemetry sidecars on and off, alternated in counterbalanced
# pairs, so the sampling cost is measured on the Pi rather than assumed.
# Never thesis evidence and never pooled with a campaign batch.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
pairs=6
cooldown_secs=60
output=""
dry_run=0
support_cpus="${WAFER_LOADGEN_CPUSET:-0}"
sut_cpus="${WAFER_RUNTIME_CPUSET:-1-3}"
config="eval/configs/pipeline-a-wafer.toml"
profile="eval/loadgen/telemetry-120b.toml"

usage() {
    cat <<USAGE
Usage: $0 [--pairs N] [--cooldown-secs S] [--output DIR] [--dry-run]

  --pairs N            Number of on/off pairs (default: 6)
  --cooldown-secs S    Pause between runs, as in the campaign (default: 60)
  --output DIR         Result directory (default: eval/results/instrument-ab/rpi5-<UTC>)
  --dry-run            Print the run order without running anything
USAGE
}

while [ $# -gt 0 ]; do
    case "$1" in
        --pairs)          pairs="${2:?}"; shift 2 ;;
        --cooldown-secs)  cooldown_secs="${2:?}"; shift 2 ;;
        --output)         output="${2:?}"; shift 2 ;;
        --dry-run)        dry_run=1; shift ;;
        -h|--help)        usage; exit 0 ;;
        *) printf 'Unknown flag: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done
[ "$pairs" -ge 2 ] || { echo "error: at least two pairs are needed" >&2; exit 2; }
[ -n "$output" ] || output="$ROOT/eval/results/instrument-ab/rpi5-$(date -u +%Y-%m-%dT%H-%M-%SZ)"

arm_order() {
    if [ $((10#$1 % 2)) -eq 1 ]; then echo "on off"; else echo "off on"; fi
}

if [ "$dry_run" -eq 1 ]; then
    printf 'experiment: instrument-ab\nevidence_class: diagnostic\nworkload: e-perf-1 wafer, %s, %s\n' "$config" "$profile"
    printf 'host state: broker up, kuiper.service stopped in both arms\n'
    printf 'cpusets: WAFER_RUNTIME_CPUSET=%s WAFER_LOADGEN_CPUSET=%s\n' "$sut_cpus" "$support_cpus"
    for pair in $(seq -f '%02g' 1 "$pairs"); do
        printf 'pair-%s: %s\n' "$pair" "$(arm_order "$pair")"
    done
    printf 'analysis: eval/scripts/analyze-instrument-ab.py %s\n' "$output"
    exit 0
fi

"$ROOT/eval/scripts/preflight-pi5.sh"
sudo systemctl stop kuiper.service
trap 'sudo systemctl start kuiper.service' EXIT
mkdir -p "$output"

first=1
for pair in $(seq -f '%02g' 1 "$pairs"); do
    for arm in $(arm_order "$pair"); do
        [ "$first" -eq 1 ] || sleep "$cooldown_secs"
        first=0
        leaf="$output/$arm/pair-$pair"
        sidecars=on
        [ "$arm" = "on" ] || sidecars=off
        if ! WAFER_HOST_SIDECARS="$sidecars" \
        WAFER_RUNTIME_CPUSET="$sut_cpus" \
        WAFER_LOADGEN_CPUSET="$support_cpus" \
            "$ROOT/eval/scripts/run-experiment.sh" \
                --config "$ROOT/$config" \
                --experiment e-perf-1 \
                --host rpi5 \
                --canonical \
                --defer-verification \
                --skip-build \
                --broker 127.0.0.1:1883 \
                --loadgen-profile "$ROOT/$profile" \
                --warmup-secs 30 \
                --measurement-secs 60 \
                --duration 120 \
                --total-messages 60000 \
                --output-dir "$leaf"; then
            echo "pair-$pair $arm failed; the analyzer will reject this pair" >&2
            mkdir -p "$leaf"
        fi
        printf '{"experiment":"instrument-ab","evidence_class":"diagnostic","thesis_evidence":false,"arm":"%s","pair":%d}\n' \
            "$arm" "$((10#$pair))" >"$leaf/instrument-ab.json"
        echo "pair-$pair $arm done"
    done
done

status=0
python3 "$ROOT/eval/scripts/analyze-instrument-ab.py" "$output" || status=$?
printf 'instrument A/B: %s\n' "$output"
exit "$status"
