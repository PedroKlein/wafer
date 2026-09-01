# Why the v11 eKuiper latency tail was misleading

The Raspberry Pi 5 v11 pilot recorded an approximately 18–20 ms eKuiper latency tail. The measurements were internally consistent, but the comparator configuration was not matched: the eKuiper MQTT sink omitted `qos`, selecting QoS 0 instead of the QoS 1 used by WAFER and the native comparator.

This page records the diagnostic result and its claim boundary. It does not present thesis evidence.

## Evidence status

- Diagnostic batch: `rpi5-ekuiper-tail-diagnostic-20260901T004026Z`
- Repository source: `40ab654031052e5fadb133155561d77f29e442c1`
- Experimental unit: one complete process run
- Repetitions: two completed runs per condition
- Warmup: 30 seconds per run
- Measurement: 60 seconds per run at 1,000 messages/second
- Evidence classification: `thesis_evidence=false`
- Retrieved raw-batch manifest: 258 files verified
- Raw-batch manifest SHA-256: `e7e61493466a571ace3765c48540ba94dfe7c0fed664becb2c19706a7b414114`

The fresh batch is separate from `rpi5-pilot-v11-n3-20260831`. The two batches must not be pooled.

## One-variable result

The decisive comparison kept the five-field schema, inclusive range predicate, source QoS 1, MQTT 3.1.1, broker, topics, payload, CPU allocation, warmup, measurement duration, and output shape fixed. It changed only the eKuiper sink QoS.

| Condition | Runs | Median p50 | Median p95 | Median p99 | Periodic signature |
|---|---:|---:|---:|---:|---|
| MQTT loopback | 2 | 0.068 ms | 0.140 ms | 0.152 ms | absent |
| Native Pipeline A | 2 | 0.143 ms | 0.190 ms | 0.234 ms | absent |
| WAFER Pipeline A | 2 | 0.160 ms | 0.245 ms | 0.266 ms | absent |
| eKuiper matched, sink QoS 1 | 2 | 0.179 ms | 0.327 ms | 2.632 ms | absent |
| eKuiper pass-through, sink QoS 1 | 2 | 0.203 ms | 0.385 ms | 2.972 ms | absent |
| eKuiper matched, sink QoS 0 | 2 | 1.324 ms | 17.932 ms | 19.636 ms | present at approximately 20 ms |

All 720,000 completed publisher/subscriber pairs had zero timestamp mutations, negative latencies, missing sequences, and duplicates. Every valid run used CPUs 1–3 for the sole active SUT and CPU 0 for Mosquitto, load generation, and the harness. No run reported throttling.

## What caused the original tail

The QoS 0 traces show repeated long receive gaps followed by tightly spaced releases and descending latency ramps. The two runs had dominant periods of 19.894 ms and 20.229 ms, with lag-20 latency autocorrelation of 0.900 and 0.909.

The same signature was absent from loopback, Native, WAFER, matched eKuiper QoS 1, and eKuiper pass-through QoS 1. The pass-through result also shows that removing the range predicate does not remove the corrected eKuiper upper tail.

The supported conclusion is:

> The v11 approximately 20 ms tail came from the unmatched eKuiper sink QoS 0 configuration and its output-transport path, not from SQL filtering or the measurement harness.

Source inspection localizes the delay after eKuiper's transform and encoding work and after the MQTT client's QoS 0 socket-write completion boundary. The retained evidence cannot distinguish Paho scheduling, kernel buffering, and broker handling within that final transport segment. Packet or syscall timing would be required for a narrower claim.

## Corrected comparison

The corrected comparator explicitly uses:

- eKuiper 2.1.0 on native ARM64 Linux;
- MQTT 3.1.1;
- source and sink QoS 1;
- `retained=false` and `sendSingle=true`;
- the same five telemetry fields as WAFER and Native;
- `50 <= temperature <= 99999`;
- the same broker, topics, payload, load generator, and timing boundary;
- one SUT at a time on CPUs 1–3.

At 1,000 messages/second, five later diagnostic runs per SUT reported median p95/p99 values of 0.196/0.236 ms for Native, 0.238/0.264 ms for WAFER, and 0.330/2.516 ms for corrected eKuiper. At 4,000 messages/second, three diagnostic runs per SUT reported 0.602/0.807 ms, 0.622/0.789 ms, and 5.252/8.802 ms respectively.

These small-N operating-point comparisons show that corrected eKuiper retained a larger upper tail for this workload. They do not establish a general ranking, a saturation-throughput ranking, or a best-tuned eKuiper result.

## Remaining limits

- The corrected runs are diagnostics, not the planned N≥30 evidence.
- A later two-block diagnostic found no material p50/p95, throughput, CPU, or RSS improvement from operator concurrency 3. The comparator freezes the default concurrency 1; the data do not characterize best-tuned eKuiper.
- WAFER and Native capacity were censored by CPU-0 support-plane load before their SUT ceilings were isolated.
- Direct MQTT is a support-path guardrail, not a comparator and not a percentile subtraction term.
- The comparison matches externally observable behavior, not internal feature breadth or implementation complexity.
- A separate five-minute diagnostic found approximately 43 bytes of WAFER anonymous/private-dirty RSS growth per offered message. Long-running resource claims require that behavior to be resolved or explicitly bounded.

## Consequences

1. Do not use the v11 eKuiper result in a comparative ranking.
2. Keep explicit sink QoS 1 in the canonical comparator and its regression test.
3. Keep fixed-load latency separate from sustainable-throughput claims.
4. Use run-level paired inference for confirmatory blocks; message samples are not independent experimental units.
5. Preserve the frozen default concurrency 1 decision and resolve the WAFER memory-retention finding before a definitive thesis comparison.
