#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ARGS=(--root "$ROOT")
if [[ -n "${WAFER_RESULTS_ROOT:-}" ]]; then
  ARGS+=(--results-root "$WAFER_RESULTS_ROOT")
fi
exec python3 "$ROOT/eval/scripts/lib/canonical_runner.py" "${ARGS[@]}" "$@"
