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

`get_throttled` must report `throttled=0x0`. Active cooling and the official Pi 5 power supply are required for sustained runs.

## 9. Run preflight and smoke

```sh
cd ~/wafer
./eval/scripts/preflight-pi5.sh
./eval/scripts/run-rpi5-smoke.sh
./eval/scripts/run-rpi5-validation.sh
```

Preflight must report zero failures. The smoke command prints a result directory under `eval/results/e-smoke/rpi5-<timestamp>/` and runs the result-contract verifier against it.

## Final checklist

These commands must succeed before longer test runs:

```sh
[ "$(uname -m)" = aarch64 ]
[ "$(cat /sys/devices/system/cpu/isolated)" = 1-3 ]
[ "$(sort -u /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor)" = performance ]
[ "$(vcgencmd get_throttled)" = throttled=0x0 ]
systemctl is-active --quiet mosquitto kuiper
./eval/scripts/preflight-pi5.sh
```

Smoke results validate installation only. They are not canonical thesis measurements. The methodology-validation command additionally checks that a known 50 ms injected delay appears at p99 within the predeclared 45–55 ms honesty window.
