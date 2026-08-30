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


def test_zero_dirs_raises(fake_results: pathlib.Path):
    with pytest.raises(FileNotFoundError, match="No shakedown directories"):
        utils.find_latest_shakedown("e-val-1")


def test_single_dir(fake_results: pathlib.Path):
    d = fake_results / "eval" / "results" / "e-val-1" / "shakedown-macos-2026-07-22T16-19-29Z"
    d.mkdir()
    result = utils.find_latest_shakedown("e-val-1")
    assert result == d


def test_multiple_dirs_returns_newest(fake_results: pathlib.Path):
    """Regression: prove the helper uses LEX sort on the directory name,
    not filesystem mtime. Creates dirs in reverse chronological order and
    stamps each with an mtime that inverts creation order, so a mtime-
    based sort would return the wrong (oldest-named) directory.

    Reviewer flagged the previous version (which created `(mid, old, new)`)
    as insufficient because mtime and lex sort both returned `new`.
    """
    import os
    import time

    base = fake_results / "eval" / "results" / "e-val-1"
    old = base / "shakedown-macos-2026-07-01T10-00-00Z"
    mid = base / "shakedown-macos-2026-07-15T12-00-00Z"
    new = base / "shakedown-macos-2026-08-01T20-47-01Z"
    # Create newest-first so the filesystem's implicit mtime ordering
    # inverts the lex ordering.
    for d in (new, mid, old):
        d.mkdir()
    # Force mtime inversion explicitly — mkdir mtimes on fast SSDs collide
    # inside the same millisecond and macOS HFS+ has 1s granularity.
    now = time.time()
    os.utime(new, (now - 3, now - 3))  # oldest mtime
    os.utime(mid, (now - 2, now - 2))
    os.utime(old, (now - 1, now - 1))  # newest mtime

    result = utils.find_latest_shakedown("e-val-1")
    assert result == new, (
        f"expected lex-newest {new.name}, got {result.name} — "
        "helper is using mtime sort instead of lex sort"
    )


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
    d = base / "rpi5-2026-08-01T10-00-00Z"
    d.mkdir()
    result = utils.find_latest_shakedown("e-perf-1", host_tag="rpi5")
    assert result == d


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
    assert utils.find_latest_shakedown("e-val-1") == batch


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


def test_cross_architecture_requires_x86_batch(fake_results: pathlib.Path):
    pi_batch = fake_results / "eval/results/e-perf-5/rpi5-batch"
    write_canonical_leaf(pi_batch / "wafer/run-01-attempt-01")
    with pytest.raises(ValueError, match="incomplete until matching x86 Linux"):
        utils.require_cross_architecture(pi_batch, None)


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
