#!/usr/bin/env python3

import csv
import importlib.util
import json
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]


def load_script(name: str):
    path = REPO_ROOT / "eval/scripts" / name
    spec = importlib.util.spec_from_file_location(name.removesuffix(".py"), path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


writer = load_script("write-p2-ab-manifest.py")
analyzer = load_script("analyze-p2-ab.py")


def write_csv(path: Path, fieldnames: list[str], rows: list[dict[str, int | float]]) -> None:
    with path.open("w", newline="") as stream:
        output = csv.DictWriter(stream, fieldnames=fieldnames)
        output.writeheader()
        output.writerows(rows)


def valid_leaf(
    root: Path,
    *,
    arm: str = "baseline",
    condition: str = "1kb",
    pair: int = 1,
    payload_bytes: int = 1024,
    git_sha: str = "c" * 40,
    started_at: str = "2026-01-01T00:00:00Z",
    p50_ns: int = 10,
    p95_ns: int = 20,
    p99_ns: int = 30,
    peak_rss_bytes: int = 110_000_000,
) -> Path:
    leaf = root / arm / condition / f"pair-{pair:02d}"
    leaf.mkdir(parents=True)
    config = """
[engine]
epoch_deadline = 100
epoch_tick_ms = 10

[engine.fuel]
transform = 10000000
filter = 500000
router = 500000

[nodes.source]
type = "source"
kind = "bench-source"
rate = 1000.0
total_messages = 90000
warmup_messages = 30000
payload_size = {payload_bytes}

[nodes.sink]
type = "sink"
kind = "bench-sink"
warmup_secs = 30
""".format(payload_bytes=payload_bytes).strip() + "\n"
    (leaf / "config.toml").write_text(config)
    config_hash = writer._sha256(leaf / "config.toml")
    provenance = {
        "config_sha256": config_hash,
        "wafer_runtime_sha256": "a" * 64,
        "wafer_plugin_hashes": {"transform": "b" * 64},
    }
    metadata = {
        **provenance,
        "git_sha": git_sha,
        "git_dirty": False,
        "host_tag": "shakedown-macos",
        "exit_codes": {"wafer_runtime": 0},
        "wasmtime_version": "48.0.2",
        "effective_metering_mode": "fuel-and-epoch",
        "started_at": started_at,
    }
    (leaf / "metadata.json").write_text(json.dumps(metadata))
    (leaf / "runtime-provenance.json").write_text(json.dumps(provenance))
    (leaf / "latency.hdr").write_text("fixture\n")
    (leaf / "latency-summary.json").write_text(
        json.dumps({"total_count": 60_000, "p50_ns": p50_ns, "p95_ns": p95_ns, "p99_ns": p99_ns})
    )
    write_csv(
        leaf / "throughput.csv",
        ["elapsed_secs", "msg_count", "bytes"],
        [
            {"elapsed_secs": index + 1, "msg_count": 1000, "bytes": 1024000}
            for index in range(60)
        ],
    )
    write_csv(
        leaf / "memory.csv",
        ["elapsed_ms", "rss_bytes"],
        [
            {"elapsed_ms": 30_000, "rss_bytes": peak_rss_bytes - 10_000_000},
            {"elapsed_ms": 60_000, "rss_bytes": peak_rss_bytes},
        ],
    )
    (leaf / "memory-clock.json").write_text(json.dumps({"start_unix_epoch_ns": 1_000_000_000}))
    (leaf / "measurement-window.json").write_text(
        json.dumps({"started_ns": 31_000_000_000, "finished_ns": 91_000_000_000})
    )
    write_csv(
        leaf / "sequence.csv",
        ["total_expected", "total_received", "gap_ranges", "gap_msgs", "duplicates_count"],
        [{"total_expected": 60_000, "total_received": 60_000, "gap_ranges": 0, "gap_msgs": 0, "duplicates_count": 0}],
    )
    (leaf / "stdout.log").write_text("pipeline complete\n")
    return leaf


def test_run_validation_accepts_complete_lossless_leaf() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        metrics = writer.validate_run_outputs(valid_leaf(Path(tmp)), "c" * 40, 1024)
    assert metrics == {
        "measurement_duration_ns": 60_000_000_000,
        "measured_messages": 60_000,
        "peak_rss_bytes": 110_000_000,
    }


def test_run_validation_rejects_sequence_loss() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        leaf = valid_leaf(Path(tmp))
        write_csv(
            leaf / "sequence.csv",
            ["total_expected", "total_received", "gap_ranges", "gap_msgs", "duplicates_count"],
            [{"total_expected": 60_000, "total_received": 59_999, "gap_ranges": 1, "gap_msgs": 1, "duplicates_count": 0}],
        )
        try:
            writer.validate_run_outputs(leaf, "c" * 40, 1024)
        except ValueError as error:
            assert "sequence.csv is not lossless" in str(error)
        else:
            raise AssertionError("lossy sequence was accepted")


def test_metric_delta_uses_baseline_denominator() -> None:
    assert analyzer._percent_delta(95.0, 100.0) == -5.0
    assert analyzer._percent_delta(105.0, 100.0) == 5.0


def test_complete_matched_evidence_adopts_within_budget() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        parent = Path(tmp)
        root = parent / "p2-ab"
        baseline_sha = "b" * 40
        candidate_sha = "c" * 40
        factors = writer._controlled_factors(120)
        factors.pop("payload_bytes")
        batch = {
            "schema": writer.BATCH_SCHEMA,
            "arms": {
                "baseline": {"git_sha": baseline_sha, "git_dirty": False},
                "candidate": {"git_sha": candidate_sha, "git_dirty": False},
            },
            "wasmtime_revision": writer.WASMTIME_REVISION,
            "controlled_factors": factors,
            "conditions": writer.CONDITIONS,
            "pairs_per_condition": 1,
            "artifacts": {
                arm: {
                    "wafer_runtime_sha256": f"{index}" * 64,
                    "pass_through_wasm_sha256": "d" * 64,
                }
                for index, arm in enumerate(("baseline", "candidate"), start=1)
            },
        }
        root.mkdir()
        (root / "batch-manifest.json").write_text(json.dumps(batch))
        candidate_source = root / "worktrees/candidate/crates/wafer-core/src"
        (candidate_source / "node").mkdir(parents=True)
        (candidate_source / "engine.rs").write_text(
            "p2::add_to_linker_async\n" + "require_store_data_send: true\n" * 4
        )
        (candidate_source / "node/mod.rs").write_text("struct DirectStoreOwner;\n")
        (parent / "p2-correctness-receipt.md").write_text(
            f"- Git SHA: `{candidate_sha}`\n- Result: **PASS**\n"
        )
        for condition, payload_bytes in writer.CONDITIONS.items():
            for arm, sha, started, latency, rss in (
                ("baseline", baseline_sha, "2026-01-01T00:00:00Z", 100, 100_000_000),
                ("candidate", candidate_sha, "2026-01-01T00:01:31Z", 104, 102_000_000),
            ):
                leaf = valid_leaf(
                    root,
                    arm=arm,
                    condition=condition,
                    payload_bytes=payload_bytes,
                    git_sha=sha,
                    started_at=started,
                    p50_ns=latency // 2,
                    p95_ns=latency,
                    p99_ns=latency * 2,
                    peak_rss_bytes=rss,
                )
                hashes = {name: writer._sha256(leaf / name) for name in analyzer.RUN_FILES}
                hashes["wafer-runtime"] = batch["artifacts"][arm]["wafer_runtime_sha256"]
                hashes["pass-through.wasm"] = batch["artifacts"][arm]["pass_through_wasm_sha256"]
                (leaf / "p2-ab-manifest.json").write_text(
                    json.dumps({
                        "schema": writer.SCHEMA,
                        "arm": arm,
                        "condition": condition,
                        "pair_index": 1,
                        "git_sha": sha,
                        "git_dirty": False,
                        "wasmtime_revision": writer.WASMTIME_REVISION,
                        "controlled_factors": writer._controlled_factors(payload_bytes),
                        "sha256": hashes,
                    })
                )
        output = root / "p2-decision.json"
        decision = analyzer.analyze(root, 1, output)
    assert decision["decision"] == "adopt-async-p2"
    assert decision["correctness"]["status"] == "pass"
    assert decision["strict_simplicity"]["status"] == "pass"
    assert decision["performance"]["status"] == "pass"


def test_failed_analysis_writes_fail_closed_decision() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        output = root / "p2-decision.json"
        try:
            analyzer.analyze(root, 10, output)
        except OSError:
            pass
        else:
            raise AssertionError("missing batch evidence was accepted")
        decision = json.loads(output.read_text())
    assert decision["schema"] == "wafer-p2-async-decision-v1"
    assert decision["analysis_status"] == "failed"
    assert decision["decision"] == "retain-sync-p2"


if __name__ == "__main__":
    test_run_validation_accepts_complete_lossless_leaf()
    test_run_validation_rejects_sequence_loss()
    test_metric_delta_uses_baseline_denominator()
    test_complete_matched_evidence_adopts_within_budget()
    test_failed_analysis_writes_fail_closed_decision()
    print("P2 A/B manifest and analysis tests: PASS")
