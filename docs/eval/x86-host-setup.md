# x86_64 host setup

Read-only preflight: `mise run preflight-x86` or `./eval/scripts/preflight-x86.sh` (from the deployed
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
| Services | Mosquitto and native eKuiper 2.1.5 | `eval/ekuiper/install-native.sh` picks the `amd64` package |

Deploy the evaluation tree with
`./eval/scripts/deploy-pi5.sh --host user@box --bin-dir target/release` from
a checkout on the machine, or run from the checkout directly with
`WAFER_PI_ROOT` pointing at it.

## Telemetry

`eval/scripts/lib/pi_telemetry.py` picks its `x86` backend on an `x86_64`
host without `vcgencmd`. Power comes from RAPL package energy under
`/sys/class/powercap/intel-rapl:*` (readable by root, or after
`chmod a+r .../energy_uj`), so the first sample of each run has no power
reading and a host without RAPL records power as unavailable. The temperature
is the `x86_pkg_temp` zone, or `k10temp`/`coretemp` through hwmon, and the
throttle signal is a core below 95% of its pinned clock or a moved
`thermal_throttle/core_throttle_count`. `power-boundary.json` records
`x86-rapl-package-energy` or `unavailable`.

## Running a batch

The host profile lives in the `hosts` map of `eval/canonical-matrix.json`.
Pass it to the runner and the validator; results land under
`x86-<batch-id>` directories and are checked against this profile only:

```sh
python3 eval/scripts/validate-canonical.py host --host x86 --require-ekuiper
python3 eval/scripts/lib/canonical_runner.py --host x86 --batch-id <batch-id> --dry-run
```

`mise run plan-campaign -- --host x86` prints the same schedule, and
`mise run run-campaign -- --host x86 --batch-id <batch-id>` runs or resumes it.
When it has finished, `mise run approve-batch -- --host x86 --batch-id <batch-id>`
on the analysis machine records it as the x86 entry of `eval/final-batches.json`.
