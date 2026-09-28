#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
CHECK="$REPO_ROOT/scripts/glibc-floor.sh"
BINARY="$(command -v bash)"

report="$("$CHECK" "$BINARY")"
case "$report" in
    "$BINARY: glibc 2."*) ;;
    *) printf 'unexpected report: %s\n' "$report" >&2; exit 1 ;;
esac
required="${report##*glibc }"

"$CHECK" --max "$required" "$BINARY" >/dev/null
"$CHECK" --max 99.0 "$BINARY" >/dev/null

if "$CHECK" --max 2.0 "$BINARY" >/dev/null 2>&1; then
    echo "a binary newer than the floor was accepted" >&2
    exit 1
fi

if "$CHECK" "$REPO_ROOT/does-not-exist" >/dev/null 2>&1; then
    echo "a missing binary was accepted" >&2
    exit 1
fi

report="$("$CHECK" "$REPO_ROOT/mise.toml" "$BINARY" 2>&1 || true)"
grep -q "^NO GLIBC SYMBOLS: $REPO_ROOT/mise.toml" <<<"$report" || { echo "non-ELF input was not reported" >&2; exit 1; }
grep -q "^$BINARY: glibc 2\." <<<"$report" || { echo "binaries after a bad input were not reported" >&2; exit 1; }
if "$CHECK" "$REPO_ROOT/mise.toml" "$BINARY" >/dev/null 2>&1; then
    echo "a non-ELF input did not fail the check" >&2
    exit 1
fi

printf 'glibc floor tests: PASS\n'
