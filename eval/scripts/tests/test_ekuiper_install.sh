#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
INSTALL="$REPO_ROOT/eval/ekuiper/install-native.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

fake_uname() {
    printf '#!/usr/bin/env bash\n[ "${1:-}" = "-m" ] && { echo "%s"; exit 0; }\nexec /usr/bin/uname "$@"\n' "$1" > "$TMP/uname"
    chmod +x "$TMP/uname"
}

fake_uname aarch64
plan="$(PATH="$TMP:$PATH" "$INSTALL" --dry-run)"
grep -q '^artifact: kuiper-2.1.0-linux-arm64.deb$' <<<"$plan" || { echo "aarch64 did not select the arm64 package" >&2; exit 1; }

fake_uname x86_64
plan="$(PATH="$TMP:$PATH" "$INSTALL" --dry-run)"
grep -q '^artifact: kuiper-2.1.0-linux-amd64.deb$' <<<"$plan" || { echo "x86_64 did not select the amd64 package" >&2; exit 1; }
grep -q '^CPUAffinity=1 2 3$' <<<"$plan" || { echo "service override changed" >&2; exit 1; }

fake_uname riscv64
if PATH="$TMP:$PATH" "$INSTALL" --dry-run >/dev/null 2>&1; then
    echo "unsupported architecture was accepted" >&2
    exit 1
fi

printf 'ekuiper installer tests: PASS\n'
