# Prepare a Raspberry Pi 5 for WAFER evaluation

This how-to prepares a Raspberry Pi 5 with 4 GB RAM as the canonical WAFER host. It uses Raspberry Pi OS Lite 64-bit, native Mosquitto, native eKuiper, and no development toolchain or Docker.

## 1. Install Raspberry Pi OS Lite

Connect Ethernet, a display, and a keyboard. Start the Raspberry Pi network installer by holding **Shift** during boot, then select **Raspberry Pi OS (other) → Raspberry Pi OS Lite (64-bit)**. Installing erases the selected microSD or NVMe device.

Set the hostname to `wafer-pi5`, create the administrative user, select the correct timezone, and enable SSH. If network install is unavailable, update the bootloader from the existing system with `sudo rpi-eeprom-update -a`, reboot, and retry. Raspberry Pi Imager on another computer is the simpler alternative for a headless installation.

## 2. Update once and install runtime packages

```sh
sudo apt update
sudo apt full-upgrade -y
sudo apt install --no-install-recommends \
  ca-certificates curl jq rsync git python3 linux-cpupower \
  mosquitto mosquitto-clients
sudo systemctl enable --now ssh mosquitto
sudo reboot
```

Freeze this package state for one complete canonical batch. If the OS, kernel, firmware, WAFER binary, plugin, or eKuiper package changes, begin a new batch rather than combining results.

## 3. Configure SSH

Install and test a public key from a second terminal before disabling passwords. Then create `/etc/ssh/sshd_config.d/10-wafer.conf`:

```text
PasswordAuthentication no
PermitRootLogin no
```

Apply it:

```sh
sudo systemctl reload ssh
```

## 4. Reserve CPUs for the active system under test

Edit `/boot/firmware/cmdline.txt`. Keep the file on one line and append:

```text
isolcpus=1-3
```

Reboot and verify:

```sh
sudo reboot
cat /sys/devices/system/cpu/isolated
```

The expected value is `1-3`. CPU 0 is reserved for operating-system work, Mosquitto, and `wafer-loadgen`; CPUs 1–3 run exactly one of WAFER, the native Rust baseline, or eKuiper.

Pin Mosquitto to CPU 0:

```sh
sudo systemctl edit mosquitto
```

Add:

```ini
[Service]
CPUAffinity=0
```

Then apply it:

```sh
sudo systemctl daemon-reload
sudo systemctl restart mosquitto
```

## 5. Remove avoidable noise

Raspberry Pi OS Lite should not install a graphical session. Confirm:

```sh
systemctl get-default
systemctl --type=service --state=running
```

The default must be `multi-user.target`. If Bluetooth and mDNS are unused:

```sh
sudo systemctl disable --now bluetooth.service hciuart.service 2>/dev/null || true
sudo systemctl disable --now avahi-daemon.service avahi-daemon.socket 2>/dev/null || true
```

Keep networking, SSH, time synchronization, journald, AppArmor, EEPROM updates, filesystem maintenance, and thermal protection enabled.

## 6. Install native eKuiper

Clone or deploy the WAFER bundle first, then run on the Pi:

```sh
cd ~/wafer
./eval/ekuiper/install-native.sh --dry-run
./eval/ekuiper/install-native.sh
./eval/ekuiper/seed-pipeline-a.sh
./eval/ekuiper/smoke-test.sh
```

The installer verifies the official SHA256 before installing eKuiper 2.1.0. Docker is not installed or used.

## 7. Deploy WAFER from the development machine

Build the ARM64 binaries and evaluation plugins:

```sh
mise run cross-build-pi
mise run cross-build-pi-check
mise run //plugins:build-plugins
```

Preview and deploy:

```sh
./eval/scripts/deploy-pi5.sh --host USER@wafer-pi5 --dry-run
./eval/scripts/deploy-pi5.sh --host USER@wafer-pi5
```

The bundle lands in `~/wafer` and preserves the relative paths used by evaluation configs. Rust is not required on the Pi.

## 8. Enter benchmark mode

Use the stock 2.4 GHz ceiling. Do not set `force_turbo` or copy the old Pi 4 1.8 GHz setting.

```sh
sudo cpupower frequency-set -g performance
sudo systemctl stop apt-daily.timer apt-daily-upgrade.timer
sudo systemctl stop apt-daily.service apt-daily-upgrade.service 2>/dev/null || true
```

Verify cooling and power:

```sh
vcgencmd measure_temp
vcgencmd get_throttled
```

`get_throttled` must report `throttled=0x0`. Active cooling is required. The retained 5 V / 4.2 A supply is admitted only by measured host gates and receives no threshold waiver: any nonzero throttling, temperature at or above the declared limit, reboot, kernel I/O error, or checksum mismatch stops admission.

## 9. Mount the single results volume

Enhanced v8 evidence uses one physical exFAT filesystem labeled `WAF_RESULTS`; the label fits exFAT's 11 UTF-16 code-unit limit. The Pi and Jetson mount it at `/mnt/wafer-results`; macOS mounts the same volume at `/Volumes/WAF_RESULTS`. The volume contains `raw/`, `manifests/`, `derived/`, and `reports/`. Evidence manifests store paths relative to this volume root so the same manifest verifies on every host.

Do not format or relabel a device from this guide. Formatting requires the separate destructive-operation gate and a fresh confirmation of the exact device identity. Before any run, verify the expected UUID, label, filesystem, mount path, free space, and read/write state. Create raw attempts additively; never overwrite an existing path. exFAT does not preserve POSIX ownership semantics, so admission depends on path identity and checksums rather than mode bits, hardlinks, or symlinks.

Before moving the drive, stop all writers, run `sync`, and unmount it cleanly. After each mount or host transition, confirm the UUID and label and verify the complete SHA-256 manifest before exposing `raw/` to analysis. Analysis opens `raw/` read-only and writes only under `derived/` and `reports/`. Never copy the raw tree to the SD card, Mac internal storage, or another removable volume.

Qualification is staged and non-destructive. The tool never formats, relabels, mounts, unmounts, copies, or deletes the volume. First capture the mounted-device facts and review the stable by-id name and UUID before creating the bounded test corpus:

```sh
./eval/scripts/qualify-results-storage.sh facts \
  --results-root /mnt/wafer-results \
  > /tmp/wafer-results-before.json
./eval/scripts/qualify-results-storage.sh prepare \
  --results-root /mnt/wafer-results \
  --facts-json /tmp/wafer-results-before.json \
  --expected-device-id 'by-id:<approved-Kingston-partition-id>' \
  --expected-uuid '<approved-exFAT-UUID>' \
  --qualification-id '<source-bound-id>' \
  --min-free-bytes '<required-campaign-bytes>'
```

`prepare` fails before writing if the stable device ID, UUID, label, exFAT type, mount path, read-write state, path identity, or free-space margin differs. Its corpus is stored under `manifests/storage-qualification/<id>/`, never under `raw/`. It writes one large file, 1,024 small files, their SHA-256 manifest, exact file/byte counts, and a `prepared.json` receipt, then calls `sync`.

Next stop every writer, run `sync`, unmount the volume with the host's normal safe-eject procedure, remount it at `/mnt/wafer-results`, and collect fresh facts. The tool does not perform this operator step. Verification requires a changed mount identity and rehashes every corpus file:

```sh
./eval/scripts/qualify-results-storage.sh facts \
  --results-root /mnt/wafer-results \
  > /tmp/wafer-results-after.json
./eval/scripts/qualify-results-storage.sh verify-remount \
  --results-root /mnt/wafer-results \
  --facts-json /tmp/wafer-results-after.json \
  --prepared-receipt /mnt/wafer-results/manifests/storage-qualification/<id>/prepared.json
```

The resulting `verified.json` records expected/observed file and byte counts, missing, extra, and mismatch counts, and the before/after mount identities. Any non-zero count blocks use of the volume.

After storage qualification and a fresh reboot, record the boot ID and run the
stop-on-first-failure host load ladder. Do not use the historical
`.plans/rpi5-host-diagnostic/run_phase.sh`; it predates the exFAT evidence
contract and writes to its local plan directory.

```sh
boot_id="$(cat /proc/sys/kernel/random/boot_id)"
session_id="host-characterization-$(date -u +%Y%m%dT%H%M%SZ)"
./eval/scripts/characterize-rpi5-host.sh \
  --output-dir "/mnt/wafer-results/raw/e-host-thermal-storage/$session_id" \
  --session-id "$session_id" \
  --expected-boot-id "$boot_id"
```

Start within ten minutes of the reboot with `vcgencmd get_throttled` equal to
`throttled=0x0`. The fixed sequence is 120 seconds idle followed by 300 seconds
each of one-, two-, and three-SUT-core CPU load, CPU plus memory, USB write, USB
read, and combined CPU plus memory plus USB. One-second samples record
wall-clock and monotonic time, boot ID, temperature, CPU frequency, throttling,
PMIC internal-rail proxy watts, memory availability and PSI, and USB throughput.
The PMIC value is not total input power and excludes direct USB-device draw.

The command exits immediately on temperature at or above 75 °C, any non-zero
throttling value, boot-ID change, kernel I/O error, workload or instrumentation
failure, or USB SHA-256 mismatch. It writes the partial receipt and marks later
phases `not-run`; retry with a new session ID after correcting the failure. It
never overwrites a prior session. N=5 requires the diagnostic phases through
USB read to pass. N=30 additionally requires the maximum combined-load phase,
so a failure there does not erase accepted diagnostic evidence but keeps final
admission blocked. The script's fixture mode is for local contract tests only;
its receipts set `execution_mode=fixture-synthetic` and can never grant either
admission gate.

For a macOS handoff, eject the volume on the Pi and mount it at `/Volumes/WAF_RESULTS`. The facts receipt binds the macOS disk identifier plus the same exFAT UUID and label; it does not reuse a Linux `/dev/disk/by-id` path:

```sh
./eval/scripts/qualify-results-storage.sh facts \
  --results-root /Volumes/WAF_RESULTS \
  > /tmp/wafer-results-macos.json
./eval/scripts/qualify-results-storage.sh handoff \
  --results-root /Volumes/WAF_RESULTS \
  --facts-json /tmp/wafer-results-macos.json \
  --verified-receipt /Volumes/WAF_RESULTS/manifests/storage-qualification/<id>/verified.json \
  --manifest /Volumes/WAF_RESULTS/manifests/expanded-n5.sha256 \
  --analysis-output /Volumes/WAF_RESULTS/derived/expanded-n5 \
  --host macos
```

On Jetson, use `/mnt/wafer-results`, capture Linux facts, and pass `--host jetson`. Both handoffs verify the same volume-relative `raw/` manifest in place. The handoff receipt declares raw input read-only and rejects any analysis output outside `derived/` or `reports/`. Do not proceed if facts capture, corpus verification, or the raw manifest check fails.

## 10. Run preflight and smoke

```sh
cd ~/wafer
./eval/scripts/preflight-pi5.sh
./eval/scripts/run-rpi5-smoke.sh
./eval/scripts/run-rpi5-validation.sh
```

Preflight must report zero failures. The smoke command prints a result directory under the selected results root and runs the result-contract verifier against it. Repository-local `eval/results/` remains a local-test fallback, not the approved v8 campaign storage path.

## Final checklist

These commands must succeed before longer test runs:

```sh
[ "$(uname -m)" = aarch64 ]
[ "$(cat /sys/devices/system/cpu/isolated)" = 1-3 ]
[ "$(sort -u /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor)" = performance ]
[ "$(vcgencmd get_throttled)" = throttled=0x0 ]
systemctl is-active --quiet mosquitto kuiper
findmnt /mnt/wafer-results
[ "$(findmnt -n -o FSTYPE /mnt/wafer-results)" = exfat ]
[ "$(lsblk -no LABEL "$(findmnt -n -o SOURCE /mnt/wafer-results)")" = WAF_RESULTS ]
./eval/scripts/preflight-pi5.sh
```

Smoke results validate installation only. They are not canonical thesis measurements. The methodology-validation command additionally checks that a known 50 ms injected delay appears at p99 within the predeclared 45–55 ms honesty window.
