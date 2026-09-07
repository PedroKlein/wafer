from __future__ import annotations

import errno
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "eval/analysis/src/wafer_analysis"))

import results_layout as storage  # noqa: E402
from results_layout import (  # noqa: E402
    ResultsLayout,
    atomic_write_json,
    audit_portability,
    resolve_alias_receipt,
    validate_segment,
    validate_unique_names,
)


def mounted(_: object) -> bool:
    return True


def test_runner_wrapper_preserves_results_root_with_spaces(
    tmp_path: Path,
) -> None:
    binary = tmp_path / "bin/python3"
    binary.parent.mkdir()
    binary.write_text("#!/bin/sh\nprintf '%s\\n' \"$@\"\n")
    binary.chmod(0o755)
    environment = {
        **os.environ,
        "PATH": f"{binary.parent}:{os.environ['PATH']}",
        "WAFER_RESULTS_ROOT": str(tmp_path / "volume with spaces"),
    }

    completed = subprocess.run(
        [str(ROOT / "eval/scripts/run-rpi5-canonical.sh"), "--dry-run"],
        cwd=ROOT,
        env=environment,
        capture_output=True,
        text=True,
        check=True,
    )

    arguments = completed.stdout.splitlines()
    index = arguments.index("--results-root")
    assert arguments[index + 1] == str(tmp_path / "volume with spaces")


def test_repository_local_layout_preserves_existing_raw_path(tmp_path: Path) -> None:
    layout = ResultsLayout.resolve(tmp_path)

    assert layout.raw == tmp_path / "eval/results"
    assert layout.manifests == tmp_path / "eval/results"
    assert layout.derived == tmp_path / "eval/derived"
    assert layout.reports == tmp_path / "eval/reports"
    assert layout.explicit is False


def test_explicit_layout_supports_spaces_and_separates_managed_trees(tmp_path: Path) -> None:
    volume = tmp_path / "mounted results volume"
    volume.mkdir()

    layout = ResultsLayout.resolve(tmp_path, volume, mount_check=mounted)
    layout.prepare()

    assert layout.raw == volume / "raw"
    assert layout.manifests == volume / "manifests"
    assert layout.derived == volume / "derived"
    assert layout.reports == volume / "reports"
    assert all(path.is_dir() for path in (layout.raw, layout.manifests, layout.derived, layout.reports))


def test_explicit_layout_rejects_absent_and_unmounted_roots(tmp_path: Path) -> None:
    with pytest.raises(FileNotFoundError, match="absent"):
        ResultsLayout.resolve(tmp_path, tmp_path / "missing", mount_check=mounted)

    volume = tmp_path / "volume"
    volume.mkdir()
    with pytest.raises(ValueError, match="not a mounted filesystem"):
        ResultsLayout.resolve(tmp_path, volume, mount_check=lambda _: False)


def test_layout_rejects_overlapping_raw_and_derived(tmp_path: Path) -> None:
    volume = tmp_path / "volume"
    raw = volume / "raw"
    layout = ResultsLayout(
        volume=volume,
        raw=raw,
        manifests=volume / "manifests",
        derived=raw,
        reports=volume / "reports",
        explicit=True,
    )

    with pytest.raises(ValueError, match="must not overlap"):
        layout.validate()


def test_analysis_outputs_cannot_target_raw_or_traverse(tmp_path: Path) -> None:
    volume = tmp_path / "volume"
    volume.mkdir()
    layout = ResultsLayout.resolve(tmp_path, volume, mount_check=mounted)

    assert layout.analysis_path("derived", "batch-a", "table.csv") == volume / "derived/batch-a/table.csv"
    with pytest.raises(ValueError, match="derived or reports"):
        layout.analysis_path("raw", "batch-a")
    with pytest.raises(ValueError, match="traversal"):
        layout.analysis_path("reports", "../raw/result.json")


def test_layout_rejects_symlinks_and_case_only_collisions(tmp_path: Path) -> None:
    volume = tmp_path / "volume"
    volume.mkdir()
    outside = tmp_path / "outside"
    outside.mkdir()
    (volume / "raw").symlink_to(outside, target_is_directory=True)

    with pytest.raises(ValueError, match="symlinks"):
        ResultsLayout.resolve(tmp_path, volume, mount_check=mounted)

    portable = tmp_path / "portable"
    with pytest.raises(ValueError, match="case-only"):
        validate_unique_names([portable / "Run-A", portable / "run-a"])


def test_exfat_names_and_hardlinks_fail_closed(tmp_path: Path) -> None:
    for name in ("CON", "bad:name", "trailing.", "two words"):
        with pytest.raises(ValueError):
            validate_segment(name)

    source = tmp_path / "source"
    source.write_text("data")
    link = tmp_path / "link"
    os.link(source, link)
    with pytest.raises(ValueError, match="hardlinked"):
        audit_portability(tmp_path)


def test_alias_receipt_rejects_link_and_tampered_source(tmp_path: Path) -> None:
    volume = tmp_path / "volume"
    source = volume / "raw/e-perf-1/rpi5-batch/native/run-01-attempt-01"
    source.mkdir(parents=True)
    status = source / "canonical-status.json"
    status.write_text('{"status":"passed"}')
    receipt = volume / "manifests/aliases/e-perf-2/rpi5-batch/native/run-01.json"
    atomic_write_json(receipt, {
        "schema_version": 1,
        "experiment": "e-perf-2",
        "condition": "native",
        "run_index": 1,
        "shared_from_experiment": "e-perf-1",
        "source_leaf": "raw/e-perf-1/rpi5-batch/native/run-01-attempt-01",
        "source_status_sha256": hashlib.sha256(status.read_bytes()).hexdigest(),
        "sample_identity": "raw/e-perf-1/rpi5-batch/native/run-01-attempt-01",
        "shared_measurement": True,
    })

    assert resolve_alias_receipt(receipt)[1] == source
    status.write_text('{"status":"failed"}')
    with pytest.raises(ValueError, match="digest differs"):
        resolve_alias_receipt(receipt)

    link = receipt.with_name("linked.json")
    link.symlink_to(receipt)
    with pytest.raises(ValueError, match="must not be a symlink"):
        resolve_alias_receipt(link)


def test_atomic_json_refuses_overwrite_and_cross_device_rename(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = tmp_path / "receipt.json"
    atomic_write_json(path, {"value": 1})
    assert json.loads(path.read_text()) == {"value": 1}
    with pytest.raises(FileExistsError, match="overwrite"):
        atomic_write_json(path, {"value": 2})

    other = tmp_path / "other.json"
    original = storage.os.replace

    def cross_device(source: object, destination: object) -> None:
        if Path(destination) == other:
            raise OSError(errno.EXDEV, "cross-device")
        original(source, destination)

    monkeypatch.setattr(storage.os, "replace", cross_device)
    with pytest.raises(OSError, match="cross-device rename"):
        atomic_write_json(other, {"value": 1})
    assert not other.exists()
