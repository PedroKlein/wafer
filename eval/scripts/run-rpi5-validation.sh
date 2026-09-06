#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
result="$ROOT/eval/results/e-val-1/rpi5-validation-$(date -u +%Y-%m-%dT%H-%M-%SZ)"
dry_run=0

if [ "${1:-}" = "--dry-run" ]; then
    dry_run=1
elif [ "$#" -ne 0 ]; then
    echo "usage: $0 [--dry-run]" >&2
    exit 2
fi

command=(
    "$ROOT/eval/scripts/run-experiment.sh"
    --config "$ROOT/eval/configs/pipeline-c-with-delay.toml"
    --experiment e-val-1
    --host rpi5
    --duration 45
    --broker 127.0.0.1:1883
    --skip-build
    --output-dir "$result"
)

if [ "$dry_run" -eq 1 ]; then
    printf 'WAFER_RUNTIME_CPUSET=1-3 WAFER_LOADGEN_CPUSET=0'
    printf ' %q' "${command[@]}"
    printf '\n'
    exit 0
fi

"$ROOT/eval/scripts/preflight-pi5.sh"
WAFER_RUNTIME_CPUSET=1-3 WAFER_LOADGEN_CPUSET=0 "${command[@]}"
[ -d "$result" ] || { echo "error: validation result directory was not created" >&2; exit 1; }

"$ROOT/target/release/wafer-loadgen" hdr-summary \
    --hdr "$result/latency.hdr" \
    --output "$result/percentiles.json"
python3 - "$result/percentiles.json" "$result/sequence.csv" <<'PY'
import csv
import json
import sys

with open(sys.argv[1]) as stream:
    summary = json.load(stream)
p99_ms = summary["p99_ns"] / 1_000_000
count = summary["total_count"]
if count <= 0:
    raise SystemExit("methodology validation failed: empty histogram")
if not 45 <= p99_ms <= 55:
    raise SystemExit(f"methodology validation failed: p99={p99_ms:.3f} ms")

with open(sys.argv[2], newline="") as stream:
    sequence = next(csv.DictReader(stream))
expected = int(sequence["total_expected"])
received = int(sequence["total_received"])
gaps = int(sequence["gap_msgs"])
duplicates = int(sequence["duplicates_count"])
if expected != received or gaps != 0 or duplicates != 0:
    raise SystemExit(
        "sequence integrity failed: "
        f"expected={expected}, received={received}, gaps={gaps}, duplicates={duplicates}"
    )
print(
    f"methodology validation: PASS "
    f"(samples={count}, p99={p99_ms:.3f} ms, messages={received}, gaps=0)"
)
PY

python3 "$ROOT/eval/scripts/verify-result-contract.py" "$result"
[ "$(vcgencmd get_throttled)" = "throttled=0x0" ] || {
    echo "error: throttling occurred during methodology validation" >&2
    exit 1
}
printf 'validation result: %s\n' "$result"
