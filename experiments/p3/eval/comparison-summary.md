# Experimental P3 matched comparison

- Decision: `defer-p3-toolchain`
- Evidence class: experimental diagnostic; not thesis or release evidence
- Source: `0bf602c300e561f8f3d2e0c129f3a1fda508f67f`
- Admitted leaves: 108 (six complete matched triplets in each of six conditions)

## Results

| Condition | P3 message throughput Δ | P3 message p95 Δ | Message budget | P3 stream throughput Δ | P3 stream p95 Δ | Stream budget |
| --- | ---: | ---: | --- | ---: | ---: | --- |
| `120b-depth1` | +42.05% | -39.02% | pass | +124.66% | +1264.39% | fail |
| `120b-depth5` | +68.18% | -44.75% | pass | +158.69% | +1087.55% | fail |
| `1kb-depth1` | +40.90% | -38.83% | pass | +124.22% | +1248.11% | fail |
| `1kb-depth5` | +68.29% | -44.04% | pass | +151.34% | +1140.35% | fail |
| `100kb-depth1` | +10.13% | -11.69% | pass | +58.56% | +2206.26% | fail |
| `100kb-depth5` | +18.07% | -16.48% | pass | +31.98% | +2386.85% | fail |

All arms had zero loss and zero duplicates, preserved every compared envelope field, and reconciled the declared copy/allocation totals. P3 message passed the frozen throughput, p95, and RSS budgets in every condition. P3 stream increased throughput in every condition and met the predeclared high-pressure throughput-benefit threshold, but failed the p95 regression budget in every condition; its batch-completion latency is not interchangeable with message latency.

Setup is descriptive and excluded from steady state. The public P2 harness exposes load-to-ready as one operation, so its `instantiate_ns` includes its internal component reload and must not be compared directly with the P3 precompiled instantiation value.

## Fail-closed gate

Migration is blocked independently of performance:

- the maintained Go P3 path is blocked;
- the selected Wasmtime P3 implementation explicitly remains experimental and not production-ready; and
- the P3 stream regression gate failed.

The required outcome is therefore `defer-p3-toolchain`. Revisit only after Wasmtime P3 is production-ready and the maintained Go toolchain builds, validates, and executes both `wafer:pipeline@0.2.0` async message and stream worlds. Then rerun the full semantic and matched comparison gates.

Raw leaves and the independent recomputation are under the plan scratch directory at `comparison/`. They are deliberately not canonical evidence.
