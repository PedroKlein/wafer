#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
INSTALL="$REPO_ROOT/eval/ekuiper/install-native.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
ARM64_SHA256="917579fdd8683e047c43d01a694180ae73ca234bd0727e11deb3a902e1556d49"
AMD64_SHA256="9ec8f68e23f507d0e02e6bb45ff61ed904443c84ef0f6288b00d6b19ea66f12b"

fake_uname() {
    printf '#!/usr/bin/env bash\n[ "${1:-}" = "-m" ] && { echo "%s"; exit 0; }\nexec /usr/bin/uname "$@"\n' "$1" > "$TMP/uname"
    chmod +x "$TMP/uname"
}

fake_uname aarch64
plan="$(PATH="$TMP:$PATH" "$INSTALL" --dry-run)"
grep -q '^artifact: kuiper-2.1.5-linux-arm64.deb$' <<<"$plan" || { echo "aarch64 did not select the arm64 package" >&2; exit 1; }
grep -q "^sha256: $ARM64_SHA256$" <<<"$plan" || { echo "arm64 package checksum is not pinned" >&2; exit 1; }

fake_uname arm64
plan="$(PATH="$TMP:$PATH" "$INSTALL" --dry-run)"
grep -q '^artifact: kuiper-2.1.5-linux-arm64.deb$' <<<"$plan" || { echo "arm64 (macOS) did not select the arm64 package" >&2; exit 1; }

fake_uname x86_64
plan="$(PATH="$TMP:$PATH" "$INSTALL" --dry-run)"
grep -q '^artifact: kuiper-2.1.5-linux-amd64.deb$' <<<"$plan" || { echo "x86_64 did not select the amd64 package" >&2; exit 1; }
grep -q "^sha256: $AMD64_SHA256$" <<<"$plan" || { echo "amd64 package checksum is not pinned" >&2; exit 1; }
grep -q '^CPUAffinity=1 2 3$' <<<"$plan" || { echo "service override changed" >&2; exit 1; }

fake_uname riscv64
if PATH="$TMP:$PATH" "$INSTALL" --dry-run >/dev/null 2>&1; then
    echo "unsupported architecture was accepted" >&2
    exit 1
fi

cat > "$TMP/curl" <<'CURL'
#!/usr/bin/env bash
out=""
url=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o) out="$2"; shift 2 ;;
        http*) url="$1"; shift ;;
        *) shift ;;
    esac
done
case "$url" in
    *.sha256) printf '%s\n' "$FAKE_PUBLISHED_SHA256" > "$out" ;;
    *.deb) printf 'not the release package\n' > "$out" ;;
esac
CURL
cat > "$TMP/sudo" <<'SUDO'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$SUDO_LOG"
[ "$1" = tee ] && cat >/dev/null
exit 0
SUDO
printf '#!/usr/bin/env bash\necho 2.1.5\n' > "$TMP/dpkg-query"
chmod +x "$TMP/curl" "$TMP/sudo" "$TMP/dpkg-query"
export SUDO_LOG="$TMP/sudo.log"
fake_uname aarch64

if output="$(FAKE_PUBLISHED_SHA256="$AMD64_SHA256" PATH="$TMP:$PATH" "$INSTALL" 2>&1)"; then
    echo "a published checksum that differs from the pin was accepted" >&2
    exit 1
fi
grep -q 'differs from the pinned one' <<<"$output" || { echo "unexpected failure: $output" >&2; exit 1; }

if output="$(FAKE_PUBLISHED_SHA256="$ARM64_SHA256" PATH="$TMP:$PATH" "$INSTALL" 2>&1)"; then
    echo "a package that does not match the pinned checksum was installed" >&2
    exit 1
fi
grep -q 'checksum mismatch for kuiper-2.1.5-linux-arm64.deb' <<<"$output" || { echo "unexpected failure: $output" >&2; exit 1; }
[ ! -e "$SUDO_LOG" ] || { echo "a rejected package reached a privileged step" >&2; exit 1; }

mkdir "$TMP/matching-hash"
printf '#!/usr/bin/env bash\necho "%s  $1"\n' "$ARM64_SHA256" > "$TMP/matching-hash/sha256sum"
chmod +x "$TMP/matching-hash/sha256sum"
FAKE_PUBLISHED_SHA256="$ARM64_SHA256" PATH="$TMP/matching-hash:$TMP:$PATH" "$INSTALL" >/dev/null
installed="$(grep -n '^apt install -y .*/kuiper-2.1.5-linux-arm64.deb$' "$SUDO_LOG" | cut -d: -f1 || true)"
restarted="$(grep -n '^systemctl restart kuiper.service$' "$SUDO_LOG" | cut -d: -f1 || true)"
[ -n "$installed" ] && [ -n "$restarted" ] && [ "$restarted" -gt "$installed" ] || {
    echo "the installer does not restart eKuiper after installing the package" >&2
    exit 1
}

printf 'ekuiper installer tests: PASS\n'
