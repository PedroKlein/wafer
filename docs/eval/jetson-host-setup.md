# Jetson Orin Nano host setup

Read-only preflight: `./eval/scripts/preflight-jetson.sh` (from the deployed
`~/wafer`). It checks the items below and exits non-zero when one fails.

## Expected state

| Item | Expected | How |
|------|----------|-----|
| OS | L4T R36 (Ubuntu 22.04, glibc 2.35) | `cat /etc/nv_tegra_release` |
| Power mode | one fixed `nvpmodel` mode for the whole batch; default `25W` (`WAFER_JETSON_POWER_MODE` overrides) | `sudo nvpmodel -m <id>`, then `sudo jetson_clocks` |
| Online CPUs | `0-3`: cores 4 and 5 offline, so the SUT budget matches the Pi 5's three cores | `echo 0 \| sudo tee /sys/devices/system/cpu/cpu{4,5}/online` |
| Isolated CPUs | `1-3` | `isolcpus=1-3` in `/boot/extlinux/extlinux.conf` `APPEND`, then reboot |
| Governor | `performance` on every online CPU | `jetson_clocks` sets it; verify with `cat /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor` |
| Clocks | `scaling_cur_freq` within 5% of `scaling_max_freq` on CPUs 1-3 | `jetson_clocks` |
| Thermal | a `CPU-therm` thermal zone | present on L4T |
| Power rails | INA3221 through hwmon | present on the developer kit |
| Binaries | built on this host | `cargo build --locked --release -p wafer-runtime -p wafer-loadgen -p waferctl`, then `mise run glibc-floor -- --max 2.35 target/release/wafer` |
| Services | Mosquitto and native eKuiper 2.1.0 | `eval/ekuiper/install-native.sh` |

Do not deploy the aarch64 binaries from CI or from `cross-build-pi`: they
need a newer glibc than L4T ships (see
[cross-compile](cross-compile.md#which-glibc-a-binary-needs)). Deploy the
plugins and the evaluation tree with
`./eval/scripts/deploy-pi5.sh --host user@jetson --bin-dir target/release`
from a checkout on the Jetson, or copy the same `.wasm` files the Pi uses.

The GPU is not used: the campaign build has no `cuda` feature and every
inference node runs on the CPU execution target, the same as on the Pi.
