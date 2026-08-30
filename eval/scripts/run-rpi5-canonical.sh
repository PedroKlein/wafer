#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
exec python3 "$ROOT/eval/scripts/lib/canonical_runner.py" --root "$ROOT" "$@"
