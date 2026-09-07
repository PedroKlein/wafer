#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import json
import os
import shlex
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
VERIFIER = ROOT / "eval/scripts/verify-storage-receipt.py"
WRAPPER = ROOT / "eval/scripts/qualify-results-storage.sh"


def make_volume(tmp_path: Path) -> Path:
    volume = tmp_path / "WAFER RESULTS"
    volume.mkdir()
    for name in ("raw", "manifests", "derived", "reports"):
        (volume / name).mkdir()
    return volume


def write_facts(
    path: Path,
    volume: Path,
    *,
    device_id: str = "by-id:usb-Kingston_fixture",
    uuid: str = "ABCD-1234",
    label: str = "WAF_RESULTS",
    filesystem: str = "exfat",
    mount_path: Path | None = None,
    options: list[str] | None = None,
    available_bytes: int = 10_000_000,
    mount_id: str = "41",
    path_device: int | None = None,
) -> None:
    value = {
        "schema_version": 1,
        "platform": "linux",
        "stable_device_ids": [device_id],
        "source": "/dev/sdz1",
        "mount_path": str(mount_path or volume),
        "mount_id": mount_id,
        "uuid": uuid,
        "label": label,
        "filesystem": filesystem,
        "mount_options": options or ["rw", "nosuid", "nodev"],
        "read_write": options is None or ("rw" in options and "ro" not in options),
        "available_bytes": available_bytes,
        "path_device": volume.stat().st_dev if path_device is None else path_device,
    }
    path.write_text(json.dumps(value))


def run(*args: str, env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(VERIFIER), *args],
        cwd=ROOT,
        env={**os.environ, "WAFER_SYNC": "/usr/bin/true", **(env or {})},
        capture_output=True,
        text=True,
        check=False,
    )


def prepare(volume: Path, facts: Path, qualification_id: str = "fixture-a") -> subprocess.CompletedProcess[str]:
    return run(
        "prepare",
        "--results-root",
        str(volume),
        "--facts-json",
        str(facts),
        "--expected-device-id",
        "by-id:usb-Kingston_fixture",
        "--expected-uuid",
        "ABCD-1234",
        "--qualification-id",
        qualification_id,
        "--min-free-bytes",
        "1000000",
        "--large-bytes",
        "1048576",
        "--small-files",
        "8",
        "--small-bytes",
        "257",
    )


@pytest.mark.parametrize(
    ("changes", "message"),
    [
        ({"device_id": "by-id:wrong"}, "stable device identifier"),
        ({"uuid": "WRONG"}, "filesystem UUID"),
        ({"label": "OTHER"}, "volume label"),
        ({"filesystem": "ext4"}, "filesystem type"),
        ({"available_bytes": 999}, "free-space margin"),
        ({"options": ["ro", "nosuid"]}, "read-write"),
        ({"mount_path": Path("/stale/mount")}, "mount path"),
        ({"path_device": -1}, "path identity"),
    ],
)
def test_prepare_rejects_identity_and_mount_failures_before_write(
    tmp_path: Path, changes: dict, message: str
) -> None:
    volume = make_volume(tmp_path)
    facts = tmp_path / "facts.json"
    write_facts(facts, volume, **changes)

    completed = prepare(volume, facts)

    assert completed.returncode == 1
    assert message in completed.stderr
    assert not (volume / "manifests/storage-qualification/fixture-a").exists()


def test_prepare_writes_bounded_corpus_manifest_and_receipt(tmp_path: Path) -> None:
    volume = make_volume(tmp_path)
    facts = tmp_path / "facts.json"
    write_facts(facts, volume)

    completed = prepare(volume, facts)

    assert completed.returncode == 0, completed.stderr
    qualification = volume / "manifests/storage-qualification/fixture-a"
    receipt = json.loads((qualification / "prepared.json").read_text())
    assert receipt["device"]["approved_stable_device_id"] == "by-id:usb-Kingston_fixture"
    assert receipt["device"]["observed_stable_device_ids"] == [
        "by-id:usb-Kingston_fixture"
    ]
    assert receipt["filesystem"]["uuid"] == "ABCD-1234"
    assert receipt["filesystem"]["label"] == "WAF_RESULTS"
    assert receipt["filesystem"]["type"] == "exfat"
    assert receipt["mount"]["path"] == str(volume)
    assert receipt["mount"]["read_write"] is True
    assert receipt["mount"]["available_bytes"] == 10_000_000
    assert receipt["corpus"]["file_count"] == 9
    assert receipt["corpus"]["byte_count"] == 1_048_576 + 8 * 257
    assert receipt["sync_completed"] is True
    assert not list(volume.joinpath("raw").rglob("*"))
    manifest = qualification / "corpus.sha256"
    assert receipt["corpus"]["manifest_sha256"] == hashlib.sha256(manifest.read_bytes()).hexdigest()


def test_verify_remount_rejects_unchanged_mount_id(tmp_path: Path) -> None:
    volume = make_volume(tmp_path)
    before = tmp_path / "before.json"
    write_facts(before, volume, mount_id="41")
    assert prepare(volume, before).returncode == 0
    after = tmp_path / "after.json"
    write_facts(after, volume, mount_id="41")

    completed = run(
        "verify-remount",
        "--results-root",
        str(volume),
        "--facts-json",
        str(after),
        "--prepared-receipt",
        str(volume / "manifests/storage-qualification/fixture-a/prepared.json"),
    )

    assert completed.returncode == 1
    assert "safe unmount/remount" in completed.stderr


def test_verify_remount_records_exact_counts_after_changed_mount_id(tmp_path: Path) -> None:
    volume = make_volume(tmp_path)
    before = tmp_path / "before.json"
    write_facts(before, volume, mount_id="41")
    assert prepare(volume, before).returncode == 0
    after = tmp_path / "after.json"
    write_facts(after, volume, mount_id="52")

    completed = run(
        "verify-remount",
        "--results-root",
        str(volume),
        "--facts-json",
        str(after),
        "--prepared-receipt",
        str(volume / "manifests/storage-qualification/fixture-a/prepared.json"),
    )

    assert completed.returncode == 0, completed.stderr
    receipt = json.loads(
        (volume / "manifests/storage-qualification/fixture-a/verified.json").read_text()
    )
    assert receipt["safe_unmount_remount_verified"] is True
    assert receipt["verification"] == {
        "expected_files": 9,
        "observed_files": 9,
        "expected_bytes": 1_048_576 + 8 * 257,
        "observed_bytes": 1_048_576 + 8 * 257,
        "missing_count": 0,
        "extra_count": 0,
        "mismatch_count": 0,
    }


def test_verify_remount_rejects_checksum_mismatch(tmp_path: Path) -> None:
    volume = make_volume(tmp_path)
    before = tmp_path / "before.json"
    write_facts(before, volume)
    assert prepare(volume, before).returncode == 0
    corpus_file = volume / "manifests/storage-qualification/fixture-a/corpus/small/file-0003.bin"
    corpus_file.write_bytes(b"changed")
    after = tmp_path / "after.json"
    write_facts(after, volume, mount_id="52")

    completed = run(
        "verify-remount",
        "--results-root",
        str(volume),
        "--facts-json",
        str(after),
        "--prepared-receipt",
        str(volume / "manifests/storage-qualification/fixture-a/prepared.json"),
    )

    assert completed.returncode == 1
    assert "checksum verification failed" in completed.stderr
    assert not (volume / "manifests/storage-qualification/fixture-a/verified.json").exists()


def write_raw_manifest(volume: Path) -> Path:
    raw_file = volume / "raw/e-val-1/run-01/metadata.json"
    raw_file.parent.mkdir(parents=True)
    raw_file.write_text('{"fixture":true}\n')
    digest = hashlib.sha256(raw_file.read_bytes()).hexdigest()
    manifest = volume / "manifests/expanded-n5.sha256"
    manifest.write_text(f"{digest}  raw/e-val-1/run-01/metadata.json\n")
    return manifest


def verified_receipt(volume: Path, facts: Path) -> Path:
    assert prepare(volume, facts).returncode == 0
    after = facts.with_name("after.json")
    write_facts(after, volume, mount_id="52")
    prepared = volume / "manifests/storage-qualification/fixture-a/prepared.json"
    assert run(
        "verify-remount",
        "--results-root",
        str(volume),
        "--facts-json",
        str(after),
        "--prepared-receipt",
        str(prepared),
    ).returncode == 0
    return volume / "manifests/storage-qualification/fixture-a/verified.json"


def test_handoff_verifies_same_manifest_and_analysis_output_boundary(tmp_path: Path) -> None:
    volume = make_volume(tmp_path)
    facts = tmp_path / "facts.json"
    write_facts(facts, volume)
    receipt = verified_receipt(volume, facts)
    handoff_facts = tmp_path / "macos.json"
    write_facts(
        handoff_facts,
        volume,
        device_id="VolumeUUID:ABCD-1234",
        mount_id="disk9s1",
    )
    value = json.loads(handoff_facts.read_text())
    value["platform"] = "macos"
    handoff_facts.write_text(json.dumps(value))
    manifest = write_raw_manifest(volume)

    completed = run(
        "handoff",
        "--results-root",
        str(volume),
        "--facts-json",
        str(handoff_facts),
        "--verified-receipt",
        str(receipt),
        "--manifest",
        str(manifest),
        "--analysis-output",
        str(volume / "derived/expanded-n5"),
        "--host",
        "macos",
    )

    assert completed.returncode == 0, completed.stderr
    handoff = json.loads(
        (volume / "manifests/storage-qualification/fixture-a/handoff-macos.json").read_text()
    )
    assert handoff["manifest_sha256"] == hashlib.sha256(manifest.read_bytes()).hexdigest()
    assert handoff["raw_open_mode"] == "read-only"
    assert handoff["analysis_output_relative"] == "derived/expanded-n5"
    assert handoff["verification"]["mismatch_count"] == 0


@pytest.mark.parametrize(
    ("manifest_line", "analysis_output", "expected"),
    [
        ("0" * 64 + "  derived/result.json\n", "derived/out", "raw-path drift"),
        (None, "raw/analysis", "analysis output"),
    ],
)
def test_handoff_rejects_raw_path_drift_and_analysis_write_into_raw(
    tmp_path: Path, manifest_line: str | None, analysis_output: str, expected: str
) -> None:
    volume = make_volume(tmp_path)
    facts = tmp_path / "facts.json"
    write_facts(facts, volume)
    receipt = verified_receipt(volume, facts)
    manifest = write_raw_manifest(volume)
    if manifest_line is not None:
        manifest.write_text(manifest_line)

    completed = run(
        "handoff",
        "--results-root",
        str(volume),
        "--facts-json",
        str(facts),
        "--verified-receipt",
        str(receipt),
        "--manifest",
        str(manifest),
        "--analysis-output",
        str(volume / analysis_output),
        "--host",
        "jetson",
    )

    assert completed.returncode == 1
    assert expected in completed.stderr


def test_linux_facts_uses_injected_platform_commands(tmp_path: Path) -> None:
    volume = make_volume(tmp_path)
    source = tmp_path / "sdz1"
    source.write_bytes(b"")
    by_id = tmp_path / "by-id"
    by_id.mkdir()
    (by_id / "usb-Kingston_fixture-part1").symlink_to(source)
    mountinfo = tmp_path / "mountinfo"
    escaped_mount = str(volume.resolve()).replace(" ", r"\040")
    mountinfo.write_text(f"41 22 0:99 / {escaped_mount} rw - fuseblk {source} rw\n")
    findmnt = tmp_path / "findmnt"
    findmnt_payload = json.dumps(
        {
            "filesystems": [
                {
                    "target": str(volume.resolve()),
                    "source": str(source),
                    "fstype": "exfat",
                    "options": "rw,nosuid,nodev",
                }
            ]
        }
    )
    findmnt.write_text(f"#!/bin/sh\nprintf '%s\\n' {shlex.quote(findmnt_payload)}\n")
    findmnt.chmod(0o755)
    lsblk = tmp_path / "lsblk"
    lsblk_payload = json.dumps(
        {
            "blockdevices": [
                {
                    "path": str(source),
                    "uuid": "ABCD-1234",
                    "label": "WAF_RESULTS",
                    "fstype": "exfat",
                }
            ]
        }
    )
    lsblk.write_text(f"#!/bin/sh\nprintf '%s\\n' {shlex.quote(lsblk_payload)}\n")
    lsblk.chmod(0o755)

    completed = run(
        "facts",
        "--results-root",
        str(volume),
        env={
            "WAFER_PLATFORM": "linux",
            "WAFER_FINDMNT": str(findmnt),
            "WAFER_LSBLK": str(lsblk),
            "WAFER_BY_ID": str(by_id),
            "WAFER_MOUNTINFO": str(mountinfo),
        },
    )

    assert completed.returncode == 0, completed.stderr
    value = json.loads(completed.stdout)
    assert value["stable_device_ids"] == ["by-id:usb-Kingston_fixture-part1"]
    assert value["uuid"] == "ABCD-1234"
    assert value["label"] == "WAF_RESULTS"
    assert value["filesystem"] == "exfat"
    assert value["mount_options"] == ["rw", "nosuid", "nodev"]
    assert value["mount_id"] == "41"


def test_wrapper_is_non_destructive_and_exposes_staged_commands() -> None:
    text = WRAPPER.read_text()
    for forbidden in ("mkfs", "diskutil erase", "umount", "sudo", "rm -rf"):
        assert forbidden not in text
    completed = subprocess.run(
        [str(WRAPPER), "--help"], cwd=ROOT, capture_output=True, text=True, check=False
    )
    assert completed.returncode == 0
    assert "prepare" in completed.stdout
    assert "verify-remount" in completed.stdout
    assert "handoff" in completed.stdout
