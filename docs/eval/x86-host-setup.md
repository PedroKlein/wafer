# x86_64 host setup

Read-only preflight: `./eval/scripts/preflight-x86.sh` (from the deployed
`~/wafer`). It checks the items below and exits non-zero when one fails.

## Expected state

| Item | Expected | How |
|------|----------|-----|
| SMT | off | `echo off \| sudo tee /sys/devices/system/cpu/smt/control` |
| Turbo | off | Intel: `echo 1 \| sudo tee /sys/devices/system/cpu/intel_pstate/no_turbo`; AMD: `echo 0 \| sudo tee /sys/devices/system/cpu/cpufreq/boost` |
| Isolated CPUs | `1-3` (three physical cores for the SUT, core 0 for the broker and load generator) | `isolcpus=1-3` on the kernel command line, then reboot |
| Governor | `performance` on every CPU | `sudo cpupower frequency-set -g performance` |
| Clocks | `scaling_cur_freq` within 5% of `scaling_max_freq` on CPUs 1-3 | follows from the governor and turbo settings |
| Temperature | `x86_pkg_temp` thermal zone (Intel) or `k10temp`/`coretemp` hwmon | kernel modules `coretemp` or `k10temp` |
| Binaries | built on this host | `mise run build-release-x86` |
| Services | Mosquitto and native eKuiper 2.1.0 | `eval/ekuiper/install-native.sh` picks the `amd64` package |

Deploy the evaluation tree with
`./eval/scripts/deploy-pi5.sh --host user@box --bin-dir target/release` from
a checkout on the machine, or run from the checkout directly with
`WAFER_PI_ROOT` pointing at it.
