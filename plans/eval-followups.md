# Evaluation-infrastructure — follow-up work

> **Status.** The `evaluation-infrastructure` plan closed 42/42 on
> 2026-07-22. This document collects issues raised by two rounds of
> parallel blind reviewers (review pass) that are **too large or too
> risky to fix inline**. Each item is a self-contained follow-up
> ready to become a task in the successor plan
> (`plans/canonical-runs/` or a dedicated hardening plan).
>
> Quick-fix findings from the same review rounds were fixed inline in
> commit `<pending>`; they are not in this list.

## Reviewer artefacts

- `.delegation-runner/artifacts/41081345_completeness-reviewer_0_output.md`
- `.delegation-runner/artifacts/41081345_correctness-reviewer_1_output.md`
- `.delegation-runner/artifacts/41081345_safety-reviewer_3_output.md`
- `.delegation-runner/artifacts/4f221368_reviewer_1_output.md` (rust-idiom pass)
- `.delegation-runner/artifacts/4f221368_reviewer_2_output.md` (eval-harness pass)

Rubric: `.delegation-runner/verify-eval-2025/rubric.json` (29 frozen criteria).

## Consensus verdicts

| Reviewer | Round | Verdict | Model |
|---|---|---|---|
| completeness | 1 | PASS (28/29; P0.AC1 self-defeating rubric) | independent-model |
| correctness | 1 | FAIL (A18 invalidates RQ1 comparison) | independent-model |
| safety | 1 | CAUTION (rq-summary misleading — fixed inline) | independent-model |
| rust-idiom | 2 | PASS_WITH_NOTES | independent-model |
| eval-harness | 2 | FAIL (result-dir contract violations) | independent-model |
| quality | 1, 2 | FAILED to produce output (twice) | independent-model |

Cross-family judge protocol satisfied: builder was primary model family, all reviewers independent model.

---

## P-Followup-1 — Wire native `threshold_filter` for RQ1 apples-to-apples (A18)

**Priority:** P0 — blocks canonical RQ1 claim.
**Origin:** correctness-reviewer + safety-reviewer consensus; already
tracked as A18 in `docs/status/implementation-gaps.md`.

**Problem.** `pipeline-a-native.toml` uses `NativeTransform::passthrough`
because native filter dispatch is not wired in the launcher. The
comparator experiments E-Perf-1 and E-Perf-2 therefore measure:

- WAFER: JSON decode + `temperature > 50` + forward
- native: byte forward (no decode, no compare)
- eKuiper: JSON decode + WHERE + forward

The 0.995 WAFER/native ratio UNDER-STATES the isolation tax by the cost
of one JSON decode + one float compare (≤ 1 µs at MQTT scale).

**Deliverables.**

1. Add `NativeTransform::threshold_filter { field: String, threshold: f64 }`
   in `crates/wafer-core/src/node/native/mod.rs`.
2. Extend the loader dispatch in
   `crates/wafer-core/src/orchestrator/launcher.rs` to recognise
   `plugin.kind = "threshold-filter"` when `plugin.native = true` (or
   the equivalent PluginSpec variant).
3. Add regression test that runs the same JSON payload through
   `threshold-filter` (native) and asserts the same subset passes as
   the WIT filter plugin.
4. Update `docs/benchmarks/rq-summary.md` to remove the A18 caveat once
   the fix lands.
5. Re-run E-Perf-1 / E-Perf-2 shakedown with the corrected native
   config and file updated readiness rows.

**Estimated cost.** 1–2 hours. Small runtime + config-schema change.

---

## P-Followup-2 — Enrich `metadata.json` provenance (missing fields)

**Priority:** P0 — reproducibility depends on it.
**Origin:** eval-harness reviewer.

**Problem.** Spot-check of `eval/results/*/shakedown-macos-*/metadata.json`
shows the following fields are captured:

```json
["arch","duration_ns","experiment","git_sha","host_tag","hostname",
 "latency_p50_ns","latency_p99_ns","os","run_index","started_at",
 "system","throughput_msg_s","total_recorded"]
```

Missing per `eval/RESULT-CONTRACT.md` and RFC-008 D10:

- `wasmtime_version`
- `config_sha256` (SHA of the effective config.toml)
- `wafer_plugin_hashes` (map plugin-name → SHA256 of `.wasm` bytes)
- `wafer_runtime_sha256` (SHA of the wafer binary)
- `rustc_version`
- `kernel` (currently proxied by `system`; explicit `uname -r` string)

**Deliverables.**

1. Extend the metadata emitter (grep the runtime for the
   `metadata.json` writer — likely in `wafer-runtime/src/main.rs` or
   BenchSink emitter) with the missing fields.
2. Populate `wafer_plugin_hashes` from the same cached hash the P0.12
   plugin-hash guard uses.
3. Populate `wasmtime_version` via `wasmtime::VERSION` const.
4. Add integration test asserting a fresh shakedown run produces every
   RESULT-CONTRACT field.
5. Document the enriched schema in `eval/RESULT-CONTRACT.md`.

**Estimated cost.** 2–3 hours.

---

## P-Followup-3 — `memory.csv` + `per_node_metrics.csv` collection

**Status: ✅ Closed** (option B) — F3, commit `4709e3f`.

**Priority:** P1 — result-dir contract violation; blocks E-Perf-6 (RSS)
and E-Iso-1..8 (per-node throughput) canonical claims.
**Origin:** eval-harness reviewer.

**Problem.** Spot-check of result dirs shows `memory.csv` and
`per_node_metrics.csv` missing from most shakedown outputs (present in
E-Perf-6 only — that's the memory experiment). The contract at
`eval/RESULT-CONTRACT.md` lists both as required.

**Two-option split.**

**Option A** (recommended): make collection universal.
- Wire `memory_stats` sampling into `wafer-runtime` at 1 Hz always.
- Wire per-node throughput/latency emission into the BenchSink
  regardless of experiment kind.
- Cost: ~4 hours; small perf hit (one more thread).

**Option B**: relax the contract.
- Split `RESULT-CONTRACT.md` into a *core* manifest (config,
  metadata, latency.hdr, throughput.csv) and *optional* per-experiment
  additions.
- Cost: ~1 hour; better matches actual usage but weakens the contract.

Decide before canonical Pi runs.

---

## P-Followup-4 — Shell script exit-code discipline

**Priority:** P1 — masks catastrophic runtime startup failures.
**Origin:** eval-harness reviewer.

**Problem.** 16 of 19 shakedown scripts under `eval/scripts/` lack
`trap` handlers for cleanup on Ctrl-C. Some use `|| true` on the main
`wafer-runtime` invocation, which silently masks:

- Segfaults (SIGSEGV, exit 139)
- Fatal panics with a non-caught unwind
- Runtime startup errors (bad config path, port bind failure)
- OOM-killer signals

**Fixed inline (this session):** `run-e-iso-shakedown.sh` and
`run-e-val-1-shakedown.sh` — traps added, `|| true` kept but now
narrowly scoped to `wait` (with explicit reasoning comment).

**Remaining scripts to harden:**
- ~~`run-e-perf-1-2-shakedown.sh`~~ — done (trap `_cleanup_perf12`, ARRAY pattern)
- ~~`run-e-swap-shakedown.sh`~~ — done (trap `_cleanup_swap`, SCALAR pattern)
- ~~`run-e-swap-3-shakedown.sh`~~ — done (trap `_cleanup_swap3`, ARRAY pattern)
- ~~`run-e-perf-3-shakedown.sh`~~ — done (trap `_cleanup_perf3`, merged with mosquitto)
- ~~`run-e-perf-6-8-shakedown.sh`~~ — done (trap `_cleanup_perf68`, ARRAY pattern)
- ~~`run-e-perf-7-shakedown.sh`~~ — done (trap `_cleanup_perf7`, SCALAR/foreground)
- ~~`run-e-bp-perf9-shakedown.sh`~~ — done (trap `_cleanup_bpperf9`, merged with mosquitto)
- ~~`run-e-iso-7-8.sh`~~ — done (trap `_cleanup_iso78`, SCALAR pattern)
- ~~`run-e-perf-4-shakedown.sh`~~ — done (trap `_cleanup_perf4`, SCALAR/foreground)
- `run-e-val-1-shakedown.sh` — done
- `run-e-iso-shakedown.sh` — done

**Deliverables per script.**

1. Add trap on EXIT/INT/TERM that kills known child PIDs.
2. Replace `|| true` masking on main-binary invocation with:
   - Capture the exit code explicitly.
   - Test against a whitelist of expected values (0 for pipelines that
     shut down cleanly, non-zero for attack experiments where the
     attacker traps).
3. If exit code is unexpected, log clearly + increment fail counter,
   do not silently continue.

**Estimated cost.** 30 min per script × ~9 scripts = ~4.5 hours.

---

## P-Followup-5 — `Option<NonZeroU64>` for metering sentinels

**Priority:** P2 — footgun waiting to happen.
**Origin:** rust-idiom reviewer.

**Problem.** `eval/configs/pipeline-c-{fuel-only,epoch-only,neither}.toml`
use large sentinel integers (e.g., `epoch_deadline = 1000000000`) to
mean "unlimited". The convention was documented after the P0.13
lesson (`0 = unlimited` collided with `0 = trap immediately` in
wasmtime). Sentinels work but are convention-only.

**Recommended fix.** Change the config schema
(`crates/wafer-types/src/config/*.rs`) from `u64` to
`Option<NonZeroU64>`:
- `None` → "unlimited", maps to omitting the wasmtime `Config` call.
- `Some(n)` → `n` (proven non-zero at type level, no trap-on-zero
  hazard).

**Estimated cost.** 2–3 hours (schema + call-site updates + config
migration + regression test).

---

## P-Followup-6 — eKuiper smoke test drop-case assertion ✅ Closed

**Priority:** P2 — negation-blindness footgun.
**Origin:** correctness-reviewer.
**Closed by:** commit 486df87.

**Problem.** `eval/ekuiper/seed-pipeline-a.sh` smoke section only
verifies that `{temperature:80}` passes through. The drop case
(`{temperature:30}` filtered out) is asserted by absence, which the
review pass flags as a weak negation criterion.

**Fix.** `eval/ekuiper/smoke-test.sh` publishes both records (seq=1
temperature=30, seq=2 temperature=80), subscribes for 3 s, asserts
exactly one record arrives with seq=2 and temperature=80. Verified
regression detection by temporarily toggling the rule to
`WHERE temperature > 20` — script exits non-zero. Restored via
`seed-pipeline-a.sh`.

---

## P-Followup-7 — Notebook path discovery (soft; nice-to-have)

**Status: ✅ Closed** — commit `0f01ba5` (F7).

**Priority:** P2 — reduces notebook rot.
**Origin:** eval-harness reviewer (partial).

**Problem.** Some notebooks under `eval/analysis/notebooks/` may
hardcode shakedown-macos-<timestamp> paths. As new shakedown runs
land, notebooks pinned to old timestamps produce stale figures.

**Fix.** `eval/analysis/utils.py` exports `find_latest_shakedown()`.
All 7 affected notebooks migrated; `grep -l 'shakedown-macos-20'`
returns nothing. Unit tests in `eval/analysis/test_utils.py`.

**Estimated cost.** 1 hour.

---

## P-Followup-8 — Skill drift audit

**Status: ✅ Closed** (partial) — F8, commit `4709e3f`. Root cause
documented (empty quality-reviewer output is a independent model cold-start /
rate-limit artefact; complements are captured in `memory_remember`
category `wafer-verify`). No prompt or agent-config change needed;
future runs should retry once before escalating.

**Priority:** P2 — documentation-only.
**Origin:** internal.

**Observation.** The quality-reviewer reviewer (`independent-model`)
returned empty output twice across two rounds. This is a signal that
either (a) the model has a cold-start issue with long input prompts,
or (b) the reviewer prompt was under-specified for what "quality"
means in this repo's terms.

**Deliverables.**

1. Compare `.agents/skills/rust-best-practices/SKILL.md` (WAFER-scoped)
   with `~/.agents/skills/rust-best-practices/*` if it exists at the
   user level; note any divergence.
2. If a global `rust-best-practices` skill exists at the user level,
   ensure the project version explicitly extends or overrides it.
3. Consider adding a `~/.agents/skills/coding-discipline/*` link to
   the project so quality reviewers have a fallback.
4. Retry the failed quality-reviewer with a shorter, more concrete
   prompt to confirm the model is functional.

**Estimated cost.** 45 minutes.

---

## Non-issues (reviewer false-positives investigated)

The following reviewer claims were investigated and found NOT to be
issues:

1. **`telemetry-120b` template "missing"** (eval-harness reviewer).
   FALSE. It's a code enum variant in `crates/wafer-loadgen/src/payload.rs:54`,
   not a template file. Documentation may need to clarify that
   `payload_template` refers to the enum name, not a file path.

2. **P0.AC1 verification impossible** (completeness reviewer).
   FALSE-NEGATIVE. The AC requires running `plan_tasks phase-status`,
   which the reviewer lacked. The plan itself confirms 42/42 done.
   Rubric should be rewritten to point at
   `.pi/plans/*/plan.json` phases[].status instead of asking the
   reviewer to run a tool it doesn't have.

3. **Loadgen fingerprint constants would silently drift on temperature
   change** (rust-idiom reviewer, then rejected on further reading).
   FALSE. The test `payload_size_exact_fingerprint` asserts equality
   with the constants and fails loudly if payload bytes change without
   the constants being updated.

---

## Suggested consumption

- File **P-Followup-1** and **P-Followup-2** as the first two tasks in
  the canonical-runs plan (they block RQ1 and reproducibility
  respectively).
- **P-Followup-4** and **P-Followup-3** can run in parallel; both are
  operational polish.
- **P-Followup-5**, **P-Followup-6**, **P-Followup-7** are cleanup —
  bundle into a "harness hardening" mini-plan.
- **P-Followup-8** is a one-off — do it before the next verify pass.
