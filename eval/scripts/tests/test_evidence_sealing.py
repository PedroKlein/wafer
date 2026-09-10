from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
VERIFIER = ROOT / "eval/scripts/verify-storage-receipt.py"


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def fixture(tmp_path: Path) -> tuple[Path, Path, Path, Path]:
    volume = tmp_path / "volume"
    for name in ("raw", "manifests", "derived", "reports"):
        (volume / name).mkdir(parents=True, exist_ok=True)
    leaf = volume / "raw/e-val-1/rpi5-expanded-n5/run-01-attempt-01"
    write_json(leaf / "canonical-status.json", {"status": "passed"})
    write_json(leaf / "metadata.json", {"fixture": True})
    status_hash = hashlib.sha256((leaf / "canonical-status.json").read_bytes()).hexdigest()
    alias = volume / "manifests/aliases/e-perf-2/rpi5-expanded-n5/wafer/run-01.json"
    write_json(
        alias,
        {
            "source_leaf": leaf.relative_to(volume).as_posix(),
            "source_status_sha256": status_hash,
        },
    )
    facts = tmp_path / "facts.json"
    write_json(
        facts,
        {
            "schema_version": 1,
            "platform": "linux",
            "stable_device_ids": ["by-id:fixture"],
            "source": "/dev/sdz1",
            "mount_path": str(volume),
            "mount_id": "42",
            "uuid": "ABCD-1234",
            "label": "WAF_RESULTS",
            "filesystem": "exfat",
            "mount_options": ["rw"],
            "read_write": True,
            "available_bytes": 1_000_000,
            "path_device": volume.stat().st_dev,
        },
    )
    verified = volume / "manifests/storage-qualification/fixture/verified.json"
    write_json(
        verified,
        {
            "state": "verified-after-remount",
            "qualification_id": "fixture",
            "device": {"approved_stable_device_id": "by-id:fixture"},
            "filesystem": {"uuid": "ABCD-1234"},
        },
    )
    reconciliation = volume / "manifests/n5-batches/rpi5-expanded-n5/t19-terminal-reconciliation.json"
    write_json(
        reconciliation.parent / "supervisor-terminal.json",
        {"status": "passed", "batch_id": "expanded-n5"},
    )
    host_leaf = volume / "raw/e-host-thermal-storage/host-characterization-fixture"
    write_json(
        host_leaf / "host-load-ladder.json",
        {
            "status": "passed",
            "source_state": {
                "git_dirty": False,
                "git_sha": "d4f26e32e6601419a0768403b8a0728035ff1e67",
                "git_tags": ["rpi5-final-rc-v6"],
            },
        },
    )
    (host_leaf / "host-telemetry.csv").write_text("temperature_millicelsius\n25000\n")
    (host_leaf / "kernel-io.log").write_text("")
    write_json(host_leaf / "usb-integrity.json", {"checksum_mismatch_count": 0})
    admission = reconciliation.parent / "usb-host-admission-receipt.json"
    write_json(
        admission,
        {
            "admission": {"expanded_n5": True, "n30": False},
            "host_characterization": {
                "session_id": "host-characterization-fixture",
                "sha256": hashlib.sha256(
                    (host_leaf / "host-load-ladder.json").read_bytes()
                ).hexdigest(),
                "diagnostic_admission": True,
                "final_admission": False,
                "kernel_io_error_count": 0,
            },
            "volume": {
                "uuid": "EFBF-159A",
                "label": "WAF_RESULTS",
                "filesystem": "exfat",
            },
        },
    )
    b00_terminal = reconciliation.parent / "batches/B00-host-prerequisite/terminal.json"
    write_json(
        b00_terminal,
        {
            "status": "passed",
            "batch_id": "expanded-n5",
            "disposition": "qualified-prerequisite-not-reexecuted",
            "schedule_result_keys": [
                "e-host-thermal-storage/eight-phase-load-ladder/run-01"
            ],
            "admission_receipt_sha256": hashlib.sha256(admission.read_bytes()).hexdigest(),
        },
    )
    supervisor = reconciliation.parent / "supervisor-terminal.json"
    write_json(
        reconciliation,
        {
            "schema_version": 1,
            "status": "passed",
            "classification": "diagnostic-expanded-n5-terminal-reconciliation",
            "batch_id": "expanded-n5",
            "campaign_started": False,
            "thesis_evidence": False,
            "n30_admitted": False,
            "counts": {
                "schedule_records": 3,
                "selected_raw_leaves": 1,
                "aliases": 1,
                "qualified_prerequisites": 1,
                "unselected_attempts": 0,
                "unselected_failed_attempts": 0,
                "batch_terminals": 2,
                "successful_progress_keys": 2,
            },
            "schedule_sha256": "a" * 64,
            "supervisor_terminal_sha256": hashlib.sha256(supervisor.read_bytes()).hexdigest(),
            "batch_terminal_sha256": {
                "B00-host-prerequisite": hashlib.sha256(b00_terminal.read_bytes()).hexdigest()
            },
            "final_health": {
                "boot_id": "boot-a",
                "free_bytes": 1_000_000,
                "throttled": "throttled=0x0",
                "temperature_millicelsius": 25_000,
                "kernel_io_errors": 0,
            },
            "selected": {
                "e-val-1/delay-50ms/run-01": {
                    "path": leaf.relative_to(volume).as_posix(),
                    "metadata_sha256": hashlib.sha256((leaf / "metadata.json").read_bytes()).hexdigest(),
                    "status_sha256": status_hash,
                    "release_tag": "rpi5-final-rc-v13",
                    "wafer_git_sha": "55b1a4e942c192908981a8aa970c180829fd6360",
                    "control_generation": "continuation-10",
                    "control_batch": "B01-validation",
                }
            },
            "aliases": {
                "e-perf-2/wafer/run-01": {
                    "path": alias.relative_to(volume).as_posix(),
                    "receipt_sha256": hashlib.sha256(alias.read_bytes()).hexdigest(),
                    "source_result_key": "e-val-1/delay-50ms/run-01",
                    "source_leaf": leaf.relative_to(volume).as_posix(),
                    "source_status_sha256": status_hash,
                }
            },
            "qualified_prerequisites": ["e-host-thermal-storage/eight-phase-load-ladder/run-01"],
            "unselected_attempts": [],
            "release_composition": {"rpi5-final-rc-v13": 1},
            "control_composition": {"continuation-10": 1},
        },
    )
    return volume, facts, verified, reconciliation


def run_seal(
    volume: Path,
    facts: Path,
    verified: Path,
    reconciliation: Path,
    *,
    proc_root: Path,
    env: dict[str, str] | None = None,
    expected_records: int = 3,
    expected_raw: int = 1,
    expected_aliases: int = 1,
    expected_unselected: int = 0,
    expected_unselected_failed: int = 0,
    production_defaults: bool = False,
) -> subprocess.CompletedProcess[str]:
    boot_id = proc_root.parent / "boot-id"
    thermal = proc_root.parent / "thermal"
    vcgencmd = proc_root.parent / "vcgencmd"
    journalctl = proc_root.parent / "journalctl"
    boot_id.write_text("boot-a\n")
    thermal.write_text("25000\n")
    vcgencmd.write_text("#!/bin/sh\necho throttled=0x0\n")
    journalctl.write_text("#!/bin/sh\nexit 0\n")
    vcgencmd.chmod(0o755)
    journalctl.chmod(0o755)
    arguments = [
        sys.executable,
        str(VERIFIER),
        "seal",
        "--results-root",
        str(volume),
        "--facts-json",
        str(facts),
        "--verified-receipt",
        str(verified),
        "--terminal-reconciliation",
        str(reconciliation),
        "--expected-reconciliation-sha256",
        hashlib.sha256(reconciliation.read_bytes()).hexdigest(),
    ]
    if not production_defaults:
        arguments.extend(
            [
                "--fixture-composition",
                "--expected-records",
                str(expected_records),
                "--expected-raw",
                str(expected_raw),
                "--expected-aliases",
                str(expected_aliases),
                "--expected-prerequisites",
                "1",
                "--expected-unselected",
                str(expected_unselected),
                "--expected-unselected-failed",
                str(expected_unselected_failed),
            ]
        )
    return subprocess.run(
        arguments,
        cwd=ROOT,
        env={
            **os.environ,
            "WAFER_SYNC": "/usr/bin/true",
            "WAFER_PROC_ROOT": str(proc_root),
            "WAFER_BOOT_ID_PATH": str(boot_id),
            "WAFER_THERMAL_PATH": str(thermal),
            "WAFER_VCGENCMD": str(vcgencmd),
            "WAFER_JOURNALCTL": str(journalctl),
            **(env or {}),
        },
        capture_output=True,
        text=True,
    )


def run_handoff(
    volume: Path,
    facts: Path,
    verified: Path,
    reconciliation: Path,
    *,
    expected_unselected: int = 0,
    expected_unselected_failed: int = 0,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            sys.executable,
            str(VERIFIER),
            "handoff",
            "--results-root",
            str(volume),
            "--facts-json",
            str(facts),
            "--verified-receipt",
            str(verified),
            "--manifest",
            str(volume / "manifests/expanded-n5.sha256"),
            "--source-seal",
            str(reconciliation.parent / "source-seal.json"),
            "--composite",
            str(reconciliation.parent / "composite-index.json"),
            "--analysis-output",
            str(volume / "derived/expanded-n5"),
            "--host",
            "macos",
            "--fixture-composition",
            "--expected-records",
            "3",
            "--expected-raw",
            "1",
            "--expected-aliases",
            "1",
            "--expected-prerequisites",
            "1",
            "--expected-unselected",
            str(expected_unselected),
            "--expected-unselected-failed",
            str(expected_unselected_failed),
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )


def seal_paths(volume: Path, reconciliation: Path) -> tuple[Path, Path, Path]:
    return (
        volume / "manifests/expanded-n5.sha256",
        reconciliation.parent / "composite-index.json",
        reconciliation.parent / "source-seal.json",
    )


def assert_unsealed(volume: Path, reconciliation: Path) -> None:
    assert all(not path.exists() for path in seal_paths(volume, reconciliation))


def test_seal_rejects_selected_non_pass_before_writing_artifacts(tmp_path: Path) -> None:
    volume, facts, verified, reconciliation = fixture(tmp_path)
    value = json.loads(reconciliation.read_text())
    selected = value["selected"]["e-val-1/delay-50ms/run-01"]
    status = volume / selected["path"] / "canonical-status.json"
    write_json(status, {"status": "failed"})
    selected["status_sha256"] = hashlib.sha256(status.read_bytes()).hexdigest()
    alias_path = volume / value["aliases"]["e-perf-2/wafer/run-01"]["path"]
    alias = json.loads(alias_path.read_text())
    alias["source_status_sha256"] = selected["status_sha256"]
    write_json(alias_path, alias)
    value["aliases"]["e-perf-2/wafer/run-01"].update(
        {
            "receipt_sha256": hashlib.sha256(alias_path.read_bytes()).hexdigest(),
            "source_status_sha256": selected["status_sha256"],
        }
    )
    write_json(reconciliation, value)
    proc_root = tmp_path / "proc"
    proc_root.mkdir()

    completed = run_seal(volume, facts, verified, reconciliation, proc_root=proc_root)

    assert completed.returncode == 1
    assert "selected leaf is not passed" in completed.stderr
    assert_unsealed(volume, reconciliation)


def test_seal_rejects_unsafe_final_host_state(tmp_path: Path) -> None:
    volume, facts, verified, reconciliation = fixture(tmp_path)
    proc_root = tmp_path / "proc"
    proc_root.mkdir()
    vcgencmd = tmp_path / "changing-vcgencmd"
    count = tmp_path / "vcgencmd-count"
    vcgencmd.write_text(
        "#!/bin/sh\n"
        f"count=$(cat {count!s} 2>/dev/null || echo 0)\n"
        "count=$((count + 1))\n"
        f"echo $count > {count!s}\n"
        "test $count -eq 1 && echo throttled=0x0 || echo throttled=0x50000\n"
    )
    vcgencmd.chmod(0o755)

    completed = run_seal(
        volume,
        facts,
        verified,
        reconciliation,
        proc_root=proc_root,
        env={"WAFER_VCGENCMD": str(vcgencmd)},
    )

    assert completed.returncode == 1
    assert "host throttle state is unsafe" in completed.stderr
    assert not (reconciliation.parent / "source-seal.json").exists()


def test_seal_rejects_boot_drift_from_terminal_reconciliation(tmp_path: Path) -> None:
    volume, facts, verified, reconciliation = fixture(tmp_path)
    proc_root = tmp_path / "proc"
    proc_root.mkdir()
    boot_id = tmp_path / "other-boot-id"
    boot_id.write_text("boot-b\n")

    completed = run_seal(
        volume,
        facts,
        verified,
        reconciliation,
        proc_root=proc_root,
        env={"WAFER_BOOT_ID_PATH": str(boot_id)},
    )

    assert completed.returncode == 1
    assert "boot ID differs" in completed.stderr
    assert_unsealed(volume, reconciliation)


def test_seal_rejects_supervisor_terminal_hash_drift(tmp_path: Path) -> None:
    volume, facts, verified, reconciliation = fixture(tmp_path)
    terminal = reconciliation.parent / "supervisor-terminal.json"
    value = json.loads(terminal.read_text())
    value["completed_at"] = "changed"
    write_json(terminal, value)
    proc_root = tmp_path / "proc"
    proc_root.mkdir()

    completed = run_seal(volume, facts, verified, reconciliation, proc_root=proc_root)

    assert completed.returncode == 1
    assert "supervisor terminal digest differs" in completed.stderr
    assert_unsealed(volume, reconciliation)


def test_seal_rejects_composite_drift_before_writing_artifacts(tmp_path: Path) -> None:
    volume, facts, verified, reconciliation = fixture(tmp_path)
    value = json.loads(reconciliation.read_text())
    value["selected"]["e-val-1/delay-50ms/run-01"]["metadata_sha256"] = "0" * 64
    write_json(reconciliation, value)
    proc_root = tmp_path / "proc"
    proc_root.mkdir()

    completed = run_seal(volume, facts, verified, reconciliation, proc_root=proc_root)

    assert completed.returncode == 1
    assert "selected leaf digest differs" in completed.stderr
    assert_unsealed(volume, reconciliation)


def test_seal_rejects_active_evidence_writer_before_writing_artifacts(tmp_path: Path) -> None:
    volume, facts, verified, reconciliation = fixture(tmp_path)
    proc_root = tmp_path / "proc"
    writer = proc_root / "123"
    writer.mkdir(parents=True)
    (writer / "cmdline").write_bytes(b"python3\0n5_runner.py\0")

    completed = run_seal(volume, facts, verified, reconciliation, proc_root=proc_root)

    assert completed.returncode == 1
    assert "evidence writers are active" in completed.stderr
    assert_unsealed(volume, reconciliation)


def test_seal_rejects_unknown_process_with_writable_raw_descriptor(tmp_path: Path) -> None:
    volume, facts, verified, reconciliation = fixture(tmp_path)
    proc_root = tmp_path / "proc"
    process = proc_root / "456"
    (process / "fd").mkdir(parents=True)
    (process / "fdinfo").mkdir()
    (process / "cmdline").write_bytes(b"python3\0unrelated.py\0")
    raw_file = volume / "raw/e-val-1/rpi5-expanded-n5/run-01-attempt-01/metadata.json"
    (process / "fd/7").symlink_to(raw_file)
    (process / "fdinfo/7").write_text("flags:\t0100001\n")

    completed = run_seal(volume, facts, verified, reconciliation, proc_root=proc_root)

    assert completed.returncode == 1
    assert "evidence writers are active" in completed.stderr
    assert "fd=7" in completed.stderr
    assert_unsealed(volume, reconciliation)


def test_seal_allows_unknown_process_with_read_only_raw_descriptor(tmp_path: Path) -> None:
    volume, facts, verified, reconciliation = fixture(tmp_path)
    proc_root = tmp_path / "proc"
    process = proc_root / "456"
    (process / "fd").mkdir(parents=True)
    (process / "fdinfo").mkdir()
    (process / "cmdline").write_bytes(b"python3\0unrelated.py\0")
    raw_file = volume / "raw/e-val-1/rpi5-expanded-n5/run-01-attempt-01/metadata.json"
    (process / "fd/7").symlink_to(raw_file)
    (process / "fdinfo/7").write_text("flags:\t0100000\n")

    completed = run_seal(volume, facts, verified, reconciliation, proc_root=proc_root)

    assert completed.returncode == 0, completed.stderr


def test_seal_rejects_wrong_uuid_and_failed_terminal(tmp_path: Path) -> None:
    for failure in ("uuid", "terminal"):
        case = tmp_path / failure
        case.mkdir()
        volume, facts, verified, reconciliation = fixture(case)
        if failure == "uuid":
            value = json.loads(facts.read_text())
            value["uuid"] = "WRONG"
            write_json(facts, value)
            expected = "filesystem UUID"
        else:
            terminal = reconciliation.parent / "supervisor-terminal.json"
            value = json.loads(terminal.read_text())
            value["status"] = "failed"
            write_json(terminal, value)
            expected = "supervisor terminal is not passed"
        proc_root = case / "proc"
        proc_root.mkdir()

        completed = run_seal(volume, facts, verified, reconciliation, proc_root=proc_root)

        assert completed.returncode == 1
        assert expected in completed.stderr
        assert_unsealed(volume, reconciliation)


def test_seal_rejects_selected_unselected_path_overlap(tmp_path: Path) -> None:
    volume, facts, verified, reconciliation = fixture(tmp_path)
    value = json.loads(reconciliation.read_text())
    selected = value["selected"]["e-val-1/delay-50ms/run-01"]
    value["unselected_attempts"] = [
        {
            "result_key": "e-val-1/delay-50ms/run-01",
            "path": selected["path"],
            "status": "failed",
            "status_sha256": selected["status_sha256"],
        }
    ]
    value["counts"]["unselected_attempts"] = 1
    value["counts"]["unselected_failed_attempts"] = 1
    write_json(reconciliation, value)
    proc_root = tmp_path / "proc"
    proc_root.mkdir()

    completed = run_seal(
        volume,
        facts,
        verified,
        reconciliation,
        proc_root=proc_root,
        expected_unselected=1,
        expected_unselected_failed=1,
    )

    assert completed.returncode == 1
    assert "selected and unselected raw paths overlap" in completed.stderr
    assert_unsealed(volume, reconciliation)


def test_seal_rejects_pooled_selected_attempt(tmp_path: Path) -> None:
    volume, facts, verified, reconciliation = fixture(tmp_path)
    value = json.loads(reconciliation.read_text())
    value["selected"]["e-val-1/delay-75ms/run-01"] = dict(
        value["selected"]["e-val-1/delay-50ms/run-01"]
    )
    value["counts"]["selected_raw_leaves"] = 2
    value["counts"]["schedule_records"] = 4
    value["release_composition"]["rpi5-final-rc-v13"] = 2
    value["control_composition"]["continuation-10"] = 2
    write_json(reconciliation, value)
    proc_root = tmp_path / "proc"
    proc_root.mkdir()

    completed = run_seal(
        volume,
        facts,
        verified,
        reconciliation,
        proc_root=proc_root,
        expected_records=4,
        expected_raw=2,
    )

    assert completed.returncode == 1
    assert "selected raw path is pooled" in completed.stderr
    assert_unsealed(volume, reconciliation)


def test_seal_rejects_alias_source_and_attempt_set_drift(tmp_path: Path) -> None:
    for failure in ("alias", "attempt"):
        case = tmp_path / failure
        case.mkdir()
        volume, facts, verified, reconciliation = fixture(case)
        if failure == "alias":
            value = json.loads(reconciliation.read_text())
            value["aliases"]["e-perf-2/wafer/run-01"]["source_result_key"] = "unknown"
            write_json(reconciliation, value)
            expected = "alias source selection differs"
        else:
            extra = volume / "raw/e-val-1/rpi5-expanded-n5/run-02-attempt-01"
            write_json(extra / "canonical-status.json", {"status": "failed"})
            expected = "campaign attempt paths differ"
        proc_root = case / "proc"
        proc_root.mkdir()

        completed = run_seal(volume, facts, verified, reconciliation, proc_root=proc_root)

        assert completed.returncode == 1
        assert expected in completed.stderr
        assert_unsealed(volume, reconciliation)


def test_seal_rejects_non_exfat_safe_relative_path() -> None:
    import importlib.util

    spec = importlib.util.spec_from_file_location("storage_verifier", VERIFIER)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)

    try:
        module.validate_portable_relative_paths(["raw/bad:name/data"])
    except ValueError as error:
        assert "not exFAT-safe" in str(error)
    else:
        raise AssertionError("non-portable path must fail")


def test_seal_rejects_portable_path_collision() -> None:
    import importlib.util

    spec = importlib.util.spec_from_file_location("storage_verifier", VERIFIER)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)

    try:
        module.validate_portable_relative_paths(["raw/straße/data", "raw/STRASSE/data"])
    except ValueError as error:
        assert "case or normalization collision" in str(error)
    else:
        raise AssertionError("portable collision must fail")


def test_seal_rejects_hardlinked_raw_file(tmp_path: Path) -> None:
    volume, facts, verified, reconciliation = fixture(tmp_path)
    source = volume / "raw/e-val-1/rpi5-expanded-n5/run-01-attempt-01/metadata.json"
    os.link(source, source.with_name("hardlinked-metadata.json"))
    proc_root = tmp_path / "proc"
    proc_root.mkdir()

    completed = run_seal(volume, facts, verified, reconciliation, proc_root=proc_root)

    assert completed.returncode == 1
    assert "must not be hardlinked" in completed.stderr
    assert_unsealed(volume, reconciliation)


def test_seal_rejects_linked_raw_path(tmp_path: Path) -> None:
    volume, facts, verified, reconciliation = fixture(tmp_path)
    source = volume / "raw/e-val-1/rpi5-expanded-n5/run-01-attempt-01/metadata.json"
    link = source.with_name("linked-metadata.json")
    link.symlink_to(source)
    proc_root = tmp_path / "proc"
    proc_root.mkdir()

    completed = run_seal(volume, facts, verified, reconciliation, proc_root=proc_root)

    assert completed.returncode == 1
    assert "raw manifest path must not be linked" in completed.stderr
    assert_unsealed(volume, reconciliation)


def test_seal_rejects_raw_mutation_during_sync(tmp_path: Path) -> None:
    volume, facts, verified, reconciliation = fixture(tmp_path)
    proc_root = tmp_path / "proc"
    proc_root.mkdir()
    sync = tmp_path / "sync"
    target = volume / "raw/e-val-1/rpi5-expanded-n5/run-01-attempt-01/metadata.json"
    sync.write_text(f"#!/bin/sh\nprintf changed >> {target!s}\n")
    sync.chmod(0o755)

    completed = run_seal(
        volume,
        facts,
        verified,
        reconciliation,
        proc_root=proc_root,
        env={"WAFER_SYNC": str(sync)},
    )

    assert completed.returncode == 1
    assert "raw tree changed during source sealing" in completed.stderr
    assert_unsealed(volume, reconciliation)


def test_seal_rejects_alias_mutation_after_composite_write(tmp_path: Path) -> None:
    volume, facts, verified, reconciliation = fixture(tmp_path)
    proc_root = tmp_path / "proc"
    proc_root.mkdir()
    sync = tmp_path / "sync"
    count = tmp_path / "sync-count"
    alias = volume / "manifests/aliases/e-perf-2/rpi5-expanded-n5/wafer/run-01.json"
    sync.write_text(
        "#!/bin/sh\n"
        f"count=$(cat {count!s} 2>/dev/null || echo 0)\n"
        "count=$((count + 1))\n"
        f"echo $count > {count!s}\n"
        f"test $count -ne 2 || printf changed >> {alias!s}\n"
    )
    sync.chmod(0o755)

    completed = run_seal(
        volume,
        facts,
        verified,
        reconciliation,
        proc_root=proc_root,
        env={"WAFER_SYNC": str(sync)},
    )

    assert completed.returncode == 1
    assert "alias receipt digest differs" in completed.stderr
    assert not (reconciliation.parent / "source-seal.json").exists()


def test_seal_preserves_unselected_failed_attempt_identity(tmp_path: Path) -> None:
    volume, facts, verified, reconciliation = fixture(tmp_path)
    failed = volume / "raw/e-val-1/rpi5-expanded-n5/run-01-attempt-02"
    write_json(failed / "canonical-status.json", {"status": "failed"})
    value = json.loads(reconciliation.read_text())
    value["counts"]["unselected_attempts"] = 1
    value["counts"]["unselected_failed_attempts"] = 1
    value["unselected_attempts"] = [
        {
            "result_key": "e-val-1/delay-50ms/run-01",
            "path": failed.relative_to(volume).as_posix(),
            "status": "failed",
            "status_sha256": hashlib.sha256((failed / "canonical-status.json").read_bytes()).hexdigest(),
        }
    ]
    write_json(reconciliation, value)
    proc_root = tmp_path / "proc"
    proc_root.mkdir()

    completed = run_seal(
        volume,
        facts,
        verified,
        reconciliation,
        proc_root=proc_root,
        expected_unselected=1,
        expected_unselected_failed=1,
    )

    assert completed.returncode == 0, completed.stderr
    composite = json.loads((reconciliation.parent / "composite-index.json").read_text())
    assert composite["counts"]["unselected_attempts"] == 1
    assert composite["unselected_attempts"] == value["unselected_attempts"]


def test_handoff_revalidates_source_seal_and_composite(tmp_path: Path) -> None:
    volume, facts, verified, reconciliation = fixture(tmp_path)
    proc_root = tmp_path / "proc"
    proc_root.mkdir()
    assert run_seal(volume, facts, verified, reconciliation, proc_root=proc_root).returncode == 0
    macos_facts = tmp_path / "macos.json"
    value = json.loads(facts.read_text())
    value.update(
        {
            "platform": "macos",
            "stable_device_ids": ["VolumeUUID:ABCD-1234"],
            "mount_id": "disk9s1",
        }
    )
    write_json(macos_facts, value)
    source_seal = reconciliation.parent / "source-seal.json"
    composite = reconciliation.parent / "composite-index.json"

    completed = run_handoff(volume, macos_facts, verified, reconciliation)

    assert completed.returncode == 0, completed.stderr
    receipt = json.loads(
        (volume / "manifests/storage-qualification/fixture/handoff-macos.json").read_text()
    )
    assert receipt["source_seal_sha256"] == hashlib.sha256(source_seal.read_bytes()).hexdigest()
    assert receipt["composite_sha256"] == hashlib.sha256(composite.read_bytes()).hexdigest()


def test_handoff_accepts_cross_host_mtime_representation_change(tmp_path: Path) -> None:
    volume, facts, verified, reconciliation = fixture(tmp_path)
    proc_root = tmp_path / "proc"
    proc_root.mkdir()
    assert run_seal(volume, facts, verified, reconciliation, proc_root=proc_root).returncode == 0
    raw_file = volume / "raw/e-val-1/rpi5-expanded-n5/run-01-attempt-01/metadata.json"
    stat = raw_file.stat()
    os.utime(raw_file, ns=(stat.st_atime_ns, stat.st_mtime_ns + 1_000_000_000))
    handoff_facts = tmp_path / "handoff-facts.json"
    handoff_value = json.loads(facts.read_text())
    handoff_value["platform"] = "macos"
    write_json(handoff_facts, handoff_value)

    completed = run_handoff(volume, handoff_facts, verified, reconciliation)

    assert completed.returncode == 0, completed.stderr


def test_handoff_rejects_missing_extra_or_manifest_drift(tmp_path: Path) -> None:
    for failure in ("missing", "extra", "manifest"):
        case = tmp_path / failure
        case.mkdir()
        volume, facts, verified, reconciliation = fixture(case)
        proc_root = case / "proc"
        proc_root.mkdir()
        assert run_seal(volume, facts, verified, reconciliation, proc_root=proc_root).returncode == 0
        if failure == "missing":
            (volume / "raw/e-val-1/rpi5-expanded-n5/run-01-attempt-01/metadata.json").unlink()
        elif failure == "extra":
            (volume / "raw/unexpected.txt").write_text("unexpected\n")
        else:
            manifest = volume / "manifests/expanded-n5.sha256"
            lines = manifest.read_text().splitlines()
            lines[0] = "0" * 64 + lines[0][64:]
            manifest.write_text("\n".join(lines) + "\n")
        handoff_facts = case / "handoff-facts.json"
        handoff_value = json.loads(facts.read_text())
        handoff_value["platform"] = "macos"
        write_json(handoff_facts, handoff_value)

        completed = run_handoff(volume, handoff_facts, verified, reconciliation)

        assert completed.returncode == 1
        assert "raw manifest verification failed" in completed.stderr
        assert not (
            volume / "manifests/storage-qualification/fixture/handoff-macos.json"
        ).exists()


def test_handoff_rejects_unsafe_source_host_state(tmp_path: Path) -> None:
    volume, facts, verified, reconciliation = fixture(tmp_path)
    proc_root = tmp_path / "proc"
    proc_root.mkdir()
    assert run_seal(volume, facts, verified, reconciliation, proc_root=proc_root).returncode == 0
    source_seal = reconciliation.parent / "source-seal.json"
    value = json.loads(source_seal.read_text())
    value["host_state"]["throttled"] = "throttled=0x50000"
    write_json(source_seal, value)
    handoff_facts = tmp_path / "handoff-facts.json"
    handoff_value = json.loads(facts.read_text())
    handoff_value["platform"] = "macos"
    write_json(handoff_facts, handoff_value)

    completed = run_handoff(volume, handoff_facts, verified, reconciliation)

    assert completed.returncode == 1
    assert "source seal or composite binding differs" in completed.stderr


def test_handoff_rejects_tampered_source_seal_or_composite(tmp_path: Path) -> None:
    for target in ("seal", "composite"):
        case = tmp_path / target
        case.mkdir()
        volume, facts, verified, reconciliation = fixture(case)
        proc_root = case / "proc"
        proc_root.mkdir()
        assert run_seal(volume, facts, verified, reconciliation, proc_root=proc_root).returncode == 0
        source_seal = reconciliation.parent / "source-seal.json"
        composite = reconciliation.parent / "composite-index.json"
        path = source_seal if target == "seal" else composite
        value = json.loads(path.read_text())
        value["raw_manifest_sha256" if target == "composite" else "manifest_sha256"] = "0" * 64
        write_json(path, value)
        macos_facts = case / "macos.json"
        facts_value = json.loads(facts.read_text())
        facts_value.update(
            {"platform": "macos", "stable_device_ids": ["VolumeUUID:ABCD-1234"], "mount_id": "disk9s1"}
        )
        write_json(macos_facts, facts_value)

        completed = run_handoff(volume, macos_facts, verified, reconciliation)

        assert completed.returncode == 1
        assert "source seal or composite binding differs" in completed.stderr
        assert not (
            volume / "manifests/storage-qualification/fixture/handoff-macos.json"
        ).exists()


def test_seal_production_defaults_reject_fixture_composition(tmp_path: Path) -> None:
    volume, facts, verified, reconciliation = fixture(tmp_path)
    proc_root = tmp_path / "proc"
    proc_root.mkdir()

    completed = run_seal(
        volume,
        facts,
        verified,
        reconciliation,
        proc_root=proc_root,
        production_defaults=True,
    )

    assert completed.returncode == 1
    assert "terminal reconciliation composition differs" in completed.stderr
    assert "'schedule_records': 661" in completed.stderr
    assert_unsealed(volume, reconciliation)


def test_production_defaults_validate_the_real_terminal_reconciliation() -> None:
    import importlib.util

    spec = importlib.util.spec_from_file_location("verify_storage_receipt", VERIFIER)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    reconciliation = json.loads(
        (
            ROOT
            / ".plans/rpi5-v5-enhanced-experiment-readiness/t19-terminal-reconciliation.json"
        ).read_text()
    )

    module.validate_reconciliation(reconciliation, module.EXPANDED_N5_COMPOSITION)


def test_seal_resumes_from_exact_partial_artifacts(tmp_path: Path) -> None:
    volume, facts, verified, reconciliation = fixture(tmp_path)
    proc_root = tmp_path / "proc"
    proc_root.mkdir()
    assert run_seal(volume, facts, verified, reconciliation, proc_root=proc_root).returncode == 0
    manifest, composite, source_seal = seal_paths(volume, reconciliation)
    manifest_hash = hashlib.sha256(manifest.read_bytes()).hexdigest()
    composite_hash = hashlib.sha256(composite.read_bytes()).hexdigest()
    source_seal.unlink()

    completed = run_seal(volume, facts, verified, reconciliation, proc_root=proc_root)

    assert completed.returncode == 0, completed.stderr
    assert hashlib.sha256(manifest.read_bytes()).hexdigest() == manifest_hash
    assert hashlib.sha256(composite.read_bytes()).hexdigest() == composite_hash
    assert source_seal.is_file()


def test_seal_rejects_divergent_partial_artifact_without_overwrite(tmp_path: Path) -> None:
    volume, facts, verified, reconciliation = fixture(tmp_path)
    proc_root = tmp_path / "proc"
    proc_root.mkdir()
    manifest, composite, source_seal = seal_paths(volume, reconciliation)
    manifest.write_text("0" * 64 + "  raw/unexpected.txt\n")
    before = manifest.read_bytes()

    completed = run_seal(volume, facts, verified, reconciliation, proc_root=proc_root)

    assert completed.returncode == 1
    assert "existing partial raw manifest differs" in completed.stderr
    assert manifest.read_bytes() == before
    assert not composite.exists()
    assert not source_seal.exists()


def test_seal_accepts_attempt_shaped_result_filename(tmp_path: Path) -> None:
    volume, facts, verified, reconciliation = fixture(tmp_path)
    artifact = (
        volume
        / "raw/e-val-1/rpi5-expanded-n5/run-01-attempt-01"
        / "per-run-percentiles/run-01-attempt-01.json"
    )
    write_json(artifact, {"p99_ns": 1})
    proc_root = tmp_path / "proc"
    proc_root.mkdir()

    completed = run_seal(volume, facts, verified, reconciliation, proc_root=proc_root)

    assert completed.returncode == 0, completed.stderr


def test_seal_rejects_symlinked_attempt_shaped_path(tmp_path: Path) -> None:
    volume, facts, verified, reconciliation = fixture(tmp_path)
    campaign = volume / "raw/e-val-1/rpi5-expanded-n5"
    (campaign / "run-02-attempt-01").symlink_to(campaign / "run-01-attempt-01")
    proc_root = tmp_path / "proc"
    proc_root.mkdir()

    completed = run_seal(volume, facts, verified, reconciliation, proc_root=proc_root)

    assert completed.returncode == 1
    assert "must not be linked" in completed.stderr
    assert_unsealed(volume, reconciliation)


def test_seal_writes_verified_manifest_composite_and_source_receipt(tmp_path: Path) -> None:
    volume, facts, verified, reconciliation = fixture(tmp_path)
    proc_root = tmp_path / "proc"
    proc_root.mkdir()

    completed = run_seal(volume, facts, verified, reconciliation, proc_root=proc_root)

    assert completed.returncode == 0, completed.stderr
    manifest = volume / "manifests/expanded-n5.sha256"
    composite = volume / "manifests/n5-batches/rpi5-expanded-n5/composite-index.json"
    seal = volume / "manifests/n5-batches/rpi5-expanded-n5/source-seal.json"
    assert manifest.is_file() and composite.is_file() and seal.is_file()
    lines = manifest.read_text().splitlines()
    assert len(lines) == 6
    assert all("  raw/" in line for line in lines)
    index = json.loads(composite.read_text())
    assert index["counts"] == {
        "aliases": 1,
        "qualified_prerequisites": 1,
        "schedule_records": 3,
        "selected_raw_leaves": 1,
        "unselected_attempts": 0,
        "unselected_failed_attempts": 0,
    }
    prerequisite = index["qualified_prerequisites"][0]
    assert prerequisite["result_key"] == (
        "e-host-thermal-storage/eight-phase-load-ladder/run-01"
    )
    assert prerequisite["source_leaf"] == (
        "raw/e-host-thermal-storage/host-characterization-fixture"
    )
    assert prerequisite["source_tag"] == "rpi5-final-rc-v6"
    assert prerequisite["source_git_sha"] == "d4f26e32e6601419a0768403b8a0728035ff1e67"
    assert set(prerequisite["source_artifacts"]) == {
        "host-load-ladder.json",
        "host-telemetry.csv",
        "kernel-io.log",
        "usb-integrity.json",
    }
    receipt = json.loads(seal.read_text())
    assert receipt["state"] == "source-host-sealed"
    assert receipt["manifest_sha256"] == hashlib.sha256(manifest.read_bytes()).hexdigest()
    assert receipt["expected_composition"] == {
        "aliases": 1,
        "qualified_prerequisites": 1,
        "schedule_records": 3,
        "selected_raw_leaves": 1,
        "unselected_attempts": 0,
        "unselected_failed_attempts": 0,
    }
    assert receipt["verification"]["mismatch_count"] == 0
    assert receipt["raw_open_mode"] == "read-only"
    assert receipt["raw_sync_completed_before_seal"] is True
    assert receipt["manifest_and_composite_sync_completed"] is True
    assert receipt["sync_required_before_unmount"] is True
    hashes = {path: hashlib.sha256(path.read_bytes()).hexdigest() for path in seal_paths(volume, reconciliation)}

    repeated = run_seal(volume, facts, verified, reconciliation, proc_root=proc_root)

    assert repeated.returncode == 1
    assert "sealed campaign artifact already exists" in repeated.stderr
    assert hashes == {
        path: hashlib.sha256(path.read_bytes()).hexdigest()
        for path in seal_paths(volume, reconciliation)
    }
