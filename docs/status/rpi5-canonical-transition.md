# Raspberry Pi 5 canonical-transition record

**Status:** accepted before Raspberry Pi 5 measurements

This record freezes the hardware and deployment choices for the canonical WAFER evaluation. It supersedes current-project statements that name Raspberry Pi 4 as the primary target or require Docker for the eKuiper comparator. It does not rewrite historical plans, prior measurements, or literature facts about experiments performed on Raspberry Pi 4.

## Current and desired state

| Concern | Previous assumption | Canonical decision |
|---|---|---|
| Primary edge gateway | Raspberry Pi 4, 4 GB | Raspberry Pi 5, 4 GB |
| Canonical host tag | `rpi4` | `rpi5` |
| Comparator deployment | eKuiper 2.1.0 in Docker Compose | Native eKuiper 2.1.0 ARM64 package |
| MQTT broker | Container or localhost | Native Mosquitto on the Pi |
| CPU allocation | Conflicting documents: either one SUT core or cores 1–3 | CPU 0 for OS, Mosquitto, and load generation; CPUs 1–3 for the active SUT |
| Synthetic input | `wafer-loadgen` | `wafer-loadgen` |
| ESP32 | Considered as an input source | Excluded from measured experiments; no additional demonstration is planned |

## Hardware boundary

The primary device is a Raspberry Pi 5 with 4 GB RAM, a quad-core Cortex-A76 CPU with a stock 2.4 GHz maximum frequency, active cooling, and wired Ethernet. Canonical metadata records the exact OS image, kernel, firmware, storage, power supply, governor, CPU affinity, temperature, and throttling state.

The 4 GB memory constraint preserves WAFER's gateway-class scope, but Raspberry Pi 5 results are not numerically interchangeable with Raspberry Pi 4 results in prior literature. The evaluation therefore reports both absolute measurements and dimensionless WAFER/native/eKuiper ratios. This hardware change reduces direct comparability with Raspberry Pi 4 studies and is reported as an external-validity limitation.

## Comparator boundary

WAFER, the native Rust baseline, and eKuiper run directly on the same Raspberry Pi OS installation. eKuiper is pinned to version 2.1.0 and installed from its official Linux ARM64 release package. All three systems use the same native Mosquitto broker, payloads, topics, load profile, measurement subscriber, CPU allocation, warmup, run length, and repetition count.

Docker is excluded from the Pi because it would add a deployment and network boundary to only one system. Existing macOS Docker shakedowns remain development evidence and are not canonical results.

## CPU allocation

The canonical allocation is:

- CPU 0: operating-system work, Mosquitto, and `wafer-loadgen` publisher/subscriber.
- CPUs 1–3: exactly one active SUT—WAFER, the native Rust baseline, or eKuiper.

The Pi boots with `isolcpus=1-3`. Each SUT is launched with `taskset -c 1-3`; Mosquitto and load generation use CPU 0. Systems run sequentially, never concurrently. This preserves the multi-threaded execution model while giving every comparator the same compute budget.

## Measurement scale

Smoke and shakedown runs validate the device and harness but are not thesis evidence. Canonical claims require the predeclared experiment scale: at least 30 repetitions, 30 seconds of excluded warmup, the experiment-specific measurement window, open-loop generation, and result-contract verification.

Existing quantitative pass criteria remain frozen as engineering budgets unless a separate pre-measurement methodology amendment changes them. They must not be relaxed after observing Raspberry Pi 5 results.

## 2026-08-29 hardware shakedown

The first real-device shakedown completed on a Raspberry Pi 5 Model B Rev 1.0 with 4,146,304 KiB visible RAM, Debian 13, kernel `6.18.39+rpt-rpi-2712`, the performance governor, and CPUs 1–3 isolated. Preflight passed 19 checks with no failures. Native eKuiper 2.1.0 ran as the `kuiper` user on CPUs 1–3 after correcting two defects in its official Debian systemd unit: a lowercase `user` key and relative data-path startup.

Pipeline C processed all 3,000 messages with zero gaps, duplicates, traps, or recoveries. E-Val-1 processed all 300 messages with zero gaps or duplicates and measured the injected 50 ms delay at 51.184 ms p99 across 249 post-warmup samples. Both runs recorded `throttled=0x0`, temperatures below 60 °C, matching runtime binary hashes, and complete result-contract artifacts.

These runs validate the host, deployment, instrumentation, and methodology only. Their metadata records `git_dirty=true`; they are not canonical thesis evidence and are not used for RQ conclusions. The first attempt is preserved under `invalid-rpi5-*` because it exposed and led to fixes for deployed source-state provenance and warmup sequence accounting.

## Scope of follow-up edits

Current architecture, requirements, evaluation, runbook, analysis, and thesis-methodology documents adopt these decisions. Archived plans, historical reports, and descriptions of external Raspberry Pi 4 studies retain their original wording.
