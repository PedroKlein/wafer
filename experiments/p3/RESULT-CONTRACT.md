# Experimental P3 result contract

This contract governs only the non-canonical WASI P3 PoC. Results are `experimental-diagnostic`, set `thesis_evidence=false`, and never enter `eval/results/` or the canonical campaign.

## Authorities

- Lifecycle semantics: [`docs/adr/0017-experimental-wasip3-streaming.md`](../../docs/adr/0017-experimental-wasip3-streaming.md)
- Frozen factors and gates: [`experiment-contract.json`](experiment-contract.json)
- Ordered dry run: [`dry-run-schedule.json`](dry-run-schedule.json)
- Raw leaf schema: [`raw-leaf-schema.json`](raw-leaf-schema.json)
- Decision schema: [`decision-schema.json`](decision-schema.json)

The prose explains the contract; the JSON files are the executable inputs. A disagreement fails closed.

## Raw layout

Raw outputs live under the plan scratch directory, never under canonical evidence:

```text
<scratch>/comparison/
├── batch.json
├── raw/<condition>/triplet-<01..06>/<position>-<arm>/
│   ├── result.json
│   └── stdout.log
├── admitted-leaves.sha256
└── decision.json
```

Failed attempts remain beside their replacement using an `attempt-N` suffix. No failed attempt is silently overwritten or admitted.

Each `result.json` must conform to `wafer-p3-poc-raw-leaf-v1` and bind:

- a clean full source SHA and experimental evidence class;
- arm, payload/depth condition, triplet, and order position;
- exact queue/session capacity, workers, population, build mode, metering, and timeout;
- executable, component, WIT-tree, and experiment-contract SHA-256 values;
- Rust, Cargo, wasm-tools, and Wasmtime revision;
- separate compile and instantiation nanoseconds;
- measured duration, throughput, p50/p95/p99, peak RSS, loss, and duplicates;
- full-field ordering/session-completion checks; and
- conservative payload-copy bytes and allocation counts.

`stdout.log` is hashed by the batch index. A panic, timeout, mandatory skip, malformed JSON, dirty source, hash mismatch, non-finite number, missing field, wrong control, wrong order, loss, duplication, field mismatch, incomplete session, or unreconciled copy counter rejects the leaf.

## Matching and admission

A triplet is admitted only when all three ordered leaves pass and share the same condition, triplet index, source, artifacts, WIT, payload, depth, queue/session capacity, workers, population, build mode, and metering mode. A rejected leaf rejects the triplet; replacement uses a new attempt suffix without changing condition, triplet index, or arm order.

Every condition requires all six triplets. Arm order follows the six rows in `experiment-contract.json`; each arm appears twice in each position. Conditions and triplets execute serially. No values are pooled across payload or depth.

## Metrics

For one leaf:

```text
throughput_messages_per_second = measured_messages / (duration_ns / 1e9)
lost_messages = measured_messages - unique_output_messages
duplicate_messages = output_messages - unique_output_messages
```

Latency is measured from admission to terminal forwarding for each measured envelope. Setup ends before warmup begins. Compilation and instantiation are reported separately and never included in steady-state throughput or latency.

For one arm/condition, throughput and p50/p95/p99 are medians of the six leaf values. RSS is the maximum of the six leaf peaks. Copy bytes and allocations are summed and separately normalized per message and per hop.

The comparison formulas and thresholds are frozen in the ADR and `experiment-contract.json`. The high-pressure benefit condition is only `100kb-depth5`.

## Decision

`decision.json` must conform to `wafer-p3-poc-decision-v1`. The analyzer applies this precedence exactly once:

1. failed toolchain or production-readiness gate → `defer-p3-toolchain`;
2. all mandatory and performance gates pass → `migrate-wit-before-release`;
3. otherwise → `retain-p2-for-v1`.

A migration result is impossible while `go-p3-toolchain` or `wasmtime-p3-production-readiness` is failed. Missing evidence is a failed gate, not an inconclusive result.

## Production comparator

The P2 arm must use the checked-in production `wafer:pipeline@0.1.0` pass-through component, `RuntimeEnvelope`, capacity-32 Tokio queue, and `WasmTransformNode::process` borrowed-buffer path. A reimplementation of P2 inside the P3 harness is not admissible. Production WIT, source, default features, and artifacts must remain unchanged.

## Counter-metrics

- Passing performance thresholds is paired with exact loss/duplicate/field checks.
- Low latency is paired with the frozen payload/depth matrix and complete population.
- Low RSS is paired with bounded queue/session capacities and no hidden side buffer.
- Passing tests is paired with mandatory no-skip logs and source/artifact hashes.
- Copy counts are paired with element/byte reconciliation, not inferred from timing.
