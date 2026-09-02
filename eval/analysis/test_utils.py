"""Tests for eval/analysis/src/wafer_analysis/paths.py."""

from __future__ import annotations

import json
import pathlib

import pytest

from wafer_analysis import paths as utils


@pytest.fixture
def fake_results(tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch):
    """Create a fake repo layout under tmp_path with eval/results/."""
    (tmp_path / "eval" / "results" / "e-val-1").mkdir(parents=True)
    # Patch _find_repo_root to return our fake repo
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


def write_canonical_leaf(path: pathlib.Path, *, sha: str = "1" * 40, dirty: bool = False, throttled: str = "0x0") -> None:
    path.mkdir(parents=True)
    (path / "config.toml").write_text("[pipeline]\nname='fixture'\n")
    (path / "canonical-status.json").write_text('{"status":"passed"}')
    (path / "metadata.json").write_text(
        json.dumps(
            {
                "host_tag": "rpi5",
                "git_sha": sha,
                "git_dirty": dirty,
                "git_tags": ["rpi5-eval-v1"],
                "throttled": throttled,
            }
        )
    )


def test_batch_environment_requires_explicit_canonical_input(
    fake_results: pathlib.Path, monkeypatch: pytest.MonkeyPatch
):
    batch = fake_results / "eval/results/e-val-1/rpi5-env-batch"
    write_canonical_leaf(batch / "delay/run-01-attempt-01")
    monkeypatch.setenv("WAFER_EVAL_BATCH_ID", "env-batch")
    assert utils.resolve_result_batch("e-val-1") == batch


def test_explicit_canonical_batch_rejects_bad_provenance(fake_results: pathlib.Path):
    batch = fake_results / "eval/results/e-val-1/rpi5-batch-a"
    leaf = batch / "delay-50ms/run-01-attempt-01"
    write_canonical_leaf(leaf)
    assert utils.find_canonical_batch("e-val-1", "batch-a") == batch

    (leaf / "canonical-status.json").unlink()
    with pytest.raises(ValueError, match="lacks completion receipt"):
        utils.find_canonical_batch("e-val-1", "batch-a")
    (leaf / "canonical-status.json").write_text('{"status":"passed"}')

    metadata = json.loads((leaf / "metadata.json").read_text())
    metadata["git_dirty"] = True
    (leaf / "metadata.json").write_text(json.dumps(metadata))
    with pytest.raises(ValueError, match="dirty canonical input"):
        utils.find_canonical_batch("e-val-1", "batch-a")


def test_canonical_batch_rejects_mixed_sha_and_throttling(fake_results: pathlib.Path):
    batch = fake_results / "eval/results/e-val-1/rpi5-batch-b"
    write_canonical_leaf(batch / "delay/run-01-attempt-01", sha="1" * 40)
    write_canonical_leaf(batch / "delay/run-02-attempt-01", sha="2" * 40)
    with pytest.raises(ValueError, match="mixed source SHAs"):
        utils.validate_canonical_batch(batch)

    second = batch / "delay/run-02-attempt-01/metadata.json"
    metadata = json.loads(second.read_text())
    metadata["git_sha"] = "1" * 40
    metadata["throttled"] = "0x50000"
    second.write_text(json.dumps(metadata))
    with pytest.raises(ValueError, match="throttled canonical input"):
        utils.validate_canonical_batch(batch)


def test_notebooks_never_select_latest_results_implicitly() -> None:
    notebook_dir = pathlib.Path(__file__).parent / "notebooks"
    forbidden = ("find_latest_shakedown", "newest shakedown", "SHAKEDOWN_DIR")
    for path in notebook_dir.glob("*.ipynb"):
        source = "".join(
            "".join(cell.get("source", []))
            for cell in json.loads(path.read_text())["cells"]
        )
        assert not any(value in source for value in forbidden), path.name
        assert "resolve_result_batch" in source or "WAFER_EVAL_BATCH_ID" in source, path.name


def test_cross_architecture_requires_x86_batch(fake_results: pathlib.Path):
    pi_batch = fake_results / "eval/results/e-perf-5/rpi5-batch"
    write_canonical_leaf(pi_batch / "wafer/run-01-attempt-01")
    with pytest.raises(ValueError, match="incomplete until matching x86 Linux"):
        utils.require_cross_architecture(pi_batch, None)


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
