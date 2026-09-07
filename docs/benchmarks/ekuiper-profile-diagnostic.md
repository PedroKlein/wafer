# eKuiper tail-profiling diagnostic

This diagnostic pairs five externally profiled and five unprofiled eKuiper runs at each of 1,000, 4,000, and 8,000 messages per second. It uses the canonical eKuiper 2.1.0 Pipeline A configuration, QoS 1 input/output, one operator worker, 30 seconds of warmup, and 60 seconds of measurement.

The profiled arm samples the eKuiper process tree through Linux `/proc` once per second. The unprofiled control does not start that sampler. Both arms retain the normal bounded latency, throughput, Pi telemetry, and interval outputs. The profiler availability receipt records whether the required `/proc` files were readable before measurement. If they are unavailable, the run continues with process metrics explicitly unavailable.

eKuiper 2.1.0 exposes Prometheus and resource-profiling configuration, but the frozen deployment has no validated GC event stream. `ekuiper-runtime-summary.json` therefore records GC/runtime event metrics as unavailable rather than inferring GC events from latency or RSS. The study reports run-level associations between the profiled process summaries and latency intervals. It does not claim that GC caused any tail event.

These runs are diagnostic only: `thesis_evidence=false`, `n30_admitted=false`, and they are never pooled with E-Perf-1, E-Perf-10, or prior diagnostic rehearsals. Profiling remains disabled for all canonical WAFER, Native, and eKuiper runs.
