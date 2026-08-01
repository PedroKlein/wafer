"""Tests for eval/analysis/utils.py path-discovery helper."""

from __future__ import annotations

import pathlib
import sys

import pytest

# Ensure utils.py is importable from any working directory.
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import utils  # noqa: E402


@pytest.fixture
def fake_results(tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch):
    """Create a fake repo layout under tmp_path with eval/results/."""
    (tmp_path / "eval" / "results" / "e-val-1").mkdir(parents=True)
    # Patch _find_repo_root to return our fake repo
    monkeypatch.setattr(utils, "_find_repo_root", lambda: tmp_path)
    return tmp_path


def test_zero_dirs_raises(fake_results: pathlib.Path):
    with pytest.raises(FileNotFoundError, match="No shakedown directories"):
        utils.find_latest_shakedown("e-val-1")


def test_single_dir(fake_results: pathlib.Path):
    d = fake_results / "eval" / "results" / "e-val-1" / "shakedown-macos-2026-07-22T16-19-29Z"
    d.mkdir()
    result = utils.find_latest_shakedown("e-val-1")
    assert result == d


def test_multiple_dirs_returns_newest(fake_results: pathlib.Path):
    base = fake_results / "eval" / "results" / "e-val-1"
    old = base / "shakedown-macos-2026-07-01T10-00-00Z"
    mid = base / "shakedown-macos-2026-07-15T12-00-00Z"
    new = base / "shakedown-macos-2026-08-01T20-47-01Z"
    for d in (mid, old, new):  # create out of order
        d.mkdir()
    result = utils.find_latest_shakedown("e-val-1")
    assert result == new


def test_pinned_bypasses_glob(fake_results: pathlib.Path):
    # Pinned path does not need to exist (user responsibility)
    pinned = "/some/absolute/path"
    result = utils.find_latest_shakedown("e-val-1", pinned=pinned)
    assert result == pathlib.Path(pinned)


def test_pinned_relative_resolves_from_repo(fake_results: pathlib.Path):
    rel = "eval/results/e-val-1/shakedown-macos-2026-01-01T00-00-00Z"
    d = fake_results / rel
    d.mkdir(parents=True)
    result = utils.find_latest_shakedown("e-val-1", pinned=rel)
    assert result == d


def test_custom_host_tag(fake_results: pathlib.Path):
    base = fake_results / "eval" / "results" / "e-perf-1"
    base.mkdir(parents=True)
    d = base / "canonical-rpi4-2026-08-01T10-00-00Z"
    d.mkdir()
    result = utils.find_latest_shakedown("e-perf-1", host_tag="canonical-rpi4")
    assert result == d


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
