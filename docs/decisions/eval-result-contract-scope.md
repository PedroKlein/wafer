# Result-directory contract scope: core vs optional artefacts

**Decision:** Option B — split the RESULT-CONTRACT into
`## Core artefacts (mandatory)` and `## Per-experiment optional
artefacts` with an explicit per-experiment matrix.

**Alternatives considered:**

- **Option A — universal collection.** Wire an always-on 1 Hz memory
  sampler and a runtime-side per-node metrics emitter into
  `wafer-runtime` so every result directory carries `memory.csv` and
  `per_node_metrics.csv`. Estimated cost 3–4 h (dep on the
  `memory-stats` crate to replace the existing `ps -o rss=` subprocess
  on macOS; author a criterion benchmark asserting < 0.1 % CPU
  overhead; wire cancel-safe shutdown; re-shakedown E-Val-1 and
  E-Perf-6). Produces the strongest contract but introduces new
  runtime surface area late in the eval-followups plan.
- **Option B — split contract.** Keep the current runtime scope and
  document which experiments require which files. Estimated cost
  30–60 min. Weaker single-source-of-truth story but matches how the
  harness already behaves and how notebooks already read data.

## Rationale for choosing B

1. **Reality tracks split-contract already.** Only E-Perf-6/7/8/9
   scripts write `memory.csv`; only E-Iso-7/8 scripts write
   `per_node_metrics.csv`. Making the contract match observed
   behaviour costs a doc edit; forcing behaviour to match a stricter
   contract costs a runtime refactor.

2. **F2 already covered runtime-owned provenance.** The heavy
   provenance-completeness pass (`wasmtime_version`,
   `config_sha256`, `wafer_plugin_hashes`, `wafer_runtime_sha256`,
   `rustc_version`, `kernel`) landed in `fcd855d`; the remaining gap
   is documentation, not code.

3. **Runtime-side migration deferred, not lost.** The option-A
   migration (memory sampling + per-node metrics emitter) is filed as
   a follow-up gap so canonical Pi runs can revisit it if the split
   contract proves too weak in practice.

4. **Constraint compliance.** The plan says the `memory_stats` crate
   must be the only source of RSS numbers. The existing
   `crates/wafer-core/src/bench/memory.rs::read_rss_bytes` still
   shells out to `ps` on macOS — a `memory_stats` swap is required
   whether or not the sampler is wired always-on. Filed alongside the
   option-A migration.

## Cost + trade-offs

|                    | Option A | Option B |
|---|---|---|
| Runtime surface | +new sampler task + shutdown hook + benchmark | none |
| Notebook impact | none (files just always exist) | must check per-experiment matrix |
| Script simplification | delete per-experiment sampler blocks | keep per-experiment sampler blocks |
| Canonical Pi run readiness | stronger (single sampler for all) | same as today |
| Risk of destabilizing runtime | non-zero | zero |
| Time to close F3 | 3–4 h | 30–60 min |
| Follow-up debt | none | option-A migration open |

## Follow-up

Option A remains on the table for the canonical-runs plan. When it
lands it MUST:

- Replace the `ps -o rss=` subprocess in
  `crates/wafer-core/src/bench/memory.rs::read_rss_bytes` (macOS
  path) with the `memory-stats` crate.
- Wire `MemoryRecorder::sample_loop` as an always-on 1 Hz task in
  `crates/wafer-runtime/src/main.rs` gated on
  `WAFER_BENCH_OUTPUT_DIR`.
- Ship a `cargo bench --bench overhead_of_memory_sampling` asserting
  < 0.1 % throughput hit vs a control run.
- Emit `per_node_metrics.csv` from `PipelineOrchestrator` on shutdown
  using the existing `NodeMetrics` atomic counters.
- Delete the per-experiment sampler blocks in `run-e-perf-6-8`,
  `run-e-perf-7`, `run-e-bp-perf9`, `run-e-iso-*` scripts once the
  runtime path is authoritative.

Filed as **A19** in `docs/status/implementation-gaps.md`.

## References

- `eval/RESULT-CONTRACT.md` § Core / Optional split (this decision).
- `plans/eval-followups.md` P-Followup-3.
- `.delegation-runner/artifacts/4f221368_reviewer_2_output.md` (eval-harness reviewer flagging the gap).
- `crates/wafer-core/src/bench/memory.rs` (existing sampler, not yet wired).
