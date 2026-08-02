# Canonical Pi/Jetson runs — preflight plan

**Goal:** Get from "macOS shakedowns clean" to "canonical measurements
booking-ready on Pi 4 + Jetson Orin Nano." Every RQ pass criterion is
met in shakedown; canonical runs produce the absolute numbers that land
in the thesis.

**Predecessor:** [`plans/eval-followups.md`](./eval-followups.md) closed
the verify-flagged gaps in the shakedown apparatus.

**Filed elsewhere:** **A17** (process-time hot-swap rollback) is out of
this plan's scope; RQ3 claim wording stays "init-time rollback only"
unless P6 is explicitly approved. **A19** (runtime-side memory sampler)
is Phase P1 below.

## Ordering rationale

- **P0 first:** everything else needs an aarch64 runtime binary.
- **P1 || P2:** memory sampler (P1) and canonical configs (P2) are
  independent and can run in parallel.
- **P3 (host setup)** unblocks any real Pi execution.
- **P4 (eKuiper arm64):** verify comparator triangle survives cross-arch.
- **P5 (gate):** preflight checklist + smallest-canonical run. Nothing
  else runs on Pi until P5 goes green.
- **P6 (A17, OPTIONAL):** decision doc only, not implementation.

## Constraints

- **No thesis-quality numbers on macOS.** Anything quotable in the
  thesis comes from `--host rpi4` or `--host jetson` produced by
  `eval/scripts/run-experiment.sh` on the real device.
- **`memory_stats` crate is the ONLY source of RSS** in the runtime
  path. Shell fallbacks live only in harness scripts.
- **CPU isolation is mandatory:** `isolcpus=2,3` + `taskset -c` for
  SUT + loadgen on separate cores.
- **Canonical scale is enforced:** 60s / 60k msgs / 30s warmup / 30
  runs. `run-experiment.sh` refuses smaller with `--host rpi4|jetson`.
- **No canonical runs from a dirty tree:** `metadata.json` must record
  a `git_sha` that matches a tagged commit on `main`.

## Non-goals

- Not implementing A17. Filed for post-thesis or a separate plan.
- Not re-shaking macOS. Shakedown dirs are informational baseline;
  canonical replaces them, never supplements.
- Not designing new experiments. RFC-008 §D2 defines the 25.
- Not booking Jetson time before Pi 4 is green. Pi is primary target.

## Danger zones

- **Cross-linker libc mismatch:** homebrew `aarch64-linux-gnu-gcc`
  links against wrong libc. Prefer `cross` (Docker) or an ARM VM;
  verify with `file` and `ldd` on the target.
- **RSS unit drift:** `smaps_rollup Private_Dirty` is kB; macOS `ps` is
  kB but includes shared libs. Silent unit drift produced the A19 gap.
  Any change here must include a unit-comparison test.
- **eKuiper QEMU trap:** Docker Buildx <0.10 falls back to QEMU without
  warning. Always inspect `docker inspect <image> | jq .Architecture`
  before quoting eKuiper numbers on Pi.
- **Warmup drift:** shakedown-scale warmup on canonical hardware
  poisons every percentile with cold-cache penalty. `run-experiment.sh`
  guard is mandatory (task R2).

---

## Phases

### P0 — aarch64 toolchain + cross-compile

- AC: `cargo build --release --target aarch64-unknown-linux-gnu -p wafer-runtime -p wafer-loadgen -p waferctl` produces working ELF binaries. Verify: `file target/aarch64-unknown-linux-gnu/release/wafer` reports `ELF 64-bit LSB … ARM aarch64`.
- AC: Reproducible via `just cross-build-pi`. Verify: recipe exists; two consecutive invocations produce identical SHA256.
- AC: `docs/status/canonical-readiness.md` "What needs to change to book Pi time" §1 ticked.

### P1 — Linux memory sampler + per_node_metrics (A19)

- AC: `memory.csv` produced from a runtime-side sampler using the `memory_stats` crate on macOS + Linux, OR a documented shell-fallback if option (a) is chosen. Verify: fresh shakedown on both hosts produces `memory.csv` with sane values.
- AC: `per_node_metrics.csv` emitted by `PipelineOrchestrator` at shutdown, RESULT-CONTRACT-conformant. Verify: fresh shakedown produces file with one row per node.
- AC: A19 in `docs/status/implementation-gaps.md` closed with SHA.

### P2 — Canonical run configurations

- AC: `eval/configs/canonical/` directory exists with one config per canonical experiment, all at 60s / 60k / 30s / 30 runs. Verify: `grep -c 'total_messages = 60000' eval/configs/canonical/*.toml` equals canonical-experiment count.
- AC: `run-experiment.sh` refuses `--host rpi4|jetson` with sub-canonical warmup/messages/runs. Verify: negative test in `eval/scripts/tests/test_canonical_guard.sh`.
- AC: `verify-result-contract.py --canonical` mode accepts canonical dirs, rejects shakedown-scale.

### P3 — Pi/Jetson host preflight

- AC: `docs/eval/pi-host-setup.md` walks a fresh Pi from OS install → canonical-ready in <2 h. Verify: reader can follow doc end-to-end.
- AC: Canonical scripts use `taskset -c 2|3` for SUT|loadgen on rpi4|jetson. Verify: `grep -l 'taskset' eval/scripts/run-e-*-canonical.sh` covers every canonical experiment.
- AC: `just deploy-pi` + `just smoke-pi` recipes work. Verify: recipes exist; smoke against a Pi produces a RESULT-CONTRACT-conformant result dir.

### P4 — eKuiper on ARM64

- AC: `lfedge/ekuiper:2.1.0-alpine` pulls native `linux/arm64` on Pi. Verify: `docker inspect … | jq .Architecture` returns `arm64`.
- AC: Pi shakedown of E-Perf-1 produces RESULT-CONTRACT-conformant dirs for WAFER + native + eKuiper. Verify: `verify-result-contract.py eval/results/e-perf-1/rpi4-<ts>/*` returns 0.
- AC: `docs/benchmarks/ekuiper-comparator.md` documents Pi-specific setup.

### P5 — Preflight gate + first canonical Pi run

- AC: `plans/canonical-runs/scripts/preflight-check.sh --host <ip>` runs 15+ checks over ssh; green before booking multi-experiment Pi time.
- AC: E-Val-1 canonical run (60s × 30 runs) on Pi 4 with gate-pass green. Verify: `eval/results/e-val-1/rpi4-<ts>/summary.json` exists + notebook 00 prints PASS.
- AC: `docs/status/canonical-readiness.md` rewritten past-tense.

### P6 — A17 decision (OPTIONAL)

- AC: `docs/decisions/a17-process-time-rollback.md` records (a) implement now, (b) implement post-thesis, or (c) accept "init-time only" claim. Signed by stakeholder.

---

## Tasks

### C1 — aarch64 cross-compile spike (P0)

Verify `cargo build --release --target aarch64-unknown-linux-gnu -p wafer-runtime` succeeds on this macOS host. Try three paths in order: (1) `rustup target add` + host toolchain, (2) `cross` (Docker), (3) homebrew `aarch64-linux-gnu-gcc`. Document what works.

**ACs:**
- AC: Working ELF binary produced. Verify: `file target/aarch64-unknown-linux-gnu/release/wafer` reports `ELF 64-bit LSB … ARM aarch64`.
- AC: `docs/eval/cross-compile.md` documents the working path with exact commands. Verify: fresh clone reproduces the binary.
- AC: `just cross-build-pi` recipe in top-level Justfile. Verify: recipe runs and produces the binary.

**References:** skills `cargo-expert`, `rust-best-practices`; files `crates/wafer-runtime/Cargo.toml`, `Cargo.toml`, `Justfile`; docs [cross-rs](https://github.com/cross-rs/cross).

**Constraints:** prefer `cross` over homebrew linker (reproducibility); don't hide which toolchain worked; if openssl/native-tls blocks, evaluate `rustls` at workspace level as a spike outcome.

**Non-goals:** don't cross-compile plugins (already wasm32-wasip2 portable); don't set up a Pi image; don't run the binary on Pi.

**Executor:** `forked reviewer` (discovery-heavy, iteration expected).

### C2 — Cross-build wafer-loadgen + waferctl (P0)

Extend C1's recipe to `wafer-loadgen` and `waferctl`.

**ACs:**
- AC: `just cross-build-pi` builds all three binaries. Verify: `ls target/aarch64-unknown-linux-gnu/release/{wafer,wafer-loadgen,waferctl}` + `file` reports ARM aarch64.
- AC: No new deps beyond C1. Verify: `git diff Cargo.lock` empty post-C2.
- AC: Binary sizes recorded in `docs/eval/cross-compile.md`.

**References:** files `crates/wafer-loadgen/Cargo.toml`, `crates/waferctl/Cargo.toml`.

**Constraints:** same toolchain as C1; don't diverge.

**Executor:** `inline`. **Depends on:** C1.

### C3 — Plugin portability check on aarch64 (P0)

Verify plugins load into an aarch64 runtime via wasmtime AOT compile.

**ACs:**
- AC: All 5 canonical plugins validate on aarch64 runtime. Verify: `docker run --platform linux/arm64 ... wafer --config eval/configs/pipeline-shakedown.toml --validate` succeeds.
- AC: Plugin `.wasm` SHA256 identical across host arches. Verify: `sha256sum plugins/*/target/wasm32-wasip2/release/*.wasm` cross-host produces zero diff.

**References:** files `plugins/*/Cargo.toml`, `crates/wafer-core/src/engine/loader.rs`.

**Constraints:** validate-only via Docker aarch64 emulation is fine; real Pi not needed yet. If AOT compile fails on aarch64 escalate before proceeding.

**Executor:** `inline`. **Depends on:** C1, C2.

### M1 — Memory sampler decision doc (P1)

Choose between (a) shell-swap `ps` → `/proc/pid/smaps_rollup` in harness scripts, or (b) full A19 runtime-side `memory_stats` crate + `MemoryRecorder::sample_loop` wiring.

**ACs:**
- AC: `docs/decisions/canonical-memory-sampler.md` records the choice, rationale, cost, migration path. If option (a), files option (b) as follow-up with owner + estimated cost.

**References:** files `crates/wafer-core/src/bench/memory.rs`, `crates/wafer-runtime/src/main.rs`, `eval/scripts/run-experiment.sh`, `docs/status/implementation-gaps.md#A19`; docs [memory-stats](https://docs.rs/memory-stats/), proc(5) `smaps_rollup`; memory `wafer-testing` lessons.

**Constraints:** RESULT-CONTRACT `memory.csv` schema stays exactly as documented. 1 Hz sampling frequency, do not tune without a benchmark. `memory_stats` crate must be the ONLY source of RSS in the runtime path.

**Executor:** `inline`.

### M2a — Linux memory sampler shell swap (P1, only if M1 = option a)

Replace `ps -o rss=` with a Linux-aware helper.

**ACs:**
- AC: `eval/scripts/lib/sample_memory.sh` (new) has `sample_memory <pid>` emitting `timestamp_ns,rss_bytes,vsz_bytes`. Verify: standalone invocation on a running process produces sane rows.
- AC: `run-experiment.sh` + every shakedown script that emits `memory.csv` sources the helper. Verify: `grep -l 'sample_memory' eval/scripts/*.sh` covers all producers.
- AC: 100MB `malloc` synthetic process RSS reported within 5% on Linux. Verify: `bash eval/scripts/tests/test_sample_memory.sh` returns 0.

**References:** files `eval/scripts/run-experiment.sh`, `eval/scripts/run-e-*-shakedown.sh`.

**Constraints:** macOS branch stays `ps`; do not unify on `smaps_rollup` (not available on macOS). Column order + units in `memory.csv` verbatim.

**Executor:** `inline`. **Depends on:** M1 = option a.

### M2b — Runtime-side memory_stats sampler (P1, only if M1 = option b)

Wire `MemoryRecorder::sample_loop` in the runtime; replace macOS `ps` shell in `read_rss_bytes` with `memory_stats` crate. Emit `memory.csv` from runtime.

**ACs:**
- AC: `crates/wafer-core/src/bench/memory.rs::read_rss_bytes` uses `memory_stats` on both macOS and Linux. Verify: `cargo test -p wafer-core memory_stats_source` passes.
- AC: `wafer-runtime` main spawns `MemoryRecorder::sample_loop` when `WAFER_BENCH_OUTPUT_DIR` is set. Verify: fresh shakedown produces `memory.csv` without harness-side polling.
- AC: `run-experiment.sh` no longer shells `ps -o rss=`. Verify: `grep -c "ps -o rss=" eval/scripts/*.sh` returns 0.
- AC: SIGTERM during sampling flushes cleanly. Verify: `cargo test -p wafer-core memory_sampler_shutdown_flushes`.

**References:** skills `async-tokio`, `observability`, `rust-best-practices`; files `crates/wafer-core/src/bench/memory.rs`, `crates/wafer-runtime/src/main.rs`; memory A19 remediation notes.

**Constraints:** 1 Hz frequency; cancel-safe shutdown; buffered writes.

**Executor:** `forked reviewer` (cross-crate + async concerns). **Depends on:** M1 = option b.

### M3 — per_node_metrics.csv emitter (P1)

Emit `per_node_metrics.csv` directly from `PipelineOrchestrator` at shutdown.

**ACs:**
- AC: Graceful shutdown writes `per_node_metrics.csv` to `WAFER_BENCH_OUTPUT_DIR` with schema `node_id,messages_in,messages_out,traps_total,error_state_seconds,recovery_count`. Verify: fresh shakedown produces file with one row per node in `pipeline-shakedown.toml`.
- AC: Scrape-based fallback in `run-experiment.sh` used only when the runtime file is absent. Verify: `--no-api` mode still produces the file.
- AC: `eval/RESULT-CONTRACT.md` matrix updated: `per_node_metrics.csv` is "core" for canonical runs.

**References:** skills `observability`, `async-tokio`; files `crates/wafer-core/src/orchestrator/pipeline.rs`, `eval/RESULT-CONTRACT.md`.

**Constraints:** metric names unchanged (notebooks depend on them); emit only on graceful shutdown.

**Executor:** `inline`. **Depends on:** M1.

### M4 — Memory-sampling overhead benchmark (P1, only if M2b landed)

Criterion bench proving runtime-side sampler <0.1% CPU overhead.

**ACs:**
- AC: `cargo bench -p wafer-core --bench overhead_of_memory_sampling` shows throughput delta <0.1%. Verify: bench output ratio, criterion regression report absent.
- AC: Bench file pinned so CI catches regressions. Verify: `crates/wafer-core/benches/overhead_of_memory_sampling.rs` exists.

**References:** skills `rust-testing`, `observability`.

**Constraints:** passthrough pipeline (no real work); N=30 minimum for significance.

**Executor:** `forked reviewer`. **Depends on:** M2b.

### R1 — Split shakedown vs canonical configs (P2)

Add `eval/configs/canonical/` directory with 60s / 60k / 30s / 30-run configs. Keep `shakedown/` configs unchanged.

**ACs:**
- AC: `eval/configs/canonical/` has one config per canonical experiment. Verify: `ls eval/configs/canonical/*.toml` count matches experiment count.
- AC: Every canonical config declares `total_messages = 60000`, `warmup_secs = 30`. Verify: `grep -c 'total_messages = 60000' eval/configs/canonical/*.toml` equals file count.
- AC: Existing configs unchanged. Verify: `git diff eval/configs/pipeline-*.toml` empty.

**References:** files `eval/configs/*.toml`.

**Constraints:** add-only; don't rename existing. Path convention `eval/configs/{shakedown,canonical}/pipeline-<a|b|c>-<experiment>.toml`.

**Non-goals:** not updating notebooks; not deleting shakedown configs.

**Executor:** `inline`.

### R2 — run-experiment.sh canonical guard (P2)

Reject sub-canonical warmup/messages/runs when `--host rpi4|jetson`.

**ACs:**
- AC: `bash run-experiment.sh --host rpi4 --warmup-secs 1 …` exits non-zero with clear message. Verify: `eval/scripts/tests/test_canonical_guard.sh` asserts exit + error string.
- AC: Same guard for `--total-messages` (min 60000) and `--runs` (min 30). Verify: three assertions in the test.
- AC: Guard documented in `docs/eval/canonical-runs.md`.

**References:** files `eval/scripts/run-experiment.sh`.

**Constraints:** guard must NOT trigger for `--host shakedown-macos|shakedown-linux`. Error message specific: "canonical warmup requires ≥30s (got 1s); use --host shakedown-macos for informational runs."

**Executor:** `inline`.

### R3 — Canonical-mode RESULT-CONTRACT verifier (P2)

Extend `verify-result-contract.py` (from F3.AC5) with `--canonical` mode that tightens the optional-artefact matrix to must-have.

**ACs:**
- AC: `verify-result-contract.py --canonical <dir>` returns 0 for valid canonical dir, 1 for missing optional artefact. Verify: positive + negative test in `eval/scripts/tests/test_verify_result_contract_canonical.py`.
- AC: `metadata.json` provenance keys must be non-empty in canonical mode. Verify: extends write_metadata merge test.
- AC: Shakedown mode behavior unchanged. Verify: existing dirs still pass without `--canonical`.

**References:** files `eval/scripts/verify-result-contract.py`, `eval/scripts/lib/write_metadata.py`.

**Executor:** `inline`. **Depends on:** M3.

### H1 — Pi host setup documentation (P3)

Step-by-step doc taking a fresh Pi 4 from OS install to canonical-ready in <2 h.

**ACs:**
- AC: `docs/eval/pi-host-setup.md` covers OS install, kernel cmdline `isolcpus=2,3`, `cpupower` governor `performance`, `drop_caches` capability, mosquitto + docker install, ssh keys, hostname, network topology, NTP.
- AC: Verification section at end: 5 commands that must return green before booking Pi time. Verify: `preflight-check.sh` runs those over ssh.
- AC: Jetson deltas mentioned (JetPack, nvpmodel) — pointers only.

**References:** files `docs/status/canonical-readiness.md`; docs Raspberry Pi + Jetson.

**Constraints:** written for someone who has never set up a Pi; every command copy-paste runnable.

**Executor:** `inline`.

### H2 — taskset wrapper in canonical scripts (P3)

Every `eval/scripts/run-e-*-canonical.sh` script pins SUT to core 2 + loadgen to core 3 via `taskset` when host is `rpi4|jetson`.

**ACs:**
- AC: `eval/scripts/lib/taskset_wrap.sh` centralises wrap logic. Verify: no per-script inline host checks.
- AC: Every canonical script uses the helper. Verify: `grep -l 'taskset_wrap\|taskset -c' eval/scripts/run-e-*-canonical.sh` covers all canonical experiments.
- AC: macOS branch is a no-op (no `taskset` binary). Verify: script exits cleanly on macOS.

**References:** files existing `run-e-*-shakedown.sh`.

**Constraints:** don't touch shakedown scripts; use `command -v taskset` for detection.

**Executor:** `inline`. **Depends on:** R1.

### H3 — Pi deployment recipes (P3)

`just deploy-pi --host <ip>` and `just smoke-pi --host <ip>`.

**ACs:**
- AC: `just deploy-pi --host <ip>` completes in <60s over LAN, produces `/opt/wafer/{bin,plugins,configs}/`. Verify: run + ssh + list.
- AC: `just smoke-pi --host <ip>` runs pipeline-shakedown.toml once, rsyncs result to `eval/results/pi-smoke/`. Verify: `verify-result-contract.py` on returned dir passes.
- AC: Recipes linked from `docs/eval/pi-host-setup.md`.

**References:** files `Justfile`.

**Constraints:** rsync + ssh only, no new deps; no systemd units.

**Executor:** `inline`. **Depends on:** C2, H1.

### E1 — eKuiper arm64 verification (P4)

Confirm `lfedge/ekuiper:2.1.0-alpine` runs natively on aarch64.

**ACs:**
- AC: `docker manifest inspect lfedge/ekuiper:2.1.0-alpine | jq '.manifests[] | select(.platform.architecture=="arm64").platform.os'` returns `linux`.
- AC: On Pi: `docker run --rm lfedge/ekuiper:2.1.0-alpine uname -m` returns `aarch64`. Verify: shipped in `preflight-check.sh`.
- AC: `docs/benchmarks/ekuiper-comparator.md` "Pi 4" section notes arm64 confirmation + Docker version prereq.

**References:** files `eval/ekuiper/docker-compose.yml`, `docs/benchmarks/ekuiper-comparator.md`.

**Constraints:** Docker Buildx ≥0.10 required (multi-arch pull).

**Executor:** `inline`.

### E2 — Pi shakedown of E-Perf-1 comparator triangle (P4)

Reproduce E-Perf-1 at shakedown-scale on Pi with WAFER + native + eKuiper.

**ACs:**
- AC: `eval/results/e-perf-1/rpi4-<ts>/{wafer,native,ekuiper}/` all RESULT-CONTRACT-conformant. Verify: `verify-result-contract.py` on each subdir passes.
- AC: All three systems achieve ≥95% of 1000 msg/s source rate. Verify: throughput.csv medians.
- AC: WAFER/native ratio on Pi within 30% of macOS shakedown ratio (0.999). Verify: comparison table in result dir README; >30% drift means investigation before canonical.

**References:** files `eval/configs/pipeline-a-{wafer,native}.toml`, `eval/ekuiper/`, `eval/scripts/run-e-perf-1-2-shakedown.sh`.

**Constraints:** shakedown-scale at this stage; do not jump to canonical until this passes.

**Executor:** `user` (requires physical Pi + network). **Depends on:** C2, H3, E1.

### F1 — Preflight checklist script (P5)

`plans/canonical-runs/scripts/preflight-check.sh --host <ip>` runs 15+ checks over ssh.

**ACs:**
- AC: Script runs kernel cmdline / governor / binary version / eKuiper arch / taskset / mosquitto / smoke-result checks. Verify: green on fully-prepared Pi, non-zero + failure list on partial.
- AC: Each check has an actionable error linking `pi-host-setup.md#section`. Verify: negative test shows "SEE docs/eval/pi-host-setup.md#X".

**References:** files `docs/eval/pi-host-setup.md`, `eval/scripts/verify-result-contract.py`.

**Constraints:** read-only (no state changes on target); ssh key auth only.

**Executor:** `inline`. **Depends on:** H1, H3, E1.

### F2 — First canonical-scale Pi run (P5)

E-Val-1 canonical run (60s × 30 runs) on Pi 4.

**ACs:**
- AC: `eval/results/e-val-1/rpi4-<ts>/summary.json` exists, gate-pass check green. Verify: notebook 00 prints PASS.
- AC: Total wall-clock <45 min. Verify: metadata.json duration_ns sum <45 min.
- AC: `docs/status/canonical-readiness.md` E-Val-1 row updated with the Pi timestamp + PASS.

**References:** files `eval/configs/canonical/pipeline-shakedown.toml`, `eval/analysis/notebooks/00-warmup-validation.ipynb`, `docs/status/canonical-readiness.md`.

**Constraints:** if gate-pass fails, STOP. Do not proceed to other canonical experiments until E-Val-1 canonical is clean. That is the point of the gate.

**Executor:** `user`. **Depends on:** F1 green.

### F3 — Canonical readiness rewrite (P5)

Update `docs/status/canonical-readiness.md` to reflect closed items.

**ACs:**
- AC: "What needs to change to book Pi time" section rewritten past-tense, referencing the closing commits/tasks.
- AC: Experiment matrix updated with `rpi4-<ts>` shakedown evidence for at least E-Val-1 + E-Perf-1.

**References:** files `docs/status/canonical-readiness.md`.

**Executor:** `inline`. **Depends on:** F2.

### A17 — Process-time rollback decision (P6, OPTIONAL)

**ACs:**
- AC: `docs/decisions/a17-process-time-rollback.md` records: (a) implement now, (b) post-thesis, or (c) accept "init-time only" claim. Stakeholder signature line.

**References:** files `docs/status/implementation-gaps.md#A17`, `docs/benchmarks/rq-summary.md`; existing `eval/results/e-swap-5/`.

**Constraints:** decision only, not implementation. If (a) is chosen, spawn a separate plan.

**Executor:** `user`.

---

## Estimated cost

| Phase | Est. hours | Blocked on |
|---|---|---|
| P0 | 4-8 | (nothing) |
| P1 | 3-6 (option a) or 8-14 (option b) | M1 decision |
| P2 | 2-4 | (nothing, parallel with P1) |
| P3 | 4-6 | C1-C3 |
| P4 | 2-4 | H3 |
| P5 | 3-5 | P0-P4 all green |
| P6 (opt) | 0.5-1 (decision) or 8-16 (implementation) | out of scope |
| **Total** | **18-33 h** without A17 implementation | |

Realistic across-sessions estimate: **3-5 sessions** at 3-6 productive
hours each, plus one Pi-bring-up session that lives outside code time.

## Handoff

- Preflight failures caught by `preflight-check.sh` are the primary
  escalation surface. Route the specific error message + failing check
  to the next session.
- Canonical results land in `eval/results/<experiment>/rpi4-<ts>/`;
  every dir must pass `verify-result-contract.py --canonical` before
  the notebook consumes it.
- Session-scratch drafts under `{scratchDir}` — cross-compile logs,
  Pi provisioning notes, preflight failure reports.
