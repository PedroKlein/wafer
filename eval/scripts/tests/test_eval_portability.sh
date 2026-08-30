#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$ROOT"

local_home='/''Users/'
private_lan='192''\.168\.|10''\.172\.'
pi_login='waferpi''@'
if matches="$(git grep -nE "$local_home|$private_lan|$pi_login" -- eval docs mise.toml ':!eval/results' || true)" && [ -n "$matches" ]; then
    echo 'tracked evaluation files contain local host details:' >&2
    echo "$matches" >&2
    exit 1
fi

echo 'evaluation path portability tests: PASS'
