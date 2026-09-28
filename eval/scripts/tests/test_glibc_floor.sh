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

printf 'glibc floor tests: PASS\n'
