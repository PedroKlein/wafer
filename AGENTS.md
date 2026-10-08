# Agent Instructions

## Thesis Context

This repo is the **experimental artifact** for an undergraduate thesis (TCC, UFRGS). The research design, evaluation methodology, and thesis framing live in a sibling repo.

| What you need | Where to find it |
|---------------|------------------|
| **What to build next** | GitHub issues. `ROADMAP.md` gives the longer-horizon narrative. |
| **Current implementation state** | `docs/status/implementation-status.md` (what's built) + `docs/status/implementation-gaps.md` (documented drift with per-gap fix plans) + `docs/status/canonical-readiness.md` (what the final campaign still needs) |
| **Which document is authoritative** | `tcc-doc/SOURCES-OF-TRUTH.md` |
| **How experiments should run** | `tcc-doc/research/analysis/evaluation-plan.md` (methodology), `eval/canonical-matrix.json` (the executable schedule and host profiles), `docs/eval/pi5-experiment-runbook.md` (operator steps), `eval/RESULT-CONTRACT.md` (per-experiment output-directory shape) |
| **Evaluation numbers** | None are final yet. Every number under `docs/benchmarks/` or `docs/history/` is a pre-final diagnostic; never quote one as a result. |
| **RQs and pass/fail criteria** | `tcc-doc/research/analysis/thesis-statement-v3.md` |
| **Pipeline topologies to implement** | `tcc-doc/context/use-cases.md` |
| **Old RQ4/5/6 references** | `tcc-doc/RQ-VERSION-MAP.md` (they map to current RQ1–3) |
| **Literature on a topic** | Obsidian vault `TCC/papers/` |

**Sibling repos:**
- `github.com/PedroKlein/tcc-doc` — research, evaluation plan, thesis writing
- `github.com/PedroKlein/obsidian-personal` — knowledge base (notes under `TCC/`)

**Key framing:** The runtime IS the contribution (not just hot-swap). RQ1=Performance, RQ2=Isolation, RQ3=Hot-swap.

---

## Project Overview

**WAFER** (WebAssembly Flow Execution Runtime) is a single-process Rust runtime that executes typed DAGs of WebAssembly components on edge gateways. It uses [Wasmtime](https://wasmtime.dev/) with the Component Model. Pipelines are declared in TOML, connected by bounded `mpsc` queues with backpressure, and individual Wasm nodes can be hot-swapped between messages without pipeline downtime.

### Key Concepts

- **DAG pipelines** — Data flows are declared as directed acyclic graphs via TOML configuration files. Cycles are rejected at build time.
- **Five node categories** — `Source`, `Sink`, `Transform`, `Filter`, `Router`. Sources and Sinks are native Rust; Transform / Filter / Router are Wasm components.
- **WIT-typed boundaries** — Project interfaces live in one `wafer:pipeline@0.1.0` package with four worlds: `transform-node`, `filter-node`, `router-node`, and the capability-gated `inference-node`. The last also imports the pinned `wasi:nn` package.
- **Inference is default deny** — Only a Wasm Transform with `allow_inference = true` receives the wasi-nn linker and ONNX backend. The immutable grant survives recovery, reconfigure, hot-swap, and rollback; a requested GPU target alone is not provider-selection evidence.
- **Bounded queues** — Every edge is a bounded tokio `mpsc` channel with a configurable capacity and an overflow policy (`slow`/backpressure, `drop`, `dead-letter`). Backpressure propagates end-to-end.
- **Fan-out / fan-in** — Fan-out is expressed by Router nodes (1→N) that return output port names. Fan-in (N→1) is an **implicit host topology**: multiple upstream nodes are wired as multi-producer senders on the downstream node's single `mpsc` receiver. There is no first-class Joiner node type; fan-in requires no Wasm.
- **Hot-swap (watch-channel, between messages)** — Each Wasm node's task holds a `watch::Receiver<Option<SwapPayload>>`. At the top of every runner loop iteration the task polls `swap_rx.has_changed()` — if a swap payload is available it drops the old instance and installs the pre-instantiated replacement, then enters its usual `select!` between cancellation, the swap signal and the input channel (so an idle node wakes for a swap). State inside the guest instance is lost by design (see Invariant 7).
- **Zero-copy envelope** — Messages cross the boundary as an `Arc<EnvelopeHeader>` plus `Bytes` payload plus lineage trail. Guests read the payload via a `borrow<buffer>` resource handle exposed by `pipeline:types`.
- **Error policy** — Every host-observed guest error maps to one of five categories (`bad-input`, `dependency-failed`, `processing-failed`, `timed-out`, `unrecoverable`) which the policy engine dispatches to `retry` / `dead-letter` / `skip` / `teardown`.
- **Multiple I/O** — Native sources and sinks support stdin/stdout, files, MQTT pub/sub, and HTTP webhooks.
- **Control plane** — axum HTTP REST API + Prometheus metrics on a separate port.
- **OCI registry** — Wasm components can be pulled from container registries (ghcr.io, Docker Hub) via the same `plugin` field used for local paths, with content-addressable local caching.
- **Fuel + epoch metering** — When a config enables them (both default to off), guests are bounded in both computation (fuel) and wall-clock time (epoch interruption on a dedicated OS thread).

### Tech Stack

- **Rust stable** (toolchain pinned in `rust-toolchain.toml`, Edition 2024)
- **wasmtime** — WebAssembly runtime with WASI Preview 2 / Component Model
- **wit-bindgen** — Code generation from WIT interface definitions
- **petgraph** — DAG topology management (toposort + cycle detection)
- **tokio** — Async runtime (`mpsc`, `watch`, `select!`)
- **axum** — HTTP control plane server
- **prometheus-client** — Metrics exposition

### Workspace Crates

| Crate | Type | Description |
|-------|------|-------------|
| `wafer-core` | Library | Core runtime: DAG orchestrator, engine, node runners, queue wiring, hot-swap, error policy, metrics, HTTP API surface |
| `wafer-runtime` | Binary | CLI runtime that loads TOML configs and runs pipelines; wires the API server into `wafer-core` |
| `wafer-types` | Library | Shared types: config schema (`NodeDef`, `WasmNodeDef`, `EdgeDef`, `EngineConfig`, `ErrorPolicyConfig`), control messages, event types, metrics types |
| `wafer-config` | Library | TOML loading, DAG construction, semantic validation on top of `wafer-types` |
| `wafer-plugin` | Library | Guest-side SDK (macros, `thread_local!` state pattern, error helpers) that plugin crates depend on |
| `wafer-loadgen` | Library + Binary | Load generator used by the evaluation harness (open-loop, HdrHistogram); the library exposes the publisher, subscriber, and recorder to tests |
| `waferctl` | Binary | CLI tool for interacting with running pipelines (status, nodes, hot-swap, shutdown) |

### Key Directories

| Directory | Contents |
|-----------|----------|
| `crates/` | Rust workspace crates (see table above) |
| `plugins/` | Wasm plugin source (e.g. `pass-through`, `uppercase`, `json-parse`, `threshold-filter`, `content-router`, `mnist-inference`, `tensor-prep`, `cayenne-decoder`, `quality-rules`, `anomaly-detector`, `vibration-features`, `result-format`, `attacks/…`) |
| `wit/` | The single `wafer:pipeline@0.1.0` project package, the four `transform-node` / `filter-node` / `router-node` / `inference-node` worlds, and pinned dependency WIT |
| `examples/` | Runtime-schema pipeline TOML examples (passthrough, uppercase, filter, chain, fanout, file-io, mqtt, http, overflow-dlq-demo, metrics-demo, mnist-inference, remote OCI, …). See `examples/README.md` before copying config shape. |
| `tests/` | Shared test fixtures (integration tests live in each crate's `tests/`) |
| `docs/` | Project documentation (see Documentation Map below) |
| `scripts/` | Helper scripts |
| `models/` | ML model files (e.g., MNIST ONNX model for inference plugin) |
| `eval/` | Evaluation harness inputs / outputs |

---

## Documentation Map

When you need deeper context on any aspect of the project, consult these files. The `docs/` tree follows an arc42-lite layout: architecture views, RFCs, ADRs, interfaces, operations guides, evaluation runbooks and status reports. `docs/history/` is an archive: every file there opens with an "Archived, not current" banner, and nothing in it describes the runtime or the evaluation as they are today. Each entry includes a summary so you know what to expect before reading. If documentation and implementation disagree, trust the source code, WIT files under `wit/`, and `docs/status/implementation-gaps.md`.

### Status & Evaluation

| Document | Summary |
|----------|---------|
| `docs/status/implementation-status.md` | Current implementation state — what's built, what's tested, per-plugin coverage. Replaces the old monolithic MVP doc. |
| `docs/status/implementation-gaps.md` | **Drift ledger.** Every documented behaviour the runtime does not yet implement, keyed by gap ID. Only A20 is open; closed entries are archived. Every RFC/ADR/architecture chapter with an aspirational banner points here. **Consult before assuming code matches docs.** |
| `docs/status/canonical-readiness.md` | What the final campaign still needs and the current claim boundaries. |
| `docs/benchmarks/README.md` | Current benchmark reference pages. The shakedown pages are archived under `docs/history/benchmarks/`. |
| `docs/benchmarks/hot-swap.md` | Hot-swap phase timing reference. |
| `docs/eval/pi5-experiment-runbook.md` | **Operator runbook** for the campaign: gates, diagnostic batch, launch, resume, approval, analysis. |
| `docs/eval/pi5-host-setup.md`, `jetson-host-setup.md`, `x86-host-setup.md` | Host preparation for the canonical Raspberry Pi 5 and the Jetson and x86 replication hosts. |
| `docs/eval/cross-compile.md` | aarch64-linux cross-compile recipe via docker (`mise run cross-build-pi`) and the glibc each binary needs. |
| `eval/canonical-matrix.json` | **Executable source** of the campaign: experiments, repetitions, rates, host profiles. |
| `eval/RESULT-CONTRACT.md` | **Authoritative shape** of every result directory under `eval/results/`. Every notebook and the result verifier assume this contract. |

### Archive

`docs/history/` holds archived plans (`plans/`), the pre-final readiness and progress logs and the closed gap entries (`status/`), the shakedown benchmark pages (`benchmarks/`) and the Phase 0 session recipes (`workflows/`). Read them only for background; see `docs/history/README.md`.

### Design & Specification

| Document | Summary |
|----------|---------|
| `docs/architecture/` | arc42-lite architecture views: vision, goals & constraints, solution strategy, building blocks, runtime view, deployment, cross-cutting concepts, quality requirements, risks, comparators. |
| `docs/status/implementation-status.md` | Current implementation status — what's built, what's tested, per-plugin coverage. Replaces the old monolithic MVP status doc. |
| `docs/rfcs/` | RFC archive — long-form design decisions with Abstract, Alternatives Considered, Related RFCs, Implementation Notes. Eleven RFCs cover WIT contracts, host runtime, node types, config schema, orchestrator, plugin SDK, performance, evaluation harness, implementation architecture, I/O integration, and WASI 0.3 / Component Model evolution. |
| `docs/adr/` | Architecture Decision Records in Michael Nygard format (short, executive). Eighteen ADRs at present. See `docs/adr/README.md` for the index and conventions. |
| `ROADMAP.md` | Aspirational / longer-horizon items flagged in RFCs and the evaluation plan. |

### API & Integration

| Document | Summary |
|----------|---------|
| `docs/interfaces/http-api.md` | HTTP control plane reference. Current endpoints: `/health`, `/ready`, `/metrics`, `/api/v1/nodes`, `/api/v1/nodes/{id}`, `/api/v1/nodes/{id}/hot-swap`, `/api/v1/nodes/{id}/reconfigure`, `/api/v1/pipeline/shutdown`. |
| `docs/interfaces/wit-contracts.md` | Reference for the single `wafer:pipeline@0.1.0` WIT package, its interfaces, and its four worlds. |
| `docs/interfaces/config-schema.md` | TOML config reference. Nodes use a `[nodes.NAME]` map, each `WasmNodeDef` has a single `plugin` field (local path or OCI reference), and each `EdgeDef` has a single optional `port` field (only used for router outputs). |
| `docs/interfaces/plugin-sdk.md` | Reference for the `wafer-plugin` guest SDK (macros, `thread_local!` + `RefCell` state pattern, error helpers). |
| `docs/api/openapi.yaml` | OpenAPI 3.0 spec for the control plane (kept in sync with the axum handlers). |
| `docs/api/bruno-collection/` | Bruno HTTP client collection for interactive API testing. |
| `docs/operations/registry.md` | OCI registry integration guide — publishing Wasm components to ghcr.io/Docker Hub, pulling via the `plugin` field, configuring caching, and using `wkg` tooling. |
| `docs/operations/mqtt-setup.md` | Local MQTT broker setup (Docker Mosquitto) for developing and testing MQTT sources and sinks. |

### Root Files

| File | Summary |
|------|---------|
| `README.md` | Project README — quick start, prerequisites, project structure, development setup, running pipelines, building plugins. Post-migration, this is a one-page quickstart that points into `docs/`. |
| `mise.toml` | Primary non-Rust developer toolchain and command-runner config. Run `mise run setup` for helper tools and `mise tasks ls --all` for tasks. |
| `rust-toolchain.toml` | Pinned Rust toolchain (1.98.1, `wasm32-wasip2` target). |
| `rustfmt.toml` | Formatter configuration. |
| `Cargo.toml` | Workspace root. Defines workspace members, shared dependencies, and profiles. |

---

## Build & Test Commands

This project uses [`mise`](https://mise.jdx.dev/) as the primary non-Rust development tool manager and command runner. Non-Rust helper tools and tasks are defined in `mise.toml`; Rust itself remains controlled by `rust-toolchain.toml` and must be installed through rustup first. If mise reports that `mise.toml` is not trusted, run `mise trust` once for this repo, then `mise run setup` to verify Rust/rustup and install pinned helper tools.

### Core Workflow

```bash
mise run setup          # Verify Rust/rustup, then install pinned helper tools
mise tasks ls --all     # List all available tasks
mise run build          # Build entire workspace
mise run test           # Run all tests
mise run test-eval      # Run the evaluation harness tests (Python and shell)
mise run check          # Type-check without building
mise run fmt            # Format code with rustfmt
mise run clippy         # Run clippy lints
```

### Building Components

```bash
mise run build-runtime  # Build wafer-runtime only
mise run build-ctl      # Build waferctl only
mise run //plugins:build-plugins      # Build all WASM plugins
mise run //plugins:build-plugin NAME  # Build a specific plugin (e.g., mise run //plugins:build-plugin uppercase)
```

### Cross-Compile (aarch64 Linux — Pi/Jetson target)

```bash
mise run cross-build-pi        # Build wafer, wafer-loadgen, waferctl for aarch64 Linux
                               # via docker run --platform linux/arm64 rust:<toolchain>-slim-bookworm
                               # (toolchain from rust-toolchain.toml).
                               # Output: target/docker-aarch64-linux/release/{wafer,wafer-loadgen,waferctl}
mise run cross-build-pi-check  # Verify the three binaries are aarch64 ELF via `file(1)`. CI-friendly.
```

See `docs/eval/cross-compile.md` for the design rationale (why docker over the `cross` crate) and the docker `--platform` compatibility path on M-series hosts. `.github/workflows/cross-arch.yml` builds the release binaries for aarch64 (on a native arm64 runner) and x86_64 when Rust sources, Cargo files, `rust-toolchain.toml`, `mise.toml` or the glibc script change; `.github/workflows/ci.yml` runs the docs check, fmt, clippy, the workspace tests and the evaluation tests, and on pull requests skips the suites the diff cannot affect.

### Analysis Notebooks

```bash
mise run //eval:notebooks                          # Open all 13 analysis notebooks in JupyterLab (browser)
mise run //eval:notebooks-view 05-hotswap-timeline # Render one notebook to HTML + open in browser (read-only)
mise run //eval:notebooks-execute                  # Re-execute all notebooks against current eval/results/ data
mise run //eval:figures                            # Re-execute notebooks + list regenerated PDFs under eval/analysis/figures/
```

Notebooks live under `eval/analysis/notebooks/`; the uv project (`eval/analysis/pyproject.toml`) declares numpy, matplotlib, seaborn, pandas and Jupyter. `notebooks-view` is the fastest path for reading rendered analysis without launching a live kernel. See `eval/analysis/notebooks/README.md` for the notebook-to-experiment mapping.

### Evaluation Campaign

Run these on the evaluation host itself (from its `~/wafer` checkout), except `preflight-pi5`, which uses SSH (`PI_HOST=user@host`), and `approve-batch`, which runs on the analysis machine against the results root that holds the host's copied batch. The full sequence is in `docs/eval/pi5-experiment-runbook.md`.

```bash
mise run preflight-pi5                               # or preflight-jetson / preflight-x86 on those hosts
mise run plan-campaign -- --host rpi5                # Print the schedule (no runs)
mise run run-campaign -- --host rpi5 --capacity-scout --batch-id SCOUT   # Next scout probe; repeat until it stops
mise run run-campaign -- --host rpi5 --batch-id ID --scout-batch-id SCOUT   # Run or resume a batch
mise run run-campaign -- --host rpi5 --batch-id ID --scout-batch-id SCOUT --repetitions 3   # Diagnostic batch, never thesis evidence
mise run smoke-swap3 -- --host rpi5 --batch-id ID    # Every E-Swap-3 arm once before a batch, diagnostic
mise run campaign-status -- --host rpi5 --batch-id ID
mise run approve-batch -- --host rpi5 --batch-id ID  # Check a finished final batch, record it in eval/final-batches.json
mise run idle-baseline-pi5                           # Diagnostic idle-power baseline
mise run instrument-ab-pi5                           # Cost of the host telemetry sidecars
```

### Running Pipelines

```bash
mise run run                                # Run default passthrough pipeline
mise run run examples/dag-uppercase.toml    # Run a specific pipeline config
mise run run-remote                         # Run with OCI-hosted plugins
```

### Testing

```bash
cargo test --workspace                          # All tests
cargo test -p wafer-core                        # Single crate
cargo test -p wafer-core --features http-api    # With feature flags
cargo test -p wafer-runtime --test exit_status  # One integration test file
cargo test --workspace -- --nocapture           # With stdout output
```

### Debugging

```bash
RUST_LOG=wafer=debug cargo run -p wafer-runtime -- --config examples/dag-passthrough.toml
RUST_LOG=wafer=trace cargo run -p wafer-runtime -- --config examples/dag-passthrough.toml
```

The runtime adds an INFO default to the `RUST_LOG` filter, so a bare level such as `RUST_LOG=debug` still logs at INFO. Name a target instead; `wafer` also matches the `wafer_*` crates.

---

## Coding Conventions

For detailed Rust idioms, patterns, and style guidance, load the relevant skill for your task:

| Skill | When to Load |
|-------|-------------|
| `rust-best-practices` | Writing or reviewing any Rust code — covers idiomatic patterns, borrowing vs cloning, error handling, testing, documentation, and clippy usage |
| `cargo-expert` | Managing dependencies, workspace configuration, build targets, profiles, or troubleshooting build issues |
| `wasm-specialist` | Working with wasmtime, Wasm plugin architecture, WIT interface definitions, WASI capabilities, or Component Model design |
| `async-tokio` | Working with `mpsc`, `watch`, `select!`, cancellation, or the hot-swap coordination code |
| `dag-orchestration` | Working with pipeline topology, petgraph, or fan-in/fan-out wiring |
| `rust-testing` | Adding integration/unit tests, benchmark setup, test harness design, mocking wasmtime state |
| `observability` | Working with `tracing` spans, Prometheus metrics, log-level policy, span attributes, or the `/metrics` endpoint |
| `oci-distribution` | Working with OCI registry pulls, `wkg` tooling, plugin publishing, or the content-addressable AOT cache |
| `mqtt-iot` | Working with MQTT sources/sinks, mosquitto setup, or QoS/topic patterns |
| `cli-design` | Working on `waferctl` or `wafer-runtime` CLI ergonomics, clap argument shapes, output formatting |

Always-loaded: `wafer-project` (project identity, thesis contribution framing, invariants, comparators). Do not load it explicitly — it's discovered automatically.

### Key Rules

- **Formatting**: All code must pass `rustfmt` (configuration in `rustfmt.toml`)
- **Linting**: All code must pass `clippy -D warnings` — warnings are treated as errors
- **Toolchain**: Pinned in `rust-toolchain.toml` — do not change without discussion
- **Tests**: Add tests for new functionality. Run `mise run test` before considering work complete
- **WASM target**: Plugins compile to `wasm32-wasip2`. The toolchain file configures this target automatically

---

## ADR & RFC Workflow

Two-tier decision archive:

- **RFCs** (`docs/rfcs/RFC-NNN-<slug>.md`) capture the long-form reasoning: abstract, decision, alternatives considered, related RFCs, implementation notes.
- **ADRs** (`docs/adr/NNNN-<slug>.md`) are short Nygard-format summaries (Context / Decision / Consequences / See Also) that link back to their parent RFC.

When an architectural decision is needed:

1. **Research** options and document trade-offs
2. **Draft** an RFC under `docs/rfcs/` (or a proposed ADR under `docs/adr/` for narrower decisions)
3. **Follow** the templates and index entries in `docs/rfcs/README.md` and `docs/adr/README.md`
4. **Present** to the user for review — user accepts or rejects
5. **On acceptance**, update status to **Accepted** / **Implemented** and create implementation tasks. When an RFC introduces a distinct decision worth surfacing separately, also add a Nygard ADR that links back.

## Commits and branches

- Commit messages follow Conventional Commits: `type: short imperative summary`,
  lower case, no trailing period, subject line only unless a body is genuinely
  needed. Types: `feat`, `fix`, `refactor`, `perf`, `test`, `docs`, `build`,
  `ci`, `chore`.
- Branch names follow `type/short-kebab-summary` with the same types (for
  example `docs/chapter-3-revision`, `feat/bounded-outbound-http`). Do not
  push work to auto-generated agent branches such as `claude/<random-name>`;
  create a branch that follows this pattern instead.
- Commits and pull requests carry no AI attribution: no `Co-Authored-By`,
  `Claude-Session`, or "Generated with" lines.
