#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
COLLECT="$REPO_ROOT/eval/scripts/collect-results.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

rpi5_path="$($COLLECT --experiment e-test --host rpi5 --root "$TMP" --print-only)"
case "$rpi5_path" in
    "$TMP/e-test/rpi5-"*) ;;
    *) printf 'unexpected rpi5 path: %s\n' "$rpi5_path" >&2; exit 1 ;;
esac

rpi4_path="$($COLLECT --experiment e-test --host rpi4 --root "$TMP" --print-only)"
case "$rpi4_path" in
    "$TMP/e-test/rpi4-"*) ;;
    *) printf 'legacy rpi4 tag stopped resolving: %s\n' "$rpi4_path" >&2; exit 1 ;;
esac

if "$COLLECT" --experiment e-test --host unknown --root "$TMP" --print-only >/dev/null 2>&1; then
    echo 'unknown host tag was accepted' >&2
    exit 1
fi

printf 'host tag tests: PASS\n'
