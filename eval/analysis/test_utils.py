"""Tests for eval/analysis/src/wafer_analysis/paths.py."""

from __future__ import annotations

import hashlib
import json
import pathlib

import pytest

from wafer_analysis import paths as utils
from wafer_analysis import results_layout

FINAL_CAMPAIGN = {
    "attempt_policy": {"infrastructure_retries": 1, "gate_experiments": ["e-val-1"]}
}


@pytest.fixture
def fake_results(tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch):
    """Create a minimal repository with one approved canonical E-Val-1 leaf."""
    (tmp_path / "eval" / "results" / "e-val-1").mkdir(parents=True)
    (tmp_path / "eval" / "canonical-matrix.json").write_text(
        json.dumps(
            {
                "final_campaign": FINAL_CAMPAIGN,
                "experiments": {
                    "e-val-1": {
                        "conditions": ["delay-50ms"],
                        "repetitions": 1,
                        "required_outputs": ["percentiles.json"],
                    }
                }
            }
        )
    )
    approve_batch(tmp_path, tmp_path / "eval/results/canonical-batches/rpi5-batch-a")
    monkeypatch.setattr(utils, "_find_repo_root", lambda: tmp_path)
    return tmp_path


def approve_batch(repo: pathlib.Path, ledger: pathlib.Path, host: str = "rpi5") -> None:
    """Write what `approve-batch` leaves behind for batch-a: ledger files and the entry."""
    matrix_sha = hashlib.sha256(
        (repo / "eval/canonical-matrix.json").read_bytes()
    ).hexdigest()
    ledger.mkdir(parents=True, exist_ok=True)
    (ledger / "raw.sha256").write_text(f"{'0' * 64}  raw/fixture\n")
    (ledger / "batch.json").write_text(
        json.dumps({"source_git_sha": "1" * 40, "canonical_matrix_sha256": matrix_sha})
    )
    final_batches = repo / "eval/final-batches.json"
    document = (
        json.loads(final_batches.read_text())
        if final_batches.is_file()
        else {"schema_version": 1, "batches": {}}
    )
    document["batches"][host] = {
        "batch_id": "batch-a",
        "wafer_git_sha": "1" * 40,
        "canonical_matrix_sha256": matrix_sha,
        "raw_manifest_sha256": hashlib.sha256(
            (ledger / "raw.sha256").read_bytes()
        ).hexdigest(),
        "approved_at": "2026-09-29T00:00:00Z",
        "other_final_batches": [],
    }
    final_batches.write_text(json.dumps(document))


def test_implicit_latest_selection_is_disabled(fake_results: pathlib.Path):
    old = fake_results / "eval/results/e-val-1/shakedown-macos-old"
    new = fake_results / "eval/results/e-val-1/shakedown-macos-new"
    old.mkdir()
    new.mkdir()

    with pytest.raises(RuntimeError, match="WAFER_EVAL_BATCH_ID"):
        utils.resolve_result_batch("e-val-1")


def test_explicit_diagnostic_path_resolves_from_repo(fake_results: pathlib.Path):
    relative = "eval/results/e-val-1/diagnostic-batch"
    expected = fake_results / relative
    expected.mkdir(parents=True)
    assert utils.resolve_result_batch("e-val-1", diagnostic_path=relative) == expected


def test_explicit_diagnostic_path_must_exist(fake_results: pathlib.Path):
    with pytest.raises(FileNotFoundError, match="Explicit diagnostic path"):
        utils.resolve_result_batch("e-val-1", diagnostic_path="missing")


def alias_fixture(fake_results: pathlib.Path) -> tuple[pathlib.Path, pathlib.Path, pathlib.Path]:
    volume = fake_results / "mounted results volume"
    source = volume / "raw/e-perf-1/rpi5-batch-a/native/run-01-attempt-01"
    write_canonical_leaf(source)
    metadata_path = source / "metadata.json"
    metadata = json.loads(metadata_path.read_text())
    metadata.update(experiment="e-perf-1", condition="native", evidence_class="final")
    metadata_path.write_text(json.dumps(metadata))
    matrix_path = fake_results / "eval/canonical-matrix.json"
    matrix = json.loads(matrix_path.read_text())
    matrix["experiments"] = {
        "e-perf-1": {
            "conditions": ["native"],
            "repetitions": 1,
            "required_outputs": ["percentiles.json"],
        },
        "e-perf-2": {
            "conditions": ["native"],
            "repetitions": 1,
            "required_outputs": ["percentiles.json"],
        },
    }
    matrix_path.write_text(json.dumps(matrix))
    approve_batch(fake_results, volume / "manifests/canonical-batches/rpi5-batch-a")
    receipt = volume / "manifests/aliases/e-perf-2/rpi5-batch-a/native/run-01.json"
    receipt.parent.mkdir(parents=True)
    source_leaf = "raw/e-perf-1/rpi5-batch-a/native/run-01-attempt-01"
    receipt.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "experiment": "e-perf-2",
                "condition": "native",
                "run_index": 1,
                "shared_from_experiment": "e-perf-1",
                "source_leaf": source_leaf,
                "source_status_sha256": hashlib.sha256(
                    (source / "canonical-status.json").read_bytes()
                ).hexdigest(),
                "sample_identity": source_leaf,
                "source_evidence_class": "final",
                "independent_n_contribution": 0,
                "shared_measurement": True,
            }
        )
    )
    return volume, source, receipt


def test_explicit_results_root_alias_resolves_single_source_batch(
    fake_results: pathlib.Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    volume, source, _ = alias_fixture(fake_results)
    monkeypatch.setattr(results_layout.os.path, "ismount", lambda _: True)

    assert utils.find_canonical_batch("e-perf-2", "batch-a", volume) == source.parents[1]


@pytest.mark.parametrize(
    ("field", "value", "message"),
    [
        ("independent_n_contribution", 1, "zero independent N"),
        ("source_evidence_class", "diagnostic", "canonical mapping"),
        ("sample_identity", "raw/e-perf-1/copied/run-01", "sample identity"),
        ("source_status_sha256", "0" * 64, "receipt digest differs"),
    ],
)
def test_alias_receipt_rejects_inflation_or_source_identity_drift(
    fake_results: pathlib.Path,
    monkeypatch: pytest.MonkeyPatch,
    field: str,
    value: object,
    message: str,
) -> None:
    volume, _, receipt = alias_fixture(fake_results)
    contents = json.loads(receipt.read_text())
    contents[field] = value
    receipt.write_text(json.dumps(contents))
    monkeypatch.setattr(results_layout.os.path, "ismount", lambda _: True)

    with pytest.raises(ValueError, match=message):
        utils.find_canonical_batch("e-perf-2", "batch-a", volume)


def test_alias_receipt_rejects_nonfinal_source_metadata(
    fake_results: pathlib.Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    volume, source, _ = alias_fixture(fake_results)
    metadata_path = source / "metadata.json"
    metadata = json.loads(metadata_path.read_text())
    metadata["evidence_class"] = "diagnostic"
    metadata["thesis_evidence"] = False
    metadata_path.write_text(json.dumps(metadata))
    monkeypatch.setattr(results_layout.os.path, "ismount", lambda _: True)

    with pytest.raises(ValueError, match="non-final evidence"):
        utils.find_canonical_batch("e-perf-2", "batch-a", volume)


def test_alias_receipt_rejects_links(
    fake_results: pathlib.Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    volume, _, receipt = alias_fixture(fake_results)
    monkeypatch.setattr(results_layout.os.path, "ismount", lambda _: True)
    contents = receipt.read_bytes()
    receipt.unlink()
    target = receipt.with_name("target.json")
    target.write_bytes(contents)
    receipt.symlink_to(target.name)
    with pytest.raises(ValueError, match="symlink"):
        utils.find_canonical_batch("e-perf-2", "batch-a", volume)

    receipt.unlink()
    receipt.hardlink_to(target)
    with pytest.raises(ValueError, match="hardlinked"):
        utils.find_canonical_batch("e-perf-2", "batch-a", volume)


def test_explicit_results_root_supports_spaces_and_restricts_analysis_outputs(
    fake_results: pathlib.Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    volume = fake_results / "mounted results volume"
    for name in ("raw", "manifests", "derived", "reports"):
        (volume / name).mkdir(parents=True)
    monkeypatch.setattr(results_layout.os.path, "ismount", lambda _: True)

    diagnostic = volume / "raw/e-val-1/diagnostic"
    diagnostic.mkdir(parents=True)
    assert utils.resolve_result_batch(
        "e-val-1", diagnostic_path="raw/e-val-1/diagnostic", results_root=volume
    ) == diagnostic
    assert utils.resolve_analysis_output(
        "reports", "expanded-n5", "summary.json", results_root=volume
    ) == volume / "reports/expanded-n5/summary.json"
    with pytest.raises(ValueError, match="derived or reports"):
        utils.resolve_analysis_output("raw", "bad.json", results_root=volume)
    with pytest.raises(ValueError, match="raw|outside the results root"):
        utils.resolve_result_batch(
            "e-val-1", diagnostic_path=str(volume / "reports"), results_root=volume
        )


def write_canonical_leaf(
    path: pathlib.Path,
    *,
    sha: str = "1" * 40,
    dirty: bool = False,
    throttled: str = "0x0",
    host: str = "rpi5",
    status: str = "passed",
    run_index: int = 1,
) -> None:
    path.mkdir(parents=True)
    (path / "config.toml").write_text("[pipeline]\nname='fixture'\n")
    (path / "canonical-status.json").write_text(json.dumps({"status": status}))
    (path / "percentiles.json").write_text(
        json.dumps(
            {
                "total_count": 60_000,
                "p50_ns": 1,
                "p95_ns": 2,
                "p99_ns": 3,
                "p999_ns": 4,
            }
        )
    )
    (path / "metadata.json").write_text(
        json.dumps(
            {
                "experiment": "e-val-1",
                "condition": "delay-50ms",
                "run_index": run_index,
                "host_tag": host,
                "git_sha": sha,
                "git_dirty": dirty,
                "git_tags": ["rpi5-eval-v1"],
                "throttled": throttled,
                "thesis_evidence": True,
            }
        )
    )


def write_swap3_canonical_leaf(
    path: pathlib.Path,
    *,
    status: str = "passed",
    run_index: int = 1,
) -> None:
    write_canonical_leaf(path, status=status, run_index=run_index)
    metadata_path = path / "metadata.json"
    metadata = json.loads(metadata_path.read_text())
    metadata.update(experiment="e-swap-3", condition="wafer-hotswap")
    metadata_path.write_text(json.dumps(metadata))
    repo = path.parents[5]
    matrix_path = repo / "eval/canonical-matrix.json"
    matrix_path.write_text(
        json.dumps(
            {
                "final_campaign": FINAL_CAMPAIGN,
                "experiments": {
                    "e-swap-3": {
                        "conditions": ["wafer-hotswap"],
                        "repetitions": 1,
                        "required_outputs": [
                            "latency.hdr",
                            "throughput.csv",
                            "sequence.csv",
                            "publisher-summary.json",
                            "subscriber-metadata.json",
                            "throughput-buckets.json",
                            "throughput-buckets-10ms.json",
                            "disruption-timeline.json",
                            "disruption-analysis.json",
                        ],
                    }
                }
            }
        )
    )
    refresh_approval_matrix_hash(repo)
    for name, content in {
        "latency.hdr": "fixture\n",
        "throughput.csv": "fixture\n",
        "sequence.csv": "fixture\n",
        "publisher-summary.json": json.dumps({"schema_version": 1}),
        "subscriber-metadata.json": json.dumps({"schema_version": 1}),
        "throughput-buckets.json": json.dumps({"schema_version": 1}),
        "throughput-buckets-10ms.json": json.dumps({"schema_version": 1}),
        "disruption-timeline.json": json.dumps({"schema_version": 1}),
        "disruption-analysis.json": json.dumps({"schema_version": 1}),
    }.items():
        (path / name).write_text(content)


def write_swap5_canonical_leaf(path: pathlib.Path) -> None:
    write_canonical_leaf(path)
    metadata_path = path / "metadata.json"
    metadata = json.loads(metadata_path.read_text())
    metadata.update(experiment="e-swap-5", condition="process-trap-rollback")
    metadata_path.write_text(json.dumps(metadata))
    requests = []
    events = []
    for index in range(50):
        timeline = {
            "compile_ns": 1,
            "instantiate_ns": 2,
            "signal_ns": 3,
            "rollback_ns": 4,
        }
        requests.append(
            {
                "event_index": index,
                "plugin": "wafer_pass_through_v2_panics.wasm",
                "request_finished_ns": 1_000 + index * 100,
                "http_status": 200,
                "body": {"status": "rolled_back", "timeline": timeline},
            }
        )
        events.append({"event_index": index, **timeline})
    sequence = {"expected": 1_000, "received": 1_000, "gaps": 0, "duplicates": 0}
    (path / "sequence.csv").write_text(
        "total_expected,total_received,gap_events,gap_msgs,duplicates_count\n"
        "1000,1000,0,0,0\n"
    )
    (path / "swap_requests.json").write_text(json.dumps(requests))
    (path / "rollback.json").write_text(
        json.dumps(
            {
                "schema_version": 1,
                "duration_unit": "ns",
                "independent_unit": "complete process run",
                "nested_unit": "rollback event within run",
                "attempts": 50,
                "rolled_back": 50,
                "all_rolled_back": True,
                "sequence": sequence,
                "events": events,
            }
        )
    )
    final_finished = requests[-1]["request_finished_ns"]
    (path / "post-rollback-continuity.json").write_text(
        json.dumps(
            {
                "schema_version": 1,
                "clock": "unix-epoch",
                "final_rollback_event_index": 49,
                "final_rollback_finished_ns": final_finished,
                "observation_start_ns": final_finished,
                "observation_end_ns": final_finished + 100,
                "messages_after_final_rollback": 10,
                "output_observed_after_final_rollback": True,
                "successful_v2_transition_observed": False,
                "interval_metrics_path": "interval-metrics.json",
                "interval_metrics_sha256": "a" * 64,
                "sequence": sequence,
            }
        )
    )


def refresh_approval_matrix_hash(fake_results: pathlib.Path) -> None:
    approve_batch(fake_results, fake_results / "eval/results/canonical-batches/rpi5-batch-a")


def canonical_batch(fake_results: pathlib.Path, name: str = "batch-a") -> pathlib.Path:
    batch = fake_results / f"eval/results/e-val-1/rpi5-{name}"
    write_canonical_leaf(batch / "delay-50ms/run-01-attempt-01")
    return batch


def test_batch_environment_requires_explicit_canonical_input(
    fake_results: pathlib.Path, monkeypatch: pytest.MonkeyPatch
):
    batch = canonical_batch(fake_results)
    monkeypatch.setenv("WAFER_EVAL_BATCH_ID", "batch-a")
    assert utils.resolve_result_batch("e-val-1") == batch


def test_explicit_canonical_ledger_requires_a_named_existing_batch(
    fake_results: pathlib.Path,
):
    ledger = fake_results / "eval/results/canonical-batches/rpi5-batch-a"
    assert utils.find_canonical_ledger("batch-a") == ledger

    with pytest.raises(
        FileNotFoundError, match="Canonical batch ledger does not exist"
    ):
        utils.find_canonical_ledger("missing")
    with pytest.raises(ValueError, match="unsafe characters"):
        utils.find_canonical_ledger("../batch-a")


def test_canonical_batch_rejects_wrong_host(fake_results: pathlib.Path):
    batch = canonical_batch(fake_results)
    metadata_path = next(batch.rglob("metadata.json"))
    metadata = json.loads(metadata_path.read_text())
    metadata["host_tag"] = "other"
    metadata_path.write_text(json.dumps(metadata))
    with pytest.raises(ValueError, match="non-rpi5 input"):
        utils.validate_canonical_batch(batch, "e-val-1")


def test_canonical_batch_rejects_dirty_source(fake_results: pathlib.Path):
    batch = canonical_batch(fake_results)
    metadata_path = next(batch.rglob("metadata.json"))
    metadata = json.loads(metadata_path.read_text())
    metadata["git_dirty"] = True
    metadata_path.write_text(json.dumps(metadata))
    with pytest.raises(ValueError, match="dirty canonical input"):
        utils.validate_canonical_batch(batch, "e-val-1")


def test_canonical_batch_rejects_mixed_sha(fake_results: pathlib.Path):
    matrix_path = fake_results / "eval/canonical-matrix.json"
    matrix = json.loads(matrix_path.read_text())
    matrix["experiments"]["e-val-1"]["repetitions"] = 2
    matrix_path.write_text(json.dumps(matrix))
    refresh_approval_matrix_hash(fake_results)
    batch = canonical_batch(fake_results)
    write_canonical_leaf(
        batch / "delay-50ms/run-02-attempt-01", sha="2" * 40, run_index=2
    )
    with pytest.raises(ValueError, match="mixed source SHAs"):
        utils.validate_canonical_batch(batch, "e-val-1")


def test_canonical_batch_rejects_throttling(fake_results: pathlib.Path):
    batch = canonical_batch(fake_results)
    metadata_path = next(batch.rglob("metadata.json"))
    metadata = json.loads(metadata_path.read_text())
    metadata["throttled"] = "0x50000"
    metadata_path.write_text(json.dumps(metadata))
    with pytest.raises(ValueError, match="throttled canonical input"):
        utils.validate_canonical_batch(batch, "e-val-1")


def test_canonical_batch_rejects_failed_leaf(fake_results: pathlib.Path):
    batch = canonical_batch(fake_results)
    status_path = next(batch.rglob("canonical-status.json"))
    status_path.write_text('{"status":"failed"}')
    with pytest.raises(ValueError, match="failed canonical input"):
        utils.validate_canonical_batch(batch, "e-val-1")


INFRASTRUCTURE_FAILURE = {
    "status": "failed",
    "failure_class": "infrastructure",
    "reasons": ["harness-error"],
}
RUNTIME_EXIT = {
    "status": "failed",
    "failure_class": "sut_outcome",
    "reasons": ["runtime-exit"],
}


def perf_unit(fake_results: pathlib.Path, *receipts: dict | None) -> pathlib.Path:
    """One E-Perf-5 unit whose attempts end with these receipts; None leaves no receipt."""
    matrix_path = fake_results / "eval/canonical-matrix.json"
    matrix = json.loads(matrix_path.read_text())
    matrix["experiments"] = {"e-perf-5": matrix["experiments"]["e-val-1"]}
    matrix_path.write_text(json.dumps(matrix))
    refresh_approval_matrix_hash(fake_results)
    batch = fake_results / "eval/results/e-perf-5/rpi5-batch-a"
    for number, receipt in enumerate(receipts, start=1):
        leaf = batch / f"delay-50ms/run-01-attempt-{number:02d}"
        write_canonical_leaf(leaf)
        metadata = json.loads((leaf / "metadata.json").read_text())
        metadata["experiment"] = "e-perf-5"
        (leaf / "metadata.json").write_text(json.dumps(metadata))
        if receipt is None:
            (leaf / "canonical-status.json").unlink()
        else:
            (leaf / "canonical-status.json").write_text(json.dumps(receipt))
    return batch


def test_gate_batch_rejects_a_retried_unit(fake_results: pathlib.Path):
    batch = canonical_batch(fake_results)
    next(batch.rglob("canonical-status.json")).write_text('{"status":"failed"}')
    write_canonical_leaf(batch / "delay-50ms/run-01-attempt-02")

    with pytest.raises(ValueError, match="exceeds its retry policy"):
        utils.validate_canonical_batch(batch, "e-val-1")


def test_canonical_batch_admits_one_retry_after_an_infrastructure_failure(
    fake_results: pathlib.Path,
):
    batch = perf_unit(fake_results, INFRASTRUCTURE_FAILURE, {"status": "passed"})

    assert utils.validate_canonical_batch(batch, "e-perf-5") == "1" * 40


def test_canonical_batch_admits_a_retry_after_an_interrupted_attempt(
    fake_results: pathlib.Path,
):
    batch = perf_unit(fake_results, None, {"status": "passed"})

    assert utils.validate_canonical_batch(batch, "e-perf-5") == "1" * 40


def test_canonical_batch_counts_an_interrupted_attempt_against_the_retry_cap(
    fake_results: pathlib.Path,
):
    batch = perf_unit(fake_results, None, INFRASTRUCTURE_FAILURE, {"status": "passed"})

    with pytest.raises(ValueError, match="exceeds its retry policy"):
        utils.validate_canonical_batch(batch, "e-perf-5")


def test_canonical_batch_rejects_a_unit_whose_retry_also_failed(
    fake_results: pathlib.Path,
):
    batch = perf_unit(fake_results, INFRASTRUCTURE_FAILURE, INFRASTRUCTURE_FAILURE)

    with pytest.raises(ValueError, match="failed canonical input"):
        utils.validate_canonical_batch(batch, "e-perf-5")


def test_canonical_batch_admits_a_system_outcome_that_stopped_the_run(
    fake_results: pathlib.Path,
):
    batch = perf_unit(fake_results, RUNTIME_EXIT)
    leaf = batch / "delay-50ms/run-01-attempt-01"
    with pytest.raises(ValueError, match="differs from its outcome evidence"):
        utils.validate_canonical_batch(batch, "e-perf-5")

    metadata = json.loads((leaf / "metadata.json").read_text())
    metadata["exit_codes"] = {"wafer_runtime": 134}
    (leaf / "metadata.json").write_text(json.dumps(metadata))
    (leaf / "percentiles.json").unlink()

    assert utils.validate_canonical_batch(batch, "e-perf-5") == "1" * 40


EKUIPER_RULE_RUNNING = {"status": "running", "message": "", "lastStartTimestamp": 1_000}


def ekuiper_health(after_service: dict, after_rule: dict | None) -> dict:
    """``ekuiper-health.json`` whose snapshot after the run differs from the one before it."""
    unit = {"NRestarts": 0, "ExecMainStatus": 0, "MainPID": 4242}
    return {
        "schema_version": 1,
        "unit": "kuiper.service",
        "rule": "pipeline_a",
        "before": {"captured_at_ns": 1, "service": unit, "rule_status": EKUIPER_RULE_RUNNING},
        "after": {
            "captured_at_ns": 2,
            "service": {**unit, **after_service},
            "rule_status": after_rule,
        },
    }


@pytest.mark.parametrize(
    ("reason", "after_service", "after_rule"),
    [
        ("runtime-exit", {"NRestarts": 1, "MainPID": 4343}, EKUIPER_RULE_RUNNING),
        ("runtime-exit", {"MainPID": 0, "ExecMainStatus": 1}, None),
        ("rule-error", {}, {**EKUIPER_RULE_RUNNING, "status": "stopped by error"}),
        ("rule-error", {}, {**EKUIPER_RULE_RUNNING, "message": "retrying after error: x"}),
    ],
)
def test_canonical_batch_admits_an_ekuiper_failure_as_a_stopped_run(
    fake_results: pathlib.Path, reason: str, after_service: dict, after_rule: dict | None
):
    batch = perf_unit(
        fake_results, {"status": "failed", "failure_class": "sut_outcome", "reasons": [reason]}
    )
    leaf = batch / "delay-50ms/run-01-attempt-01"
    metadata = json.loads((leaf / "metadata.json").read_text())
    metadata.update(system="ekuiper", exit_codes={"ekuiper": 0})
    (leaf / "metadata.json").write_text(json.dumps(metadata))
    (leaf / "percentiles.json").unlink()
    (leaf / "ekuiper-health.json").write_text(
        json.dumps(ekuiper_health({}, EKUIPER_RULE_RUNNING))
    )
    with pytest.raises(ValueError, match="differs from its outcome evidence"):
        utils.validate_canonical_batch(batch, "e-perf-5")

    (leaf / "ekuiper-health.json").write_text(json.dumps(ekuiper_health(after_service, after_rule)))

    assert utils.validate_canonical_batch(batch, "e-perf-5") == "1" * 40


def test_canonical_batch_rejects_a_retry_after_a_system_outcome(
    fake_results: pathlib.Path,
):
    batch = perf_unit(fake_results, RUNTIME_EXIT, {"status": "passed"})

    with pytest.raises(ValueError, match="multiple admitted attempts"):
        utils.validate_canonical_batch(batch, "e-perf-5")


def test_canonical_batch_rejects_multiple_passed_attempts_for_one_run(
    fake_results: pathlib.Path,
):
    batch = canonical_batch(fake_results)
    write_canonical_leaf(batch / "delay-50ms/run-01-attempt-02")

    with pytest.raises(ValueError, match="multiple admitted attempts"):
        utils.validate_canonical_batch(batch, "e-val-1")


def test_canonical_analysis_rejects_semantically_invalid_swap5_artifacts(
    fake_results: pathlib.Path,
) -> None:
    matrix_path = fake_results / "eval/canonical-matrix.json"
    matrix_path.write_text(
        json.dumps(
            {
                "final_campaign": FINAL_CAMPAIGN,
                "experiments": {
                    "e-swap-5": {
                        "conditions": ["process-trap-rollback"],
                        "repetitions": 1,
                        "required_outputs": [
                            "sequence.csv",
                            "swap_requests.json",
                            "rollback.json",
                            "post-rollback-continuity.json",
                        ],
                    }
                }
            }
        )
    )
    refresh_approval_matrix_hash(fake_results)
    batch = fake_results / "eval/results/e-swap-5/rpi5-batch-a"
    leaf = batch / "process-trap-rollback/run-01-attempt-01"
    write_swap5_canonical_leaf(leaf)
    assert utils.validate_canonical_batch(batch, "e-swap-5") == "1" * 40

    rollback_path = leaf / "rollback.json"
    rollback = json.loads(rollback_path.read_text())
    rollback["rolled_back"] = 49
    rollback_path.write_text(json.dumps(rollback))
    with pytest.raises(ValueError, match="rollback evidence does not reconcile"):
        utils.validate_canonical_batch(batch, "e-swap-5")


def test_alias_mapping_rejects_cycles() -> None:
    with pytest.raises(ValueError, match="cycle"):
        results_layout.validate_alias_mapping({"a": "b", "b": "a"})


def test_swap3_canonical_batch_rejects_legacy_swap_timeline(
    fake_results: pathlib.Path,
):
    batch = fake_results / "eval/results/e-swap-3/rpi5-batch-a"
    leaf = batch / "wafer-hotswap/run-01-attempt-01"
    write_swap3_canonical_leaf(leaf)
    (leaf / "swap_timeline.json").write_text('{"schema_version":1}\n')

    with pytest.raises(ValueError, match="legacy swap_timeline.json is forbidden"):
        utils.validate_canonical_batch(batch, "e-swap-3")


def test_swap3_canonical_batch_rejects_retained_publisher_timing(
    fake_results: pathlib.Path,
):
    batch = fake_results / "eval/results/e-swap-3/rpi5-batch-a"
    leaf = batch / "wafer-hotswap/run-01-attempt-01"
    write_swap3_canonical_leaf(leaf)
    (leaf / "publisher-timing.json").write_text('{"event_unix_epoch_ns":1060005000000}\n')

    with pytest.raises(ValueError, match="temporary publisher-timing.json is forbidden"):
        utils.validate_canonical_batch(batch, "e-swap-3")


def test_swap3_canonical_batch_rejects_multiple_passed_attempts_for_one_run(
    fake_results: pathlib.Path,
):
    batch = fake_results / "eval/results/e-swap-3/rpi5-batch-a"
    write_swap3_canonical_leaf(batch / "wafer-hotswap/run-01-attempt-01")
    write_swap3_canonical_leaf(batch / "wafer-hotswap/run-01-attempt-02")

    with pytest.raises(ValueError, match="multiple admitted attempts"):
        utils.validate_canonical_batch(batch, "e-swap-3")


def test_canonical_batch_rejects_malformed_schema(fake_results: pathlib.Path):
    batch = canonical_batch(fake_results)
    metadata_path = next(batch.rglob("metadata.json"))
    metadata_path.write_text("[]")
    with pytest.raises(TypeError, match="malformed metadata schema"):
        utils.validate_canonical_batch(batch, "e-val-1")


def test_canonical_batch_rejects_service_percentiles_without_numeric_fields(
    fake_results: pathlib.Path,
):
    matrix_path = fake_results / "eval/canonical-matrix.json"
    matrix = json.loads(matrix_path.read_text())
    matrix["experiments"]["e-val-1"]["required_outputs"].append("service-percentiles.json")
    matrix_path.write_text(json.dumps(matrix))
    refresh_approval_matrix_hash(fake_results)
    batch = canonical_batch(fake_results)
    leaf = batch / "delay-50ms/run-01-attempt-01"
    (leaf / "service-percentiles.json").write_text(json.dumps({"total_count": 60_000, "p50_ns": "1"}))
    with pytest.raises(ValueError, match="malformed service-percentiles.json"):
        utils.validate_canonical_batch(batch, "e-val-1")


def test_canonical_batch_rejects_incomplete_n(fake_results: pathlib.Path):
    matrix_path = fake_results / "eval/canonical-matrix.json"
    matrix = json.loads(matrix_path.read_text())
    matrix["experiments"]["e-val-1"]["repetitions"] = 2
    matrix_path.write_text(json.dumps(matrix))
    refresh_approval_matrix_hash(fake_results)
    batch = canonical_batch(fake_results)
    with pytest.raises(ValueError, match="incomplete canonical batch"):
        utils.validate_canonical_batch(batch, "e-val-1")


def test_canonical_batch_rejects_unapproved_batch(fake_results: pathlib.Path):
    batch = canonical_batch(fake_results, "batch-b")
    with pytest.raises(ValueError, match="unapproved canonical batch"):
        utils.validate_canonical_batch(batch, "e-val-1")


def test_canonical_batch_requires_a_final_batches_entry(fake_results: pathlib.Path):
    batch = canonical_batch(fake_results)
    (fake_results / "eval/final-batches.json").unlink()
    with pytest.raises(
        ValueError,
        match="run `mise run approve-batch -- --batch-id batch-a --host rpi5`",
    ):
        utils.validate_canonical_batch(batch, "e-val-1")


def test_diagnostic_path_does_not_consume_approval(fake_results: pathlib.Path):
    diagnostic = fake_results / "eval/results/e-val-1/diagnostic"
    diagnostic.mkdir(parents=True)
    (fake_results / "eval/final-batches.json").unlink()
    assert utils.resolve_analysis_batch("e-val-1", diagnostic_path=str(diagnostic)) == (
        diagnostic,
        False,
    )


def test_canonical_batch_rejects_a_malformed_final_batches_file(fake_results: pathlib.Path):
    batch = canonical_batch(fake_results)
    final_batches = fake_results / "eval/final-batches.json"
    document = json.loads(final_batches.read_text())
    del document["schema_version"]
    final_batches.write_text(json.dumps(document))
    with pytest.raises(ValueError, match="unapproved canonical batch"):
        utils.validate_canonical_batch(batch, "e-val-1")


def test_canonical_batch_accepts_an_untagged_clean_source(fake_results: pathlib.Path):
    batch = canonical_batch(fake_results)
    metadata_path = next(batch.rglob("metadata.json"))
    metadata = json.loads(metadata_path.read_text())
    metadata["git_tags"] = []
    metadata_path.write_text(json.dumps(metadata))
    assert utils.validate_canonical_batch(batch, "e-val-1") == "1" * 40


@pytest.mark.parametrize(
    ("field", "value", "message"),
    [
        ("canonical_matrix_sha256", "0" * 64, "differs from the approved matrix"),
        ("wafer_git_sha", "2" * 40, "batch.json differs from its eval/final-batches.json entry"),
        ("batch_id", "batch-b", "approves rpi5-batch-b"),
    ],
)
def test_canonical_batch_rejects_final_batch_entry_drift(
    fake_results: pathlib.Path,
    field: str,
    value: str,
    message: str,
):
    batch = canonical_batch(fake_results)
    final_batches = fake_results / "eval/final-batches.json"
    document = json.loads(final_batches.read_text())
    document["batches"]["rpi5"][field] = value
    final_batches.write_text(json.dumps(document))
    with pytest.raises(ValueError, match=message):
        utils.validate_canonical_batch(batch, "e-val-1")


def test_canonical_batch_rejects_a_changed_raw_manifest(fake_results: pathlib.Path):
    batch = canonical_batch(fake_results)
    assert utils.validate_canonical_batch(batch, "e-val-1") == "1" * 40
    manifest = fake_results / "eval/results/canonical-batches/rpi5-batch-a/raw.sha256"
    manifest.write_text(f"{'f' * 64}  raw/fixture\n")
    with pytest.raises(ValueError, match="raw.sha256 differs from the approved"):
        utils.validate_canonical_batch(batch, "e-val-1")


def test_canonical_batch_rejects_missing_or_malformed_artifact(
    fake_results: pathlib.Path,
):
    batch = canonical_batch(fake_results)
    artifact = next(batch.rglob("percentiles.json"))
    artifact.write_text("{}")
    with pytest.raises(ValueError, match="malformed percentiles.json"):
        utils.validate_canonical_batch(batch, "e-val-1")


def test_canonical_batch_rejects_amended_artifact_schema_drift(
    fake_results: pathlib.Path,
):
    matrix_path = fake_results / "eval/canonical-matrix.json"
    matrix = json.loads(matrix_path.read_text())
    matrix["experiments"]["e-val-1"]["required_outputs"].append(
        "throughput-buckets.json"
    )
    matrix_path.write_text(json.dumps(matrix))
    refresh_approval_matrix_hash(fake_results)
    batch = canonical_batch(fake_results)
    next(batch.rglob("metadata.json")).parent.joinpath(
        "throughput-buckets.json"
    ).write_text("{}")
    with pytest.raises(ValueError, match="schema_version must be 1"):
        utils.validate_canonical_batch(batch, "e-val-1")


def test_diagnostic_artifacts_are_always_non_thesis_pending_compatible(
    fake_results: pathlib.Path,
):
    diagnostic = fake_results / "eval/results/e-val-1/diagnostic"
    diagnostic.mkdir(parents=True)
    (diagnostic / "metadata.json").write_text('{"thesis_evidence":true}')
    resolved = utils.resolve_result_batch("e-val-1", diagnostic_path=str(diagnostic))
    assert resolved == diagnostic
    assert utils.analysis_evidence_status(diagnostic, canonical=False) == {
        "mode": "diagnostic",
        "thesis_evidence": False,
        "uncertainty": "descriptive only",
    }


def test_notebooks_never_select_latest_results_implicitly() -> None:
    notebook_dir = pathlib.Path(__file__).parent / "notebooks"
    forbidden = ("find_latest_shakedown", "newest shakedown", "SHAKEDOWN_DIR")
    for path in notebook_dir.glob("*.ipynb"):
        source = "".join(
            "".join(cell.get("source", []))
            for cell in json.loads(path.read_text())["cells"]
        )
        assert not any(value in source for value in forbidden), path.name
        assert (
            "resolve_result_batch" in source
            or "resolve_analysis_batch" in source
            or "WAFER_EVAL_BATCH_ID" in source
        ), path.name
        assert "except (FileNotFoundError" not in source, path.name


def test_cross_architecture_requires_x86_batch(fake_results: pathlib.Path):
    pi_batch = fake_results / "eval/results/e-perf-5/rpi5-batch"
    write_canonical_leaf(pi_batch / "wafer/run-01-attempt-01")
    with pytest.raises(ValueError, match="incomplete until matching x86 Linux"):
        utils.require_cross_architecture(pi_batch, None)



def write_host_approval(repo: pathlib.Path, host: str, **changes: object) -> None:
    approve_batch(repo, repo / f"eval/results/canonical-batches/{host}-batch-a", host)
    final_batches = repo / "eval/final-batches.json"
    document = json.loads(final_batches.read_text())
    document["batches"][host].update(changes)
    final_batches.write_text(json.dumps(document))


def test_each_host_batch_is_validated_against_its_own_approval(
    fake_results: pathlib.Path,
):
    batch = fake_results / "eval/results/e-val-1/jetson-batch-a"
    write_canonical_leaf(batch / "delay-50ms/run-01-attempt-01", host="jetson")
    with pytest.raises(ValueError, match="has no jetson entry"):
        utils.validate_canonical_batch(batch, "e-val-1")

    write_host_approval(fake_results, "jetson")
    assert utils.validate_canonical_batch(batch, "e-val-1") == "1" * 40
    assert utils.find_canonical_batch("e-val-1", "jetson-batch-a") == batch

    write_host_approval(fake_results, "jetson", batch_id="batch-b")
    with pytest.raises(ValueError, match="unapproved canonical batch"):
        utils.validate_canonical_batch(batch, "e-val-1")


def test_host_batch_rejects_leaves_from_another_host(fake_results: pathlib.Path):
    write_host_approval(fake_results, "x86")
    batch = fake_results / "eval/results/e-val-1/x86-batch-a"
    write_canonical_leaf(batch / "delay-50ms/run-01-attempt-01", host="rpi5")
    with pytest.raises(ValueError, match="non-x86 input"):
        utils.validate_canonical_batch(batch, "e-val-1")


def test_canonical_batch_needs_a_known_host_prefix(fake_results: pathlib.Path):
    batch = fake_results / "eval/results/e-val-1/laptop-batch-a"
    write_canonical_leaf(batch / "delay-50ms/run-01-attempt-01", host="laptop")
    with pytest.raises(ValueError, match="no known host prefix"):
        utils.validate_canonical_batch(batch, "e-val-1")


def test_cross_architecture_validates_each_side_against_its_own_host(
    fake_results: pathlib.Path,
):
    fake_results.joinpath("eval/canonical-matrix.json").write_text(
        json.dumps(
            {
                "final_campaign": FINAL_CAMPAIGN,
                "experiments": {
                    "e-perf-5": {
                        "conditions": ["delay-50ms"],
                        "repetitions": 1,
                        "required_outputs": ["percentiles.json"],
                    }
                }
            }
        )
    )
    refresh_approval_matrix_hash(fake_results)
    write_host_approval(fake_results, "x86")
    pi_batch = fake_results / "eval/results/e-perf-5/rpi5-batch-a"
    x86_batch = fake_results / "eval/results/e-perf-5/x86-batch-a"
    for batch, host in ((pi_batch, "rpi5"), (x86_batch, "x86")):
        leaf = batch / "delay-50ms/run-01-attempt-01"
        write_canonical_leaf(leaf, host=host)
        metadata = json.loads((leaf / "metadata.json").read_text())
        metadata["experiment"] = "e-perf-5"
        (leaf / "metadata.json").write_text(json.dumps(metadata))
    utils.require_cross_architecture(pi_batch, x86_batch)
    with pytest.raises(ValueError, match="compares an rpi5 batch with an x86 batch"):
        utils.require_cross_architecture(x86_batch, pi_batch)


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
