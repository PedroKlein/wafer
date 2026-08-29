#!/usr/bin/env bash
set -euo pipefail

VERSION="2.1.0"
ARCHIVE="kuiper-${VERSION}-linux-arm64.deb"
BASE_URL="https://github.com/lf-edge/ekuiper/releases/download/v${VERSION}"
INSTALL_ROOT="/usr/lib/kuiper"
dry_run=0

render_service_override() {
    printf '%s\n' \
        '[Service]' \
        'User=kuiper' \
        'Group=kuiper' \
        'Type=simple' \
        'WorkingDirectory=/usr/lib/kuiper' \
        'ExecStart=' \
        'ExecStart=/usr/bin/kuiperd -loadFileType absolute' \
        'ExecStop=' \
        'Environment=HOME=/var/lib/kuiper' \
        'Environment=MQTT_SOURCE__DEFAULT__SERVER=tcp://127.0.0.1:1883' \
        'CPUAffinity=1 2 3' \
        'Restart=on-failure' \
        'RestartSec=2'
}

if [ "${1:-}" = "--dry-run" ]; then
    dry_run=1
elif [ "$#" -ne 0 ]; then
    echo "usage: $0 [--dry-run]" >&2
    exit 2
fi

cat <<PLAN
version: $VERSION
artifact: $ARCHIVE
artifact_url: $BASE_URL/$ARCHIVE
checksum_url: $BASE_URL/$ARCHIVE.sha256
install_root: $INSTALL_ROOT
service: kuiper.service
broker: tcp://127.0.0.1:1883
PLAN

if [ "$dry_run" -eq 1 ]; then
    echo 'service_override:'
    render_service_override
    exit 0
fi
[ "$(uname -m)" = "aarch64" ] || { echo "error: native eKuiper package requires aarch64" >&2; exit 1; }
command -v curl >/dev/null || { echo "error: curl is required" >&2; exit 1; }
command -v sha256sum >/dev/null || { echo "error: sha256sum is required" >&2; exit 1; }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fL "$BASE_URL/$ARCHIVE" -o "$tmp/$ARCHIVE"
curl -fL "$BASE_URL/$ARCHIVE.sha256" -o "$tmp/$ARCHIVE.sha256"
expected="$(tr -d '[:space:]' < "$tmp/$ARCHIVE.sha256")"
actual="$(sha256sum "$tmp/$ARCHIVE" | awk '{print $1}')"
[ "$actual" = "$expected" ] || {
    echo "error: checksum mismatch for $ARCHIVE" >&2
    exit 1
}
echo "$ARCHIVE: checksum OK"
sudo apt install -y "$tmp/$ARCHIVE"
sudo install -d -o kuiper -g kuiper \
    /var/lib/kuiper/data \
    /var/lib/kuiper/plugins \
    /var/log/kuiper
sudo chown -R kuiper:kuiper /var/lib/kuiper /var/log/kuiper /etc/kuiper
sudo install -d /etc/systemd/system/kuiper.service.d
render_service_override \
    | sudo tee /etc/systemd/system/kuiper.service.d/wafer-eval.conf >/dev/null
sudo systemctl daemon-reload
sudo systemctl reset-failed kuiper.service
sudo systemctl enable --now kuiper.service

for _ in $(seq 1 30); do
    if curl -fsS http://127.0.0.1:9081/ >/dev/null 2>&1; then
        installed_version="$(dpkg-query -W -f='${Version}' kuiper)"
        [ "$installed_version" = "$VERSION" ] || {
            echo "error: installed eKuiper version is $installed_version, expected $VERSION" >&2
            exit 1
        }
        echo "eKuiper $installed_version ready at http://127.0.0.1:9081"
        exit 0
    fi
    sleep 1
done

sudo systemctl status kuiper.service --no-pager --full >&2 || true
echo "error: eKuiper did not become ready at http://127.0.0.1:9081" >&2
exit 1
