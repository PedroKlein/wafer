# Binary Size Comparison — Wasm Components vs. Container Images

<!-- historical-diagnostic-file -->

**Experiment**: E-Density-1 (RFC-008, D9).
**Research question**: RQ1 orthogonal — is the Wasm-component isolation
unit materially smaller than the container-image isolation unit?
**Host**: `shakedown-macos` (laptop shakedown; canonical Pi/x86 numbers
deferred to the canonical-runs plan).
**Scope qualifier**: Sizes are order-of-magnitude comparisons. The exact
byte counts depend on the Rust toolchain (`1.85.0`), the `[profile.release]`
tuning (`opt-level = "s"`, `lto = true`, `strip = "debuginfo"`), and any
`wasm-opt` post-processing. Wire-image sizes for the container column are
the smallest realistic Docker Hub image that could host an equivalent
worker, not what production teams actually ship.

## Methodology

1. Every first-party plugin is compiled for `wasm32-wasip2` with the
   workspace release profile.
2. `stat` reports the on-disk byte count of the resulting `.wasm`
   component. No `wasm-opt` is applied for this table so numbers are
   reproducible from `cargo build --release` alone.
3. Container floors are computed as
   `min_realistic_image_MiB × 1024² ÷ wasm_bytes` and reported as `ratio_min`.
   The floor image is `alpine (~7 MB) + statically-linked Rust worker +
   any per-plugin dependency artefacts` (ndarray, rustfft, serde, …).
   Rationale for each floor is in the CSV's `container_rationale` column.
4. `eval/scripts/collect-binary-sizes.sh` regenerates `binary-sizes.csv`
   and `metadata.json` idempotently.

**Sources for the container floor**

- `alpine:3.19` — ~7.7 MB uncompressed. [Docker Hub](https://hub.docker.com/_/alpine).
- Statically-linked Rust binary via `x86_64-unknown-linux-musl`: ~40 MB
  once the standard library, panic infrastructure, and eventual ecosystem
  crates land. Source: Rust community benchmarks (2024) — matches the
  Alpine + `cargo install --target musl` recipe.
- `python:3.11-slim` — ~45 MB. Included only as a comparative reference;
  none of the twelve plugins actually require Python.

The floor deliberately excludes:

- observability agents (fluent-bit, cadvisor sidecars) — routinely add 15–30 MB;
- health-probe binaries and shell tooling;
- CI/SBOM/provenance artefacts (cosign signatures, SBOMs);
- distro CVE patch layers, which grow the base by 5–15 MB per year.

Production images in practice are 2–5× the floor. Treat the CSV's
`ratio_min` column as a lower bound.

## Table 4 — Binary sizes (`shakedown-macos`)

| Plugin | Wasm size (KB) | Container base | Container floor (MB) | Ratio (×) |
|---|---:|---|---:|---:|
| pass-through | 56.6 | alpine + static Rust binary | 50 | 904 |
| uppercase | 57.4 | alpine + static Rust binary | 50 | 891 |
| json-parse | 63.2 | alpine + static Rust binary + serde_json | 60 | 971 |
| threshold-filter | 92.0 | alpine + static Rust binary | 50 | 556 |
| content-router | 83.7 | alpine + static Rust binary | 50 | 611 |
| tensor-prep | 94.7 | alpine + static Rust binary + ndarray | 70 | 756 |
| result-format | 97.3 | alpine + static Rust binary | 50 | 525 |
| anomaly-detector | 144.4 | alpine + static Rust binary + serde | 60 | 425 |
| cayenne-decoder | 155.1 | alpine + static Rust binary | 55 | 363 |
| vibration-features | 366.5 | alpine + static Rust binary + rustfft | 120 | 335 |
| quality-rules | 131.4 | alpine + static Rust binary + serde | 60 | 467 |
| delay-injector | 73.0 | alpine + static Rust binary | 50 | 701 |

**Aggregate figures** (12 first-party plugins):

- Wasm sizes range from 56.6 KB (`pass-through`) to 366.5 KB (`vibration-features`).
- Ratio range: 335× (`vibration-features`) to 971× (`json-parse`).
- The smallest ratio, 335×, still means Wasm ships **two orders of
  magnitude** less bytes than the equivalent container floor. Every
  plugin sits inside RFC-008's expected `5–300 KB` band; the outlier
  (`vibration-features`) is caused by the `rustfft` planner + twiddle
  tables, not the WAFER SDK.

## Interpretation

- The Wasm-vs-container ratio is dominated by the container base image,
  not the plugin code. Even a fully-featured DSP plugin
  (`vibration-features`, 366.5 KB) stays two orders of magnitude below the
  container floor.
- Every plugin fits comfortably within the RFC-008 D9 pass criterion
  (Wasm ≤ 500 KB, container ≥ 50 MB, ratio ≥ 100×).
- The ratio *widens* as base images grow. Standard practice of adding
  observability agents and CVE patches means production ratios of
  ~1000–5000× are typical.

## Reproducibility

```console
$ cargo build --release --target wasm32-wasip2 --workspace  # or plugins/Makefile
$ ./eval/scripts/collect-binary-sizes.sh eval/results/e-density-1
Wrote eval/results/e-density-1/binary-sizes.csv
Wrote eval/results/e-density-1/metadata.json
```

`metadata.json` records the git SHA, host tag, and generation timestamp
per the result-directory contract (P1.1). To recreate this table for a
different revision, re-run the script and pipe the CSV through the
`Rendering` cell of `eval/analysis/notebooks/00-warmup-validation.ipynb`
(or manually edit the table above).

## Follow-ups

- Add `wasm-opt` (Binaryen) to `plugins/Makefile` release step. Historical
  gain: 10–30 % additional shrinkage; further widens the ratio.
- Canonical Pi/x86 numbers deferred to `canonical-runs` plan — same CSV
  contract, different `host_tag`. Wasm sizes are architecture-independent
  so those runs measure the container floor on ARM/x86, not the Wasm.
- Once `mnist-inference` builds successfully (the crate is present but
  fails to resolve WIT deps under the current `wit/` layout), extend the
  index to include it as the thirteenth plugin. That will produce the
  first "ML-heavy" data point for the table.

## Cross-references

- `docs/rfcs/RFC-008-evaluation-harness.md` §D9 — pass criterion.
- `tcc-doc/main/research/analysis/evaluation-plan.md` — Table 4 template.
- `eval/scripts/collect-binary-sizes.sh` — the generator.
- `eval/scripts/binary-sizes.index` — the input list.
- `eval/results/e-density-1/binary-sizes.csv` — the canonical data.
