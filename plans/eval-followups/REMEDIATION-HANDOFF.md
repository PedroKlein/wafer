# eval-followups — verify-flagged remediation (next session)

**Status:** Plan closed 8/8 with 12 commits on `main` (ahead of origin by 12), but
GPT-5.4 cross-family verify pass found **1 blocker + 6 real weaknesses** the
independent model pass missed. User approved `fix_all` (option 4, ~3-5h) but scheduled the
work for the next session after compact.

**Verify session that produced these findings:** end of the eval-followups
build session. Reviewers: `hai-proxy/independent-model` × 4 (safety timed out — separate
wrapper issue, not model). Base ref for the review range: `4b0f7b3`.

---

## The blocker (must fix)

### F5.AC2 — `None` must skip the wasmtime setter entirely

**AC verbatim:** *"Runtime engine wiring: when Some(n), wasmtime's Config gets
the value; when None, the corresponding limit-setting method is **skipped
entirely**. Verify: `cargo test -p wafer-core epoch_none_means_wasmtime_untouched`
— test inspects wasmtime Config state."*

**Actual code violates this.** Grep `map_or(u64::MAX, |n| n.get())` and
`map_or(u64::MAX / 2, |n| n.get())` — 11 call sites still call the setter with
a sentinel value:

- `crates/wafer-core/src/node/wasm.rs:119` (make_recovery_store epoch)
- `crates/wafer-core/src/node/wasm.rs:249, 308` (transform process + init fuel)
- `crates/wafer-core/src/node/wasm.rs:256, 311` (transform process + init epoch)
- `crates/wafer-core/src/node/wasm.rs:503, 506` (filter init fuel + epoch)
- `crates/wafer-core/src/node/wasm.rs:542, 545` (filter process fuel + epoch)
- `crates/wafer-core/src/node/wasm.rs:741, 744` (router init fuel + epoch)
- `crates/wafer-core/src/node/wasm.rs:780, 783` (router process fuel + epoch)
- `crates/wafer-core/src/orchestrator/launcher.rs:276-279` (transform load)
- `crates/wafer-core/src/orchestrator/launcher.rs:404-407` (filter load)
- `crates/wafer-core/src/orchestrator/launcher.rs:475-478` (router load)

**Fix pattern:** replace each `store.set_fuel(x.map_or(u64::MAX, |n| n.get()))`
with `if let Some(n) = x { store.set_fuel(n.get())?; }`, same for
`set_epoch_deadline`. Preserve the WHY comment.

**Test the AC calls for:** `cargo test -p wafer-core epoch_none_means_wasmtime_untouched`.
This test does not exist — must author. Wasmtime Config state cannot be
directly inspected (opaque type), so the test should instead:

1. Load a pipeline with `epoch_deadline = None` in TOML.
2. Set the engine's internal epoch counter far past a synthetic deadline.
3. Run a plugin that would normally trap.
4. Assert the plugin returns Ok (no trap) — proves the setter was skipped.

Alternative: wrap the setter calls in a `#[cfg(test)]` counter and assert
counter == 0 when config is None.

---

## Other findings (all real, all fix per user's `fix_all` option)

### F1 regression test bypasses launcher wiring

**Reviewer:** *"Would still pass if F1 launcher wiring were removed."*

`crates/wafer-core/tests/native_threshold_filter.rs:53` constructs
`FilterNode::from(NativeFilter::range("t", FIELD, MIN, MAX))` directly instead
of going through `launch_pipeline` on `eval/configs/pipeline-a-native.toml`.

**Fix:** add a new test that reads `pipeline-a-native.toml`, calls
`load_config` + `launch_pipeline`, drives a message through the pipeline,
asserts the filter drops out-of-range and forwards in-range. Keeps the
existing 3 tests too — they cover the low-level contract.

### F2 test bypasses the metadata.json merge

**Reviewer:** *"Would still pass if `_write_metadata` stopped merging
runtime-provenance.json into metadata.json."*

`crates/wafer-runtime/tests/metadata_provenance.rs` only inspects the
sidecar file, never the merged `metadata.json` from `run-experiment.sh`.

**Fix:** add a second test path that invokes
`eval/scripts/run-experiment.sh` with a small config, then asserts
`OUT_DIR/metadata.json` contains ALL six provenance keys. Or invoke
`_write_metadata` directly by sourcing the script's function.

### F3.AC5 — two re-shakedowns confirming the split not performed

**AC:** *"pick one experiment that didn't have memory.csv before (e.g., E-Val-1)
and one that did (E-Perf-6). Both produce contract-conformant output."*

**Fix:** run `bash eval/scripts/run-e-val-1-shakedown.sh --runs 1` and
`bash eval/scripts/run-e-perf-6-8-shakedown.sh --experiments perf-6 --runs 1`,
then run `python3 -c` inline assertion checking that E-Val-1 lacks
`memory.csv`/`per_node_metrics.csv` (per new optional matrix) but has all
core artefacts, and E-Perf-6 has both optional + all core. Commit result dirs.

### F4 close-SHA not pinned in plan

Section `## P-Followup-4` in `plans/eval-followups.md` lacks a
`**Status: ✅ Closed** — commit <SHA>` line. F3/F6/F7/F8 all have theirs.

**Fix:** add closed-status line pointing to commit `223d83f`.

### F7 test doesn't distinguish lex vs mtime

`eval/analysis/test_utils.py::test_multiple_dirs_returns_newest` creates
dirs in `(mid, old, new)` order — mtime sort would return `new` too.

**Fix:** create in `(new, mid, old)` order (mtime `new` = oldest) and assert
`find_latest_shakedown` still returns `new` (proves lex sort). Alternative:
mock `pathlib.Path.stat` and verify it was NOT called.

### F7.AC4 — notebooks re-executed but outputs stripped

`grep -c '"outputs": \[\]' eval/analysis/notebooks/*.ipynb` shows outputs are
empty. AC required "non-trivial cell outputs" committed.

**Fix:** `uv run jupyter execute eval/analysis/notebooks/*.ipynb --inplace`,
verify each notebook has at least one cell with `"outputs": [...non-empty...]`,
commit. Requires shakedown result dirs to be present.

### Quality: launcher error message diverges from accepted values

`crates/wafer-core/src/orchestrator/launcher.rs:307-319` accepts
`"threshold" | "threshold-filter" | "range"` but error text says
`(valid: threshold)`.

**Fix:** change error text to `(valid: threshold, threshold-filter, range)`
OR narrow the match arm to only `"threshold"`. Prefer widening the error
message — the three aliases are documented in
`eval/configs/pipeline-a-native.toml` comments.

### Quality: utils.py bypasses eval/analysis/src/wafer_analysis/ package

`eval/analysis/utils.py` and `eval/analysis/test_utils.py` sit at the
crate root and use `sys.path.insert` for import. Adjacent code lives under
`eval/analysis/src/wafer_analysis/`.

**Fix:** move to `eval/analysis/src/wafer_analysis/paths.py`, update
notebook imports from `from utils import find_latest_shakedown` to
`from wafer_analysis.paths import find_latest_shakedown`, update test
imports, use PEP-604 `str | None` instead of `Optional[str]`.

---

## Non-issues (reviewer false-positives verified)

- F4.AC2 "|| true on main-binary invocation" — AC explicitly permits it on
  `wait $pid` (verbatim: *"narrowly scoped (on `wait $pid`, not on the launch
  itself)"*). Reviewer conflated `|| true` on `wait` with `|| true` on the
  runtime launch. All 26 `|| true` matches are on `wait` calls with adjacent
  WHY comments (commit `223d83f`).
- F1.AC5 "no re-shakedown dirs" — E-Perf-1 and E-Perf-2 DO have new dirs
  from `shakedown-macos-2026-08-01T19-48-15Z` and `-19-50-15Z`. Reviewer
  didn't check.
- F8.AC4 "retry not performed" — stakeholder-authorized divergence per F8
  constraint block ("If the answer is independent model rate-limit, document and move on
  — don't fix a phantom"). Already annotated in plan.

---

## Session context

**Verify skill:** `<home>/.agents/skills/verify/SKILL.md`. Cross-family
required: builder is primary model family Claude-4.7-Opus, so reviewers must be non-
primary model family. Available via hai-proxy: independent-model (used here, worked), independent-model
(known flaky, memory lesson `wafer-verify`).

**pi-caffeinate wrapper bug:** every reviewer run reports `exit code 1` after
the model finishes cleanly. Verdict text IS captured in the failure body —
parse the transcript. Do NOT interpret exit-1 as failure. Memory lesson
recorded.

**Workspace test hang:** `cargo test --workspace` hangs at `attack_containment`.
Per-crate `cargo test -p wafer-*` works. Memory lesson recorded. Do NOT run
workspace test to verify these fixes — use per-crate.

**Skills to load for the remediation:** `building`, `tdd`, `proof-of-work`,
`rust-best-practices`.

---

## Estimated remediation order

1. **F5.AC2** (blocker): edit 11 setter sites in wasm.rs + launcher.rs,
   author `epoch_none_means_wasmtime_untouched` test, run
   `cargo test -p wafer-core --lib` + `-p wafer-config --test epoch_zero_rejected`.
   ~60-90 min.
2. **F1 launcher-wired test**: author regression test that goes through
   `launch_pipeline`. ~30 min.
3. **F2 metadata.json merge test**: extend `metadata_provenance.rs` to also
   invoke `run-experiment.sh` and check merged JSON. ~30 min.
4. **F3.AC5 shakedowns**: two re-runs + jq assertion. ~30 min.
5. **F7 test tightening + F7.AC4 notebook outputs**: fix ordering + re-execute
   headless with outputs committed. ~30 min.
6. **Quality nits**: launcher error message + utils.py package move.
   ~20 min.
7. **F4 plan close-SHA pin**. ~2 min.
8. **Final verify** with fresh independent-model reviewers on the fix diff. Skip independent model.

**Total estimate:** 3.5-4h. Matches the option-4 user estimate.

## Commands to run at session start

```bash
cd <repo>
git log --oneline -12  # verify at 1f81d54
plan_tasks action=status  # verify 8/8 done
grep -c "map_or(u64::MAX" crates/wafer-core/src/node/wasm.rs crates/wafer-core/src/orchestrator/launcher.rs  # F5 blocker sites
```
