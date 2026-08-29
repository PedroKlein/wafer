#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
dry_run=0

if [ "${1:-}" = "--dry-run" ]; then
    dry_run=1
elif [ "$#" -ne 0 ]; then
    echo "usage: $0 [--dry-run]" >&2
    exit 2
fi

command=(
    "$ROOT/eval/scripts/run-experiment.sh"
    --config "$ROOT/eval/configs/pipeline-c-rpi5-smoke.toml"
    --experiment e-smoke
    --host rpi5
    --duration 15
    --broker 127.0.0.1:1883
    --skip-build
)

if [ "$dry_run" -eq 1 ]; then
    printf 'WAFER_RUNTIME_CPUSET=1-3 WAFER_LOADGEN_CPUSET=0'
    printf ' %q' "${command[@]}"
    printf '\n'
    exit 0
fi

"$ROOT/eval/scripts/preflight-pi5.sh"
before="$(vcgencmd get_throttled)"
[ "$before" = "throttled=0x0" ] || { echo "error: throttling present before smoke: $before" >&2; exit 1; }

WAFER_RUNTIME_CPUSET=1-3 WAFER_LOADGEN_CPUSET=0 "${command[@]}"
result="$(find "$ROOT/eval/results/e-smoke" -mindepth 1 -maxdepth 1 -type d -name 'rpi5-*' | sort | tail -1)"
[ -n "$result" ] || { echo "error: smoke result directory was not created" >&2; exit 1; }
python3 "$ROOT/eval/scripts/verify-result-contract.py" "$result"
python3 - "$result/sequence.csv" <<'PY'
import csv
import sys

with open(sys.argv[1], newline="") as stream:
    row = next(csv.DictReader(stream))
expected = int(row["total_expected"])
received = int(row["total_received"])
gaps = int(row["gap_msgs"])
duplicates = int(row["duplicates_count"])
if expected != received or gaps != 0 or duplicates != 0:
    raise SystemExit(
        "sequence integrity failed: "
        f"expected={expected}, received={received}, gaps={gaps}, duplicates={duplicates}"
    )
print(f"sequence integrity: PASS ({received} messages)")
PY

after="$(vcgencmd get_throttled)"
[ "$after" = "throttled=0x0" ] || { echo "error: throttling occurred during smoke: $after" >&2; exit 1; }
printf 'smoke result: %s\n' "$result"
