# Run WAFER experiments on Raspberry Pi 5

This runbook identifies the pipeline behind each research question, shows the commands that are runnable today, and explains how result data returns to the analysis workstation. Complete [`pi5-host-setup.md`](pi5-host-setup.md) first.

## Execution levels

| Level | Purpose | Evidence status |
|---|---|---|
| Smoke | Prove deployment, plugin loading, CPU affinity, shutdown, and result writing | Diagnostic only |
| Shakedown | Exercise an experiment at reduced repetitions or duration | Informational only |
| Canonical | Produce thesis evidence with the frozen method | Quotable after contract and analysis checks |

A Pi 5 directory name alone does not make a run canonical. Canonical runs require at least 30 repetitions, 30 seconds of excluded warmup, the experiment-specific measurement window, a clean tagged source revision, no throttling, and successful result-contract verification.

## Tests available immediately

Run these on the Pi after deployment:

```sh
cd ~/wafer
./eval/scripts/preflight-pi5.sh
./eval/scripts/run-rpi5-smoke.sh
./eval/scripts/run-rpi5-validation.sh
./eval/ekuiper/seed-pipeline-a.sh
./eval/ekuiper/smoke-test.sh
```

### Pipeline C smoke

`run-rpi5-smoke.sh` runs:

```text
BenchSource → pass-through Wasm Transform → BenchSink
```

Configuration: `eval/configs/pipeline-c-rpi5-smoke.toml`.

It emits 3,000 messages, excludes the first 1,000, runs WAFER on CPUs 1–3, and writes `eval/results/e-smoke/rpi5-<timestamp>/`. This proves the deployed binary and Component Model plugin execute on the Pi; it is not a performance result.

### Methodology validation

`run-rpi5-validation.sh` runs:

```text
BenchSource at 10 msg/s → 50 ms delay Wasm Transform → BenchSink
```

Configuration: `eval/configs/pipeline-c-with-delay.toml`.

The run passes only when the histogram is non-empty, p99 is within 45–55 ms, the result directory conforms to the contract, and the Pi reports no throttling. One short run is a harness check; canonical E-Val-1 still requires the predefined repeated-run procedure.

### Native eKuiper smoke

`eval/ekuiper/smoke-test.sh` publishes two records through Pipeline A. Temperature 30 must be dropped and temperature 80 must be forwarded. This proves native eKuiper, native Mosquitto, and the comparator rule work before performance runs.

## Pipeline and experiment map

| Group | Experiments | Pipeline/config | Current runner | Primary data |
|---|---|---|---|---|
| Method validation | E-Val-1 | `pipeline-c-with-delay.toml` | `run-rpi5-validation.sh` for one short run; `run-e-val-1-shakedown.sh` is still macOS-specific for repeated runs | `latency.hdr`, `throughput.csv`, `percentiles.json` |
| RQ1 comparator | E-Perf-1, E-Perf-2 | `pipeline-a-wafer.toml`, `pipeline-a-native.toml`, native eKuiper Pipeline A | `run-e-perf-1-2-shakedown.sh` requires Pi 5/native-eKuiper adaptation before canonical use | latency, throughput, sequence, memory |
| RQ1 MQTT depth | E-Perf-3 | `e-perf-3/pipeline-mqtt-depth-{1,3,5,10}.toml` | `run-e-perf-3-shakedown.sh`; canonical wrapper pending | latency by depth |
| RQ1 boundary cost | E-Perf-4 | `e-perf-4/pipeline-c-passthrough-{120b,1kb,10kb,100kb}.toml` | `run-e-perf-4-shakedown.sh`; canonical wrapper pending | per-size latency histograms |
| RQ1 cross-architecture | E-Perf-5 | matching Pipeline C builds on Pi 5 and x86 Linux | canonical paired wrapper pending | WAFER/native ratios |
| RQ1 memory/depth | E-Perf-6, E-Perf-8 | `e-perf-6/pipeline-depth-{1,3,5,10}.toml` | `run-e-perf-6-8-shakedown.sh`; canonical wrapper pending | `memory.csv`, latency by depth |
| RQ1 metering | E-Perf-7 | `e-perf-7/pipeline-c-{passthrough,fuel-only,epoch-only,neither}.toml` | `run-e-perf-7-shakedown.sh`; canonical wrapper pending | latency by metering mode |
| RQ1 startup | E-Perf-9 | `e-perf-9/pipeline-tier-{small,medium,large}.toml` | `run-e-bp-perf9-shakedown.sh`; canonical wrapper pending | cold/warm startup |
| RQ1 pressure | E-Backpressure | `e-backpressure/pipeline-burst.toml` | `run-e-bp-perf9-shakedown.sh`; canonical wrapper pending | throughput, queue behavior, sequence |
| RQ2 containment | E-Iso-1..6 | `e-iso-{1..6}/pipeline.toml` | `run-e-iso-shakedown.sh`; canonical wrapper pending | trap and node-state evidence |
| RQ2 branch/recovery | E-Iso-7, E-Iso-8 | `e-iso-7/*.toml`, `e-iso-8/pipeline.toml` | `run-e-iso-7-8.sh`; canonical wrapper pending | healthy-branch throughput, recovery latency |
| RQ3 hot-swap | E-Swap-1,2,4,5,6 | `e-swap/pipeline-hotswap*.toml` | `run-e-swap-shakedown.sh`; canonical wrapper pending | swap timeline, sequence, throughput |
| RQ3 comparator | E-Swap-3 | `e-swap/pipeline-swap3-mqtt.toml`, native eKuiper Pipeline A | `run-e-swap-3-shakedown.sh` requires native-eKuiper adaptation | throughput dip by strategy |
| Static density | E-Density-1 | built plugin artefacts | `collect-binary-sizes.sh` | `binary-sizes.csv` |

The shakedown scripts named above are useful implementation references, but scripts that embed `shakedown-macos`, Docker, or reduced duration must not produce canonical Pi 5 claims until dedicated wrappers replace those assumptions.

## Canonical execution contract

Every canonical wrapper must enforce:

- host tag `rpi5`;
- N ≥ 30 independent runs per condition;
- at least 30 seconds excluded warmup;
- experiment-defined duration and rate;
- WAFER/native/eKuiper running separately on CPUs 1–3;
- native Mosquitto and load generation on CPU 0;
- governor `performance` and `vcgencmd get_throttled=0x0` before and after;
- a clean, tagged source revision;
- `verify-result-contract.py` success for every run directory.

Do not expand a short smoke command into an overnight loop and call it canonical. Implement and review each canonical wrapper against its experiment definition first.

## Retrieve data

From the development machine:

```sh
mkdir -p eval/results
rsync -av USER@wafer-pi5:wafer/eval/results/ eval/results/
```

The transfer is additive and preserves timestamped directories. Never delete a device result until its copy has been verified.

Verify one retrieved run:

```sh
python3 eval/scripts/verify-result-contract.py \
  eval/results/e-smoke/rpi5-YYYY-MM-DDTHH-MM-SSZ
```

Inspect provenance:

```sh
jq . eval/results/e-smoke/rpi5-*/metadata.json
sha256sum eval/results/e-smoke/rpi5-*/config.toml
```

## Analyze data

Install the locked analysis environment once:

```sh
cd eval/analysis
uv sync
```

Select a Pi 5 result explicitly while the notebooks are being migrated from shakedown defaults:

```sh
SHAKEDOWN_DIR=../results/e-perf-4/rpi5-YYYY-MM-DDTHH-MM-SSZ \
  uv run jupyter execute notebooks/02-per-hop-overhead.ipynb
```

After all canonical inputs exist:

```sh
cd ../..
mise run //eval:notebooks-execute
mise run //eval:figures
```

Notebook output is not proof by itself. Confirm every input directory is `rpi5-*`, every metadata file identifies the same source revision and hardware state, and no smoke directory was selected.
