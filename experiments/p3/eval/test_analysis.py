#!/usr/bin/env python3
import importlib.util
import unittest
from pathlib import Path

MODULE = Path(__file__).with_name("analyze.py")
SPEC = importlib.util.spec_from_file_location("p3_analysis", MODULE)
analysis = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(analysis)


class AnalysisTests(unittest.TestCase):
    def test_regression_boundaries_are_inclusive(self):
        baseline = {
            "throughput_median_messages_per_second": 100.0,
            "p95_median_ns": 1000.0,
            "peak_rss_max_bytes": 16 * 1024 * 1024,
        }
        candidate = {
            "throughput_median_messages_per_second": 95.0,
            "p95_median_ns": 1050.0,
            "peak_rss_max_bytes": 18 * 1024 * 1024,
        }
        self.assertTrue(analysis.delta(candidate, baseline)["regression_budget_pass"])

    def test_stream_benefit_uses_predeclared_or_rule(self):
        baseline = {
            "throughput_median_messages_per_second": 100.0,
            "p95_median_ns": 1000.0,
            "peak_rss_max_bytes": 16 * 1024 * 1024,
        }
        throughput_win = {
            "throughput_median_messages_per_second": 110.0,
            "p95_median_ns": 1000.0,
            "peak_rss_max_bytes": 16 * 1024 * 1024,
        }
        latency_win = {
            "throughput_median_messages_per_second": 100.0,
            "p95_median_ns": 900.0,
            "peak_rss_max_bytes": 16 * 1024 * 1024,
        }
        self.assertTrue(analysis.delta(throughput_win, baseline, benefit=True)["stream_benefit_pass"])
        self.assertTrue(analysis.delta(latency_win, baseline, benefit=True)["stream_benefit_pass"])

    def test_decision_is_fail_closed_and_exclusive(self):
        base = {
            "rust-p3-toolchain": "pass",
            "go-p3-toolchain": "pass",
            "wasmtime-p3-production-readiness": "pass",
            "lifecycle-conformance": "pass",
        }
        self.assertEqual(analysis.decide(base), "migrate-wit-before-release")
        semantic_failure = dict(base, **{"lifecycle-conformance": "fail"})
        self.assertEqual(analysis.decide(semantic_failure), "retain-p2-for-v1")
        toolchain_failure = dict(base, **{"go-p3-toolchain": "fail"})
        self.assertEqual(analysis.decide(toolchain_failure), "defer-p3-toolchain")


if __name__ == "__main__":
    unittest.main()
