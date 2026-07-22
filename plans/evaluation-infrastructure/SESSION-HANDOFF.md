# Session handoff — evaluation-infrastructure plan

**Session end state.** Everything I could execute autonomously on macOS
has landed. The remaining tasks require you to install eKuiper (P1.4
`executor: user`) — after which four more tasks become executable and
the plan can close.

## Progress

**36 / 42 tasks done (86%).**

Fully green:
- **P0.\*** all 14 runtime prerequisites (P0.1 – P0.14).
- **P1.1** result-directory contract + orchestration script.
- **P2.1** E-Perf-4 per-hop overhead × payload size.
- **P2.2** E-Perf-6 per-node RSS scaling (depths 1/3/5/10).
- **P2.3** E-Perf-7 metering overhead decomposition (4 ablations).
- **P2.4** E-Perf-8 pipeline depth scaling.
- **P3.1** E-Val-1 methodology validation (honesty gate PASS).
- **P3.4** E-Perf-3 per-hop overhead vs depth with MQTT bookends.
- **P3.6** E-Backpressure + E-Perf-9 AOT cold/warm.
- **P4.1–P4.6** E-Iso-1..6 attack containment (six attacks, all contained).
- **P4.7** E-Iso-7 diamond fault isolation (branch-A −0.04% drop).
- **P4.8** E-Iso-8 recovery time (338 histogram samples).
- **P5.1** E-Swap-6 hot-swap phase decomposition.
- **P5.2** E-Swap-1 pause duration (p95 = 1.33 ms, well under 100 ms target).
- **P5.3** E-Swap-2 zero-loss + zero-dup verification.
- **P5.5** E-Swap-4 swap under 2× burst.
- **P5.6** E-Swap-5 failed swap (partial — see A17).
- **P5.7** E-Density-1 binary sizes.

## Runtime bugs closed this session

1. **A16** (P0.14, commit `40ab46b`) — WASI async host calls panicking
   inside Tokio runner tasks. Wrapped sync guest calls in
   `tokio::task::block_in_place` at three sites. Regression test:
   `crates/wafer-core/tests/wasi_async_runner.rs`.
2. **DLQ retry_count reset** (commit landing at P0 verify follow-up) —
   `SerializableEnvelope` now carries `retry_count`; re-injected poison
   messages cannot regain a fresh retry budget.
3. **Native transform 400** — `/hot-swap` and `/reconfigure` handlers
   return `BAD_REQUEST` for native transforms instead of leaking a 500
   from the swap machinery.
4. **Epoch deadline wraparound** (commit `56a2ccf`, pre-session) —
   `Store::set_epoch_deadline` is relative; must reset per call.
5. **Buffer resource leak** (same commit) — `borrow<buffer>` in WIT
   requires explicit `delete_buffer` after each guest call.
6. **Metadata drop** (commit `b08fd3c`, pre-session) —
   `WasmTransformNode::process` was dropping `output.metadata`.

## Open gaps filed but not fixed

1. **A17** — Process-time hot-swap rollback not implemented. Blocks
   E-Swap-5 from full PASS; downgrades RQ3 claim from "rollback on any
   failure" to "rollback on init failure only". Stakeholder decision
   required (canary window vs. accept current behaviour).

## What P1.4 needs

**Task**: `eval/ekuiper/docker-compose.yml`, `pipeline-a-rule.sql`,
`README.md`, `docs/benchmarks/ekuiper-comparator.md`.

**Manual steps for you** (macOS):

```sh
cd <repo>

# 1. Create the compose file directory + files
mkdir -p eval/ekuiper
```

Then follow RFC-008 §D6 for the eKuiper compose spec. Pin the LTS tag
explicitly: `lfedge/ekuiper:2.1.0-alpine` (or the latest 2.1.x LTS at
the time of running). Wire mosquitto and eKuiper in the same compose
so the harness can start both with one command. The SQL rule needs to
mirror Pipeline A: MQTT input → JSON parse → `temperature > 50` filter
→ routed MQTT output.

**Verify P1.4:**

```sh
docker compose -f eval/ekuiper/docker-compose.yml up -d
curl -s :9081/streams | jq .           # 200 + Pipeline-A stream
# Drive with loadgen; capture eKuiper output. Details in RFC-008 §D6.
```

Once P1.4 is green, complete it with `plan_tasks complete taskId=P1.4`,
and the following unblock automatically:

- **P3.2** — E-Perf-1 throughput comparison (WAFER vs native vs eKuiper).
- **P3.3** — E-Perf-2 latency comparison (WAFER vs native vs eKuiper).
- **P5.4** — E-Swap-3 throughput dip vs full-restart (WAFER vs eKuiper).

Each of those has the same fresh reviewer delegation shape as the
E-Perf-3/E-Backpressure/E-Perf-9 batch just landed — reuse
`eval/scripts/run-experiment.sh` for driving both systems and a
wrapper script like `eval/scripts/run-e-perf-1-shakedown.sh`.

After those three complete, **P7.1** (fill analysis notebooks 00–10
with the statistical pipeline) and **P7.2** (canonical-readiness
matrix per-experiment gap list) unblock. Both are `executor: any`
with clear scope; both are fresh reviewer candidates.

## Artefacts landed this session

- **9 experiment result directories** under `eval/results/` (all gitignored per
  contract): e-val-1, e-iso-{1..8}, e-swap-{1,2,4,5,6}, e-perf-{3,6,7,8,9},
  e-backpressure.
- **~20 pipeline configs** under `eval/configs/e-*/`.
- **~10 shakedown scripts** under `eval/scripts/run-e-*.sh`.
- **9 analysis notebooks** under `eval/analysis/notebooks/` — 02, 03, 04, 04b,
  05, 06, 08, 09, 10. Notebook naming reconciliation to match plan's
  `{00, 01, 04, 07, 09, 10}` scheme is deferred to P7.1.
- **2 canonical config sentinel fixes** — `pipeline-c-{fuel-only,
  epoch-only,neither}.toml` no longer use the buggy `0 = unlimited`
  placeholder.
- **Full readiness matrix rows** for every completed experiment in
  `docs/status/canonical-readiness.md`.

## Commit trail

```
d3e1a05 fix(eval): canonical metering-ablation configs use large sentinels
b00f215 eval: E-Perf-6/7/8 shakedown (P2.2/P2.3/P2.4)
bdec8cc docs(gaps): A17 — process-time hot-swap rollback not implemented
4714119 eval: track uv.lock for Python analysis environment reproducibility
819f47a eval: E-Swap-1/2/4/5/6 hot-swap shakedown infrastructure + macOS results
ef65314 eval: E-Iso-7 (diamond fault isolation) & E-Iso-8 (recovery time)
1d0a6b2 feat(eval): E-Iso-1..6 attack containment shakedown on macOS
d5560f2 docs: E-Val-1 methodology validation passes on macOS (P3.1)
40ab46b fix(runtime): wrap sync guest calls in block_in_place (A16, P0.14)
```
(Plus the E-Perf-3/E-Backpressure/E-Perf-9 commit landing here.)

## Test surface

- `cargo test -p wafer-core --lib` — **310 passing, 0 failed, 1 ignored**.
- `cargo test -p wafer-core --tests` — 326 total (310 lib + 16 integration).
- `cargo test -p wafer-config --test eval_configs_load` — 1 passing (every
  new config parses and validates).

## When to compact

Now is a good compaction point. The ORCHESTRATOR-PLAYBOOK, memory facts,
plan state, and this handoff document capture everything needed for the
next session. Post-compact, resume with:

1. Read `plans/evaluation-infrastructure/execution-playbook.md`.
2. Read `plans/evaluation-infrastructure/SESSION-HANDOFF.md` (this file).
3. Ask user: has P1.4 (eKuiper) been installed? If yes, complete it and
   proceed to P3.2 / P3.3 / P5.4 batch. If not, stop and wait.
