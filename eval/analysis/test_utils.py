"""Tests for eval/analysis/src/wafer_analysis/paths.py."""

from __future__ import annotations

import hashlib
import json
import pathlib

import pytest

from wafer_analysis import paths as utils
from wafer_analysis import results_layout


@pytest.fixture
def fake_results(tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch):
    """Create a minimal repository with one approved canonical E-Val-1 leaf."""
    (tmp_path / "eval" / "results" / "e-val-1").mkdir(parents=True)
    (tmp_path / "eval" / "canonical-matrix.json").write_text(
        json.dumps(
            {
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
    approval = (
        tmp_path / ".plans/rpi5-final-experiment-readiness/full-run-approval.json"
    )
    approval.parent.mkdir(parents=True)
    matrix_sha = hashlib.sha256(
        (tmp_path / "eval/canonical-matrix.json").read_bytes()
    ).hexdigest()
    approval.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "decision": "APPROVE",
                "batch_id": "batch-a",
                "wafer_git_sha": "1" * 40,
                "canonical_matrix_sha256": matrix_sha,
                "campaign_started": False,
            }
        )
    )
    monkeypatch.setattr(utils, "_find_repo_root", lambda: tmp_path)
    return tmp_path


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


def test_explicit_results_root_alias_resolves_single_source_batch(
    fake_results: pathlib.Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    volume = fake_results / "mounted results volume"
    source = volume / "raw/e-perf-1/rpi5-batch-a/native/run-01-attempt-01"
    write_canonical_leaf(source)
    metadata_path = source / "metadata.json"
    metadata = json.loads(metadata_path.read_text())
    metadata.update(experiment="e-perf-1", condition="native")
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
    refresh_approval_matrix_hash(fake_results)
    receipt = volume / "manifests/aliases/e-perf-2/rpi5-batch-a/native/run-01.json"
    receipt.parent.mkdir(parents=True)
    status_digest = hashlib.sha256((source / "canonical-status.json").read_bytes()).hexdigest()
    receipt.write_text(json.dumps({
        "schema_version": 1,
        "experiment": "e-perf-2",
        "condition": "native",
        "run_index": 1,
        "shared_from_experiment": "e-perf-1",
        "source_leaf": "raw/e-perf-1/rpi5-batch-a/native/run-01-attempt-01",
        "source_status_sha256": status_digest,
        "sample_identity": "raw/e-perf-1/rpi5-batch-a/native/run-01-attempt-01",
        "shared_measurement": True,
    }))
    monkeypatch.setattr(results_layout.os.path, "ismount", lambda _: True)

    assert utils.find_canonical_batch("e-perf-2", "batch-a", volume) == source.parents[1]


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


def refresh_approval_matrix_hash(fake_results: pathlib.Path) -> None:
    approval_path = (
        fake_results / ".plans/rpi5-final-experiment-readiness/full-run-approval.json"
    )
    approval = json.loads(approval_path.read_text())
    approval["canonical_matrix_sha256"] = hashlib.sha256(
        (fake_results / "eval/canonical-matrix.json").read_bytes()
    ).hexdigest()
    approval_path.write_text(json.dumps(approval))


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
    ledger.mkdir(parents=True)
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


def test_canonical_batch_selects_one_passed_retry_without_pooling_failed_attempt(
    fake_results: pathlib.Path,
):
    batch = canonical_batch(fake_results)
    next(batch.rglob("canonical-status.json")).write_text('{"status":"failed"}')
    write_canonical_leaf(batch / "delay-50ms/run-01-attempt-02")

    assert utils.validate_canonical_batch(batch, "e-val-1") == "1" * 40


def test_canonical_batch_rejects_multiple_passed_attempts_for_one_run(
    fake_results: pathlib.Path,
):
    batch = canonical_batch(fake_results)
    write_canonical_leaf(batch / "delay-50ms/run-01-attempt-02")

    with pytest.raises(ValueError, match="duplicate canonical run"):
        utils.validate_canonical_batch(batch, "e-val-1")


def test_canonical_batch_rejects_malformed_schema(fake_results: pathlib.Path):
    batch = canonical_batch(fake_results)
    metadata_path = next(batch.rglob("metadata.json"))
    metadata_path.write_text("[]")
    with pytest.raises(TypeError, match="malformed metadata schema"):
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


def test_canonical_batch_requires_an_approval_receipt(fake_results: pathlib.Path):
    batch = canonical_batch(fake_results)
    (
        fake_results / ".plans/rpi5-final-experiment-readiness/full-run-approval.json"
    ).unlink()
    with pytest.raises(ValueError, match="unapproved canonical batch"):
        utils.validate_canonical_batch(batch, "e-val-1")


def test_explicit_approval_receipt_takes_precedence(
    fake_results: pathlib.Path,
    monkeypatch: pytest.MonkeyPatch,
):
    batch = canonical_batch(fake_results)
    fallback = (
        fake_results / ".plans/rpi5-final-experiment-readiness/full-run-approval.json"
    )
    explicit = fake_results / "approval.json"
    explicit.write_bytes(fallback.read_bytes())
    fallback.unlink()
    monkeypatch.setenv("WAFER_FULL_RUN_APPROVAL", str(explicit))
    assert utils.validate_canonical_batch(batch, "e-val-1") == "1" * 40


def test_diagnostic_path_does_not_consume_approval(
    fake_results: pathlib.Path,
    monkeypatch: pytest.MonkeyPatch,
):
    diagnostic = fake_results / "eval/results/e-val-1/diagnostic"
    diagnostic.mkdir(parents=True)
    monkeypatch.setenv("WAFER_FULL_RUN_APPROVAL", str(fake_results / "missing.json"))
    assert utils.resolve_analysis_batch("e-val-1", diagnostic_path=str(diagnostic)) == (
        diagnostic,
        False,
    )


def test_canonical_batch_rejects_invalid_minimal_approval(fake_results: pathlib.Path):
    batch = canonical_batch(fake_results)
    approval_path = (
        fake_results / ".plans/rpi5-final-experiment-readiness/full-run-approval.json"
    )
    approval = json.loads(approval_path.read_text())
    del approval["schema_version"]
    approval_path.write_text(json.dumps(approval))
    with pytest.raises(ValueError, match="unapproved canonical batch"):
        utils.validate_canonical_batch(batch, "e-val-1")


@pytest.mark.parametrize(
    ("field", "value", "message"),
    [
        ("canonical_matrix_sha256", "0" * 64, "unapproved canonical batch"),
        ("wafer_git_sha", "2" * 40, "canonical input SHA differs from approval"),
        ("batch_id", "batch-b", "unapproved canonical batch"),
    ],
)
def test_canonical_batch_rejects_approval_identifier_drift(
    fake_results: pathlib.Path,
    field: str,
    value: str,
    message: str,
):
    batch = canonical_batch(fake_results)
    approval_path = (
        fake_results / ".plans/rpi5-final-experiment-readiness/full-run-approval.json"
    )
    approval = json.loads(approval_path.read_text())
    approval[field] = value
    approval_path.write_text(json.dumps(approval))
    with pytest.raises(ValueError, match=message):
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


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
