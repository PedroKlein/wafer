# x86_64 host setup

Read-only preflight: `mise run preflight-x86` or `./eval/scripts/preflight-x86.sh` (from the deployed
`~/wafer`). It checks the items below and exits non-zero when one fails.

## Expected state

| Item | Expected | How |
|------|----------|-----|
| SMT | off | `echo off \| sudo tee /sys/devices/system/cpu/smt/control` |
| Turbo | off | Intel: `echo 1 \| sudo tee /sys/devices/system/cpu/intel_pstate/no_turbo`; AMD: `echo 0 \| sudo tee /sys/devices/system/cpu/cpufreq/boost` |
| CPU 0 for everything but the SUT | systemd affinity `0`; no isolated CPUs (cores 1-3 run the SUT, core 0 the broker, load generator and samplers) | `CPUAffinity=0` under `[Manager]` in `/etc/systemd/system.conf`, then reboot; drop any `isolcpus=` from the kernel command line |
| IRQs on CPU 0 | `/proc/irq/default_smp_affinity` is CPU 0 only | add `irqaffinity=0` to `GRUB_CMDLINE_LINUX` in `/etc/default/grub`, run `sudo update-grub`, then reboot |
| SUT CPUs balanced | three busy loops under `taskset -c 1-3` run on three CPUs | follows from the two rows above |
| Governor | `performance` on every online CPU | `sudo cpupower -c 0-3 frequency-set -g performance` |
| Clocks | `scaling_min_freq` equals `scaling_max_freq` on CPUs 0-3, and busy CPUs 1-3 report `scaling_cur_freq` within 5% of that limit during preflight | after disabling SMT and turbo, set `max=$(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_max_freq)` and run `sudo cpupower -c 0-3 frequency-set -d "${max}kHz" -u "${max}kHz"` |
| Temperature | `x86_pkg_temp` thermal zone (Intel) or `k10temp`/`coretemp` hwmon | kernel modules `coretemp` or `k10temp` |
| Power | RAPL package `energy_uj` readable by the user that runs the batch; without it every run fails its power check | `sudo chmod a+r /sys/class/powercap/intel-rapl:*/energy_uj`, again after each reboot |
| Binaries | built on this host | `mise run build-release-x86` |
| Services | Mosquitto and native eKuiper 2.1.5 | `eval/ekuiper/install-native.sh` picks the `amd64` package |

Repeat the governor and minimum/maximum frequency commands after every boot. In
active mode, `intel_pstate` defines `scaling_cur_freq` as an average P-state between
scheduler utilization callbacks, so an idle core can transiently report below the
requested minimum. Preflight checks `scaling_cur_freq` while CPUs 1-3 are busy.
During a run, frequency remains audit data while admission continuously requires the
`performance` governor and equal minimum/maximum limits and watches the hardware
core and package throttle counters. See the kernel documentation for
[`intel_pstate` policy attributes](https://www.kernel.org/doc/html/latest/admin-guide/pm/intel_pstate.html#interpretation-of-policy-attributes).

The runner starts WAFER and the native baseline with `taskset -c 1-3` and pins
`wafer-loadgen` and both telemetry samplers to CPU 0; the eKuiper unit sets
`CPUAffinity=1 2 3`, and Mosquitto inherits CPU 0 from systemd. Do not use
`isolcpus`: its default domain isolation stops load balancing on CPUs 1-3, so
every thread of a system under test would stay on the single CPU its process
started on.

Deploy the evaluation tree with
`./eval/scripts/deploy-pi5.sh --host user@box --bin-dir target/release` from
a checkout on the machine, or run from the checkout directly with
`WAFER_PI_ROOT` pointing at it.

E-Density-1 on this host uses the x86_64 container floor,
`eval/container-floor/linux-amd64.json`. Measure it with
`python3 eval/scripts/measure-container-floor.py --platform linux/amd64` and
commit it before deploying, as described in the
[runbook](pi5-experiment-runbook.md).

## Telemetry

`eval/scripts/lib/pi_telemetry.py` picks its `x86` backend on an `x86_64`
host without `vcgencmd`. Power comes from RAPL package energy under
`/sys/class/powercap/intel-rapl:*` (readable by root, or after
`chmod a+r .../energy_uj`), so the first sample of each run has no power
reading and a host without RAPL records power as unavailable. The temperature
is the `x86_pkg_temp` zone, or `k10temp`/`coretemp` through hwmon. Admission
fails if an online clock policy disappears, its governor changes from
`performance`, its minimum and maximum limits become unreadable or unequal, or a
`thermal_throttle/core_throttle_count` or
`thermal_throttle/package_throttle_count` increases. Per-core
`scaling_cur_freq` remains in the audit telemetry but is not itself an x86
throttle verdict. `power-boundary.json` records `x86-rapl-package-energy` or
`unavailable`.

## Running a batch

Before each batch on this host, re-check the eKuiper comparator as the
[runbook](pi5-experiment-runbook.md#re-check-the-ekuiper-comparator-before-each-batch)
describes, with `--host x86`.

The host profile lives in the `hosts` map of `eval/canonical-matrix.json`.
Pass it to the runner and the validator; results land under
`x86-<batch-id>` directories and are checked against this profile only.
E-Perf-10 adds this host's bracket rates, so run the host's
[capacity scout](pi5-experiment-runbook.md#run-the-hosts-capacity-scout) first:
repeat `mise run run-campaign -- --host x86 --capacity-scout --batch-id <scout-id>`
until it prints `"action": "stop"`. Then:

```sh
python3 eval/scripts/validate-canonical.py host --host x86 --require-ekuiper
python3 eval/scripts/lib/canonical_runner.py --host x86 --batch-id <batch-id> \
  --scout-batch-id <scout-id> --dry-run
```

`mise run plan-campaign -- --host x86 --batch-id <batch-id> --scout-batch-id <scout-id>`
prints the same schedule, and
`mise run run-campaign -- --host x86 --batch-id <batch-id> --scout-batch-id <scout-id>`
runs or resumes it. A final batch without `--scout-batch-id` refuses to start E-Perf-10.
When it has finished, `mise run approve-batch -- --host x86 --batch-id <batch-id>`
on the analysis machine records it as the x86 entry of `eval/final-batches.json`.
