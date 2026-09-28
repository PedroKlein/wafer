#!/usr/bin/env bash
# Print the highest GLIBC symbol version each ELF binary requires, and fail
# when --max is given and a binary needs a newer glibc than that. readelf
# reads any ELF architecture, so aarch64 binaries can be checked on x86 hosts.
set -euo pipefail

max=""
if [ "${1:-}" = "--max" ]; then
    max="${2:?--max needs a version such as 2.35}"
    shift 2
fi
[ "$#" -gt 0 ] || { echo "usage: $0 [--max VERSION] BINARY..." >&2; exit 2; }

status=0
for binary in "$@"; do
    [ -f "$binary" ] || { echo "MISSING: $binary" >&2; status=1; continue; }
    required="$( { readelf -W --dyn-syms "$binary" 2>/dev/null || true; } | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/^GLIBC_//' | sort -uV | tail -n 1 || true)"
    [ -n "$required" ] || { echo "NO GLIBC SYMBOLS: $binary (static or not an ELF binary)" >&2; status=1; continue; }
    if [ -n "$max" ] && [ "$(printf '%s\n%s\n' "$required" "$max" | sort -V | tail -n 1)" != "$max" ]; then
        echo "TOO NEW: $binary requires glibc $required, allowed up to $max" >&2
        status=1
        continue
    fi
    echo "$binary: glibc $required"
done
exit "$status"
