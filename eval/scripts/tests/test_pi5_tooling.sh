#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

deploy="$("$ROOT/eval/scripts/deploy-pi5.sh" --host pi@example --dry-run)"
grep -q 'remote_root: ~/wafer' <<<"$deploy"
grep -q 'source_dirty:' <<<"$deploy"
grep -q 'source_tags:' <<<"$deploy"
grep -q 'target/release/wafer-loadgen' <<<"$deploy"
grep -q 'plugins/\*/target/wasm32-wasip2/release/\*.wasm' <<<"$deploy"
grep -q 'eval/canonical-matrix.json' <<<"$deploy"

smoke="$("$ROOT/eval/scripts/run-rpi5-smoke.sh" --dry-run)"
grep -q 'WAFER_RUNTIME_CPUSET=1-3' <<<"$smoke"
grep -q -- '--host rpi5' <<<"$smoke"
grep -q 'pipeline-c-rpi5-smoke.toml' <<<"$smoke"

validation="$("$ROOT/eval/scripts/run-rpi5-validation.sh" --dry-run)"
grep -q 'pipeline-c-with-delay.toml' <<<"$validation"
grep -q -- '--experiment e-val-1' <<<"$validation"

if WAFER_PI_ROOT="$ROOT/.definitely-missing-pi-root" \
    "$ROOT/eval/scripts/preflight-pi5.sh" >"$tmp/preflight-negative.log" 2>&1; then
    echo 'preflight unexpectedly passed on the development host' >&2
    exit 1
fi
grep -q '^FAIL  Raspberry Pi 5 hardware' "$tmp/preflight-negative.log"
grep -q '^FAIL  wafer deployed' "$tmp/preflight-negative.log"

result="$tmp/e-smoke/rpi5-2026-01-01T00-00-00Z"
mkdir -p "$result"
touch "$result/config.toml" "$result/stdout.log"
cat >"$result/metadata.json" <<'JSON'
{
  "host_tag": "rpi5",
  "hardware_model": "Raspberry Pi 5 Model B Rev 1.0",
  "arch": "aarch64",
  "isolated_cpus": "1-3",
  "cpu_governors": ["performance"],
  "throttled": "0x0",
  "git_sha": "1111111111111111111111111111111111111111",
  "git_dirty": true,
  "exit_codes": {"wafer_runtime": 0}
}
JSON
python3 "$ROOT/eval/scripts/verify-result-contract.py" "$result" >/dev/null
python3 - "$result/metadata.json" <<'PY'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
metadata = json.loads(path.read_text())
metadata["git_sha"] = "unknown"
path.write_text(json.dumps(metadata))
PY
if python3 "$ROOT/eval/scripts/verify-result-contract.py" "$result" >"$tmp/contract-negative.log" 2>&1; then
    echo 'result verifier accepted missing Pi 5 source provenance' >&2
    exit 1
fi
grep -q 'Pi 5 metadata lacks a source commit SHA' "$tmp/contract-negative.log"

echo 'Pi 5 tooling tests: PASS'
