# Jetson Orin Nano host setup

Read-only preflight: `mise run preflight-jetson` or `./eval/scripts/preflight-jetson.sh` (from the deployed
`~/wafer`). It checks the items below and exits non-zero when one fails.

## Expected state

| Item | Expected | How |
|------|----------|-----|
| OS | L4T R36 (Ubuntu 22.04, glibc 2.35) | `cat /etc/nv_tegra_release` |
| Power mode | one fixed `nvpmodel` mode for the whole batch; default `25W` (`WAFER_JETSON_POWER_MODE` overrides) | `sudo nvpmodel -m <id>`, then `sudo jetson_clocks` |
| Online CPUs | `0-3`: cores 4 and 5 offline, so the SUT budget matches the Pi 5's three cores | `echo 0 \| sudo tee /sys/devices/system/cpu/cpu{4,5}/online` |
| CPU 0 for everything but the SUT | systemd affinity `0`; no isolated CPUs | `CPUAffinity=0` under `[Manager]` in `/etc/systemd/system.conf`, then reboot; drop any `isolcpus=` from the kernel command line |
| IRQs on CPU 0 | `/proc/irq/default_smp_affinity` is CPU 0 only | add `irqaffinity=0` to `APPEND` in `/boot/extlinux/extlinux.conf`, then reboot |
| SUT CPUs balanced | three busy loops under `taskset -c 1-3` run on three CPUs | follows from the two rows above |
| Governor | `performance` on every online CPU | `jetson_clocks` sets it; verify with `cat /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor` |
| Clocks | `scaling_cur_freq` within 5% of `scaling_max_freq` on CPUs 1-3 | `jetson_clocks` |
| Thermal | a `cpu-thermal` thermal zone (`CPU-therm` on L4T R32) | present on L4T |
| Power rails | INA3221 through hwmon | present on the developer kit |
| Binaries | built on this host | `cargo build --locked --release -p wafer-runtime -p wafer-loadgen -p waferctl`, then `mise run glibc-floor -- --max 2.35 target/release/wafer` |
| Services | Mosquitto and native eKuiper 2.1.5 | `eval/ekuiper/install-native.sh` |

The runner starts WAFER and the native baseline with `taskset -c 1-3` and pins
`wafer-loadgen` and both telemetry samplers to CPU 0; the eKuiper unit sets
`CPUAffinity=1 2 3`, and Mosquitto inherits CPU 0 from systemd. Do not use
`isolcpus`: its default domain isolation stops load balancing on CPUs 1-3, so
every thread of a system under test would stay on the single CPU its process
started on.

Do not deploy the aarch64 binaries from CI or from `cross-build-pi`: they
need a newer glibc than L4T ships (see
[cross-compile](cross-compile.md#which-glibc-a-binary-needs)). Deploy the
plugins and the evaluation tree with
`./eval/scripts/deploy-pi5.sh --host user@jetson --bin-dir target/release`
from a checkout on the Jetson, or copy the same `.wasm` files the Pi uses.

The GPU is not used: the campaign build has no `cuda` feature and every
inference node runs on the CPU execution target, the same as on the Pi.

## Telemetry

`eval/scripts/lib/pi_telemetry.py` picks its `jetson` backend when
`/etc/nv_tegra_release` exists and no `vcgencmd` is on the path. It writes the
same `pi-telemetry.csv`, `pmic-rails.csv` and `power-boundary.json` as on the
Pi, with the INA3221 module input rail (`VDD_IN`) from `/sys/class/hwmon` as
the power source, the `cpu-thermal` zone as the temperature and a core running
below 95% of its pinned clock as the throttle signal. `power-boundary.json` records
`jetson-ina3221-rail-proxy`, so these watts are never compared with Pi PMIC
watts.

## Running a batch

Set `WAFER_RESULTS_ROOT` to this host's own mounted or bind-mounted results root
before any batch command, as the
[runbook](pi5-experiment-runbook.md#set-the-hosts-results-root) describes.
Approval and analysis read the batch from one root on the analysis machine that
holds every host's copied results
([runbook](pi5-experiment-runbook.md#bring-the-host-results-together)).

Before each batch on this host, re-check the eKuiper comparator as the
[runbook](pi5-experiment-runbook.md#re-check-the-ekuiper-comparator-before-each-batch)
describes, with `--host jetson`.

The host profile lives in the `hosts` map of `eval/canonical-matrix.json`.
Pass it to the runner and the validator; results land under
`jetson-<batch-id>` directories and are checked against this profile only.
E-Perf-10 adds this host's bracket rates, so run the host's
[capacity scout](pi5-experiment-runbook.md#run-the-hosts-capacity-scout) first:
repeat `mise run run-campaign -- --host jetson --capacity-scout --batch-id <scout-id>`
until it prints `"action": "stop"`. Then:

```sh
python3 eval/scripts/validate-canonical.py host --host jetson --require-ekuiper
python3 eval/scripts/lib/canonical_runner.py --host jetson --batch-id <batch-id> \
  --scout-batch-id <scout-id> --dry-run
```

`mise run plan-campaign -- --host jetson --batch-id <batch-id> --scout-batch-id <scout-id>`
prints the same schedule, and
`mise run run-campaign -- --host jetson --batch-id <batch-id> --scout-batch-id <scout-id>`
runs or resumes it. A final batch without `--scout-batch-id` refuses to start E-Perf-10.
When it has finished, `mise run approve-batch -- --host jetson --batch-id <batch-id>`
on the analysis machine records it as the Jetson entry of `eval/final-batches.json`.
