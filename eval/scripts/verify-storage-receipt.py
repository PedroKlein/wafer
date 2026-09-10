#!/usr/bin/env python3

from __future__ import annotations

import argparse
import hashlib
from collections import Counter
import json
import os
import plistlib
import re
import shlex
import subprocess
import sys
import time
import unicodedata
from pathlib import Path
from typing import Any

CHUNK = bytes(range(256)) * 4096
SAFE_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
SHA_LINE = re.compile(r"^([0-9a-f]{64})  (.+)$")
LAYOUT = ("raw", "manifests", "derived", "reports")
EXFAT_FORBIDDEN = set('<>:"\\|?*')
EXPANDED_N5_RELEASES = {
    "rpi5-final-rc-v10": "c121b49e0a5dfaee7822269fb838c415f8d29ba2",
    "rpi5-final-rc-v11": "b79b6858f3b40416b2f92c27d38213261f5a9a4c",
    "rpi5-final-rc-v12": "7074b33a71da8646e520b3a85fa77e9b83b8453e",
    "rpi5-final-rc-v13": "55b1a4e942c192908981a8aa970c180829fd6360",
}
EXPANDED_N5_CONTROLS = {
    "legacy-v10",
    "continuation-01",
    "continuation-03",
    "continuation-05",
    "continuation-08",
    "continuation-09",
    "continuation-10",
}
EXPANDED_N5_PREREQUISITE = "e-host-thermal-storage/eight-phase-load-ladder/run-01"
PREREQUISITE_RELEASE = {
    "release_tag": "rpi5-final-rc-v6",
    "wafer_git_sha": "d4f26e32e6601419a0768403b8a0728035ff1e67",
}
EXPANDED_N5_COMPOSITION = {
    "schedule_records": 661,
    "selected_raw_leaves": 623,
    "aliases": 37,
    "qualified_prerequisites": 1,
    "unselected_attempts": 4,
    "unselected_failed_attempts": 4,
}
EXFAT_RESERVED = {
    "CON",
    "PRN",
    "AUX",
    "NUL",
    *(f"COM{index}" for index in range(1, 10)),
    *(f"LPT{index}" for index in range(1, 10)),
}


def fail(message: str) -> None:
    raise ValueError(message)


def load_object(path: Path, label: str) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot read {label}: {error}") from error
    if not isinstance(value, dict):
        fail(f"{label} must be a JSON object")
    return value


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def ensure_inside(path: Path, root: Path, label: str) -> Path:
    resolved = path.resolve()
    root_resolved = root.resolve()
    if resolved != root_resolved and root_resolved not in resolved.parents:
        fail(f"{label} is outside the results root: {path}")
    current = path.absolute()
    boundary = root.absolute()
    while True:
        if current.is_symlink():
            fail(f"{label} must not use symlinks: {current}")
        if current == boundary:
            break
        if current == current.parent:
            fail(f"{label} is outside the results root: {path}")
        current = current.parent
    return resolved


def validate_layout(root: Path) -> None:
    if not root.is_dir() or root.is_symlink():
        fail(f"results root is absent, stale, or linked: {root}")
    seen: set[int] = set()
    for name in LAYOUT:
        path = root / name
        if not path.is_dir() or path.is_symlink():
            fail(f"results layout is incomplete or linked: {path}")
        device = path.stat().st_dev
        if device != root.stat().st_dev:
            fail(f"results layout crosses devices: {path}")
        seen.add(device)
    if len(seen) != 1:
        fail("results layout paths do not share one filesystem")


def validate_facts(
    facts: dict[str, Any],
    root: Path,
    *,
    expected_device_id: str | None,
    expected_uuid: str,
    min_free_bytes: int,
    corpus_bytes: int = 0,
) -> str:
    if facts.get("schema_version") != 1:
        fail("storage facts schema_version must be 1")
    mount_path = Path(str(facts.get("mount_path", "")))
    if mount_path.resolve() != root.resolve():
        fail(f"mount path differs: expected {root}, got {mount_path}")
    if int(facts.get("path_device", -1)) != root.stat().st_dev:
        fail("path identity differs from the mounted results filesystem")
    stable_ids = facts.get("stable_device_ids")
    if not isinstance(stable_ids, list) or not stable_ids:
        fail("stable device identifier is missing")
    if expected_device_id is not None and expected_device_id not in stable_ids:
        fail("stable device identifier differs from the approved device")
    if not expected_uuid or facts.get("uuid") != expected_uuid:
        fail("filesystem UUID differs from the approved volume")
    if facts.get("label") != "WAF_RESULTS":
        fail("volume label must be WAF_RESULTS")
    if str(facts.get("filesystem", "")).lower() != "exfat":
        fail("filesystem type must be exFAT")
    options = facts.get("mount_options")
    if not isinstance(options, list):
        fail("mount options are missing")
    if facts.get("read_write") is not True or "rw" not in options or "ro" in options:
        fail("results filesystem must be mounted read-write")
    available = int(facts.get("available_bytes", -1))
    if available < min_free_bytes + corpus_bytes:
        fail(
            "free-space margin is insufficient: "
            f"available={available}, required={min_free_bytes + corpus_bytes}"
        )
    mount_id = facts.get("mount_id")
    if not isinstance(mount_id, str) or not mount_id:
        fail("mount identity is missing")
    return mount_id


def write_pattern(path: Path, size: int, seed: int) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    remaining = size
    offset = seed % len(CHUNK)
    with path.open("xb") as stream:
        while remaining:
            chunk = CHUNK[offset:] + CHUNK[:offset]
            data = chunk[: min(remaining, len(chunk))]
            stream.write(data)
            remaining -= len(data)
            offset = (offset + 17) % len(CHUNK)
        stream.flush()
        os.fsync(stream.fileno())


def write_new_json(path: Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("x") as stream:
        json.dump(value, stream, indent=2, sort_keys=True)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())


def run_sync(root: Path) -> None:
    command = shlex.split(os.environ.get("WAFER_SYNC", "sync"))
    if not command:
        fail("sync command is empty")
    subprocess.run(command, cwd=root, check=True)


def corpus_manifest(root: Path, corpus: Path, destination: Path) -> tuple[int, int]:
    files = sorted(path for path in corpus.rglob("*") if path.is_file())
    byte_count = 0
    with destination.open("x") as stream:
        for path in files:
            relative = path.relative_to(root).as_posix()
            byte_count += path.stat().st_size
            stream.write(f"{sha256(path)}  {relative}\n")
        stream.flush()
        os.fsync(stream.fileno())
    return len(files), byte_count


def parse_manifest(path: Path, root: Path, prefix: str) -> dict[str, str]:
    entries: dict[str, str] = {}
    for line_number, line in enumerate(path.read_text().splitlines(), 1):
        match = SHA_LINE.fullmatch(line)
        if match is None:
            fail(f"manifest line {line_number} is malformed")
        digest, relative = match.groups()
        candidate = Path(relative)
        if candidate.is_absolute() or ".." in candidate.parts or candidate.as_posix() != relative:
            fail(f"manifest line {line_number} has an unsafe path")
        if not relative.startswith(prefix):
            fail(f"raw-path drift: manifest entry is outside {prefix}: {relative}")
        ensure_inside(root / candidate, root, "manifest entry")
        if relative in entries:
            fail(f"manifest contains duplicate path: {relative}")
        entries[relative] = digest
    if not entries:
        fail("manifest is empty")
    validate_portable_relative_paths(list(entries))
    return entries


def verify_manifest(
    root: Path, manifest: Path, *, prefix: str
) -> tuple[dict[str, int], list[str]]:
    entries = parse_manifest(manifest, root, prefix)
    expected_paths = set(entries)
    observed_paths = {
        path.relative_to(root).as_posix()
        for path in (root / prefix.rstrip("/")).rglob("*")
        if path.is_file()
    }
    missing = sorted(expected_paths - observed_paths)
    extra = sorted(observed_paths - expected_paths)
    mismatches: list[str] = []
    expected_bytes = 0
    observed_bytes = 0
    for relative in sorted(expected_paths & observed_paths):
        path = root / relative
        try:
            ensure_inside(path, root, "manifest entry")
            stat = path.stat()
            if stat.st_nlink > 1:
                fail(f"manifest entry must not be hardlinked: {relative}")
            observed_bytes += stat.st_size
            expected_bytes += stat.st_size
            if sha256(path) != entries[relative]:
                mismatches.append(relative)
        except (OSError, ValueError):
            mismatches.append(relative)
    counts = {
        "expected_files": len(expected_paths),
        "observed_files": len(observed_paths),
        "expected_bytes": expected_bytes,
        "observed_bytes": observed_bytes,
        "missing_count": len(missing),
        "extra_count": len(extra),
        "mismatch_count": len(mismatches),
    }
    return counts, missing + extra + mismatches


def base_receipt(facts: dict[str, Any], root: Path, stable_id: str) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "generated_at_unix_ns": time.time_ns(),
        "results_root": str(root.resolve()),
        "device": {
            "approved_stable_device_id": stable_id,
            "observed_stable_device_ids": facts["stable_device_ids"],
            "source": facts.get("source"),
        },
        "filesystem": {
            "uuid": facts["uuid"],
            "label": facts["label"],
            "type": str(facts["filesystem"]).lower(),
        },
        "mount": {
            "path": str(root.resolve()),
            "mount_id": facts["mount_id"],
            "options": facts["mount_options"],
            "read_write": facts["read_write"],
            "available_bytes": facts["available_bytes"],
            "path_device": facts["path_device"],
        },
    }


def prepare(args: argparse.Namespace) -> None:
    root = args.results_root.resolve()
    validate_layout(root)
    facts = load_object(args.facts_json, "storage facts")
    total_bytes = args.large_bytes + args.small_files * args.small_bytes
    validate_facts(
        facts,
        root,
        expected_device_id=args.expected_device_id,
        expected_uuid=args.expected_uuid,
        min_free_bytes=args.min_free_bytes,
        corpus_bytes=total_bytes,
    )
    if SAFE_ID.fullmatch(args.qualification_id) is None:
        fail("qualification ID is not portable")
    qualification = root / "manifests/storage-qualification" / args.qualification_id
    if qualification.exists():
        fail(f"qualification path already exists: {qualification}")
    corpus = qualification / "corpus"
    write_pattern(corpus / "large.bin", args.large_bytes, 1)
    for index in range(args.small_files):
        write_pattern(corpus / "small" / f"file-{index:04d}.bin", args.small_bytes, index + 2)
    manifest = qualification / "corpus.sha256"
    file_count, byte_count = corpus_manifest(root, corpus, manifest)
    run_sync(root)
    receipt = base_receipt(facts, root, args.expected_device_id)
    receipt.update(
        {
            "state": "prepared-awaiting-unmount-remount",
            "qualification_id": args.qualification_id,
            "minimum_free_bytes_after_corpus": args.min_free_bytes,
            "corpus": {
                "root_relative": corpus.relative_to(root).as_posix(),
                "manifest_relative": manifest.relative_to(root).as_posix(),
                "manifest_sha256": sha256(manifest),
                "file_count": file_count,
                "byte_count": byte_count,
                "large_file_bytes": args.large_bytes,
                "small_file_count": args.small_files,
                "small_file_bytes": args.small_bytes,
            },
            "sync_completed": True,
            "operator_action_required": "stop writers; sync; unmount; remount; collect fresh facts",
        }
    )
    write_new_json(qualification / "prepared.json", receipt)
    print(f"storage qualification prepared: {qualification / 'prepared.json'}")


def verify_remount(args: argparse.Namespace) -> None:
    root = args.results_root.resolve()
    validate_layout(root)
    prepared_path = ensure_inside(args.prepared_receipt, root / "manifests", "prepared receipt")
    prepared = load_object(prepared_path, "prepared receipt")
    if prepared.get("state") != "prepared-awaiting-unmount-remount":
        fail("prepared receipt has the wrong state")
    facts = load_object(args.facts_json, "storage facts")
    stable_id = str(prepared["device"]["approved_stable_device_id"])
    expected_uuid = str(prepared["filesystem"]["uuid"])
    mount_id = validate_facts(
        facts,
        root,
        expected_device_id=stable_id,
        expected_uuid=expected_uuid,
        min_free_bytes=int(prepared["minimum_free_bytes_after_corpus"]),
    )
    if mount_id == prepared["mount"]["mount_id"]:
        fail("safe unmount/remount is not proven: mount identity did not change")
    manifest = ensure_inside(root / prepared["corpus"]["manifest_relative"], root, "manifest")
    if sha256(manifest) != prepared["corpus"]["manifest_sha256"]:
        fail("corpus manifest changed before verification")
    corpus_prefix = prepared["corpus"]["root_relative"].rstrip("/") + "/"
    counts, failures = verify_manifest(root, manifest, prefix=corpus_prefix)
    counts["expected_bytes"] = int(prepared["corpus"]["byte_count"])
    if failures or counts["expected_files"] != prepared["corpus"]["file_count"]:
        fail(f"checksum verification failed: counts={counts}, paths={failures[:5]}")
    if counts["observed_bytes"] != counts["expected_bytes"]:
        fail("checksum verification failed: byte count differs")
    receipt = base_receipt(facts, root, stable_id)
    receipt.update(
        {
            "state": "verified-after-remount",
            "qualification_id": prepared["qualification_id"],
            "prepared_receipt_sha256": sha256(prepared_path),
            "corpus_manifest_sha256": sha256(manifest),
            "safe_unmount_remount_verified": True,
            "verification": counts,
        }
    )
    destination = prepared_path.with_name("verified.json")
    write_new_json(destination, receipt)
    print(f"storage qualification verified: {destination}")


def validate_handoff_platform(facts: dict[str, Any], host: str) -> None:
    expected = "macos" if host == "macos" else "linux"
    if str(facts.get("platform", "")).lower() != expected:
        fail(f"handoff host facts differ: expected {expected}")


def active_evidence_writers(proc_root: Path, raw_root: Path) -> list[str]:
    patterns = (
        "n5_supervisor.py",
        "n5_runner.py",
        "canonical_runner.py",
        "run-experiment.sh",
        "wafer-loadgen",
        "/target/release/wafer ",
        "kuiperd",
    )
    active = []
    if not proc_root.is_dir():
        fail(f"process filesystem is unavailable: {proc_root}")
    raw_resolved = raw_root.resolve()
    for entry in proc_root.iterdir():
        if not entry.name.isdigit():
            continue
        try:
            command_line = (entry / "cmdline").read_bytes().replace(b"\0", b" ").decode(errors="replace")
        except OSError:
            command_line = ""
        if any(pattern in command_line for pattern in patterns):
            active.append(f"{entry.name}:{command_line.strip()}")
        try:
            descriptors = list((entry / "fd").iterdir())
        except OSError:
            continue
        for descriptor in descriptors:
            try:
                target = Path(os.readlink(descriptor).removesuffix(" (deleted)"))
                target_resolved = target.resolve(strict=False)
            except OSError:
                continue
            if target_resolved != raw_resolved and raw_resolved not in target_resolved.parents:
                continue
            try:
                info = (entry / "fdinfo" / descriptor.name).read_text()
                match = re.search(r"^flags:\s*([0-7]+)$", info, flags=re.MULTILINE)
            except OSError:
                match = None
            if match is not None and int(match.group(1), 8) & os.O_ACCMODE == os.O_RDONLY:
                continue
            active.append(
                f"{entry.name}:{command_line.strip()} fd={descriptor.name} target={target}"
            )
    return sorted(set(active))


def safe_source_host_state(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("boot_id"), str)
        and bool(value["boot_id"])
        and value.get("throttled") == "throttled=0x0"
        and isinstance(value.get("temperature_millicelsius"), int)
        and value["temperature_millicelsius"] < 75_000
        and value.get("kernel_io_errors") == 0
    )


def seal_host_state(expected_boot_id: str) -> dict[str, Any]:
    boot_id = Path(
        os.environ.get("WAFER_BOOT_ID_PATH", "/proc/sys/kernel/random/boot_id")
    ).read_text().strip()
    if boot_id != expected_boot_id:
        fail(f"boot ID differs: expected {expected_boot_id}, got {boot_id}")
    throttled = subprocess.run(
        [*command("WAFER_VCGENCMD", "vcgencmd"), "get_throttled"],
        capture_output=True,
        check=True,
        text=True,
    ).stdout.strip()
    if throttled != "throttled=0x0":
        fail(f"host throttle state is unsafe: {throttled}")
    temperature = int(
        Path(
            os.environ.get(
                "WAFER_THERMAL_PATH", "/sys/class/thermal/thermal_zone0/temp"
            )
        ).read_text().strip()
    )
    if temperature >= 75_000:
        fail(f"host temperature is unsafe: {temperature}")
    kernel_log = subprocess.run(
        [*command("WAFER_JOURNALCTL", "journalctl"), "-b", "-k", "--no-pager"],
        capture_output=True,
        check=True,
        text=True,
    ).stdout
    errors = len(
        re.findall(
            r"I/O error|Buffer I/O error|blk_update_request|uas_eh_abort_handler|"
            r"reset SuperSpeed USB device|exfat.*error",
            kernel_log,
            flags=re.IGNORECASE,
        )
    )
    if errors:
        fail(f"kernel I/O errors are present: {errors}")
    return {
        "boot_id": boot_id,
        "throttled": throttled,
        "temperature_millicelsius": temperature,
        "kernel_io_errors": errors,
    }


def validate_portable_relative_paths(paths: list[str]) -> None:
    names: dict[str, str] = {}
    for relative in paths:
        candidate = Path(relative)
        if candidate.is_absolute() or not candidate.parts or ".." in candidate.parts:
            fail(f"raw manifest has an unsafe path: {relative}")
        for segment in candidate.parts:
            utf16_units = len(segment.encode("utf-16-le")) // 2
            base = segment.split(".", 1)[0].upper()
            if (
                segment in {"", ".", ".."}
                or segment.endswith((".", " "))
                or any(ord(character) < 32 or character in EXFAT_FORBIDDEN for character in segment)
                or utf16_units > 255
                or base in EXFAT_RESERVED
            ):
                fail(f"raw manifest path is not exFAT-safe: {relative}")
        folded = unicodedata.normalize("NFC", relative).casefold()
        if folded in names and names[folded] != relative:
            fail(
                "raw manifest has a case or normalization collision: "
                f"{names[folded]} and {relative}"
            )
        names[folded] = relative


def raw_manifest_bytes(root: Path) -> tuple[bytes, dict[str, int]]:
    raw = root / "raw"
    files = []
    byte_count = 0
    for path in sorted(raw.rglob("*")):
        if path.is_symlink():
            fail(f"raw manifest path must not be linked: {path}")
        if not path.is_file():
            continue
        ensure_inside(path, raw, "raw manifest entry")
        stat = path.stat()
        if stat.st_nlink > 1:
            fail(f"raw manifest entry must not be hardlinked: {path}")
        relative = path.relative_to(root).as_posix()
        files.append((relative, stat.st_size, stat.st_mtime_ns, sha256(path)))
        byte_count += stat.st_size
    if not files:
        fail("raw tree is empty")
    validate_portable_relative_paths([relative for relative, _, _, _ in files])
    content = "".join(f"{digest}  {relative}\n" for relative, _, _, digest in files).encode()
    path_size_sha256 = hashlib.sha256(
        b"".join(f"{relative}\0{size}\0".encode() for relative, size, _, _ in files)
    ).hexdigest()
    path_size_mtime_sha256 = hashlib.sha256(
        b"".join(f"{relative}\0{size}\0{mtime}\0".encode() for relative, size, mtime, _ in files)
    ).hexdigest()
    return content, {
        "file_count": len(files),
        "byte_count": byte_count,
        "path_size_sha256": path_size_sha256,
        "source_path_size_mtime_sha256": path_size_mtime_sha256,
    }


def composition_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--expected-records", type=int, default=EXPANDED_N5_COMPOSITION["schedule_records"]
    )
    parser.add_argument(
        "--expected-raw", type=int, default=EXPANDED_N5_COMPOSITION["selected_raw_leaves"]
    )
    parser.add_argument(
        "--expected-aliases", type=int, default=EXPANDED_N5_COMPOSITION["aliases"]
    )
    parser.add_argument(
        "--expected-prerequisites",
        type=int,
        default=EXPANDED_N5_COMPOSITION["qualified_prerequisites"],
    )
    parser.add_argument(
        "--expected-unselected",
        type=int,
        default=EXPANDED_N5_COMPOSITION["unselected_attempts"],
    )
    parser.add_argument(
        "--expected-unselected-failed",
        type=int,
        default=EXPANDED_N5_COMPOSITION["unselected_failed_attempts"],
    )
    parser.add_argument("--fixture-composition", action="store_true", help=argparse.SUPPRESS)


def expected_composition(args: argparse.Namespace) -> dict[str, int]:
    expected = {
        "schedule_records": args.expected_records,
        "selected_raw_leaves": args.expected_raw,
        "aliases": args.expected_aliases,
        "qualified_prerequisites": args.expected_prerequisites,
        "unselected_attempts": args.expected_unselected,
        "unselected_failed_attempts": args.expected_unselected_failed,
    }
    if expected != EXPANDED_N5_COMPOSITION and not args.fixture_composition:
        fail("non-production composition requires --fixture-composition")
    return expected


def validate_reconciliation(
    value: dict[str, Any], expected: dict[str, int]
) -> None:
    counts = value.get("counts")
    if (
        value.get("schema_version") != 1
        or value.get("status") != "passed"
        or value.get("classification") != "diagnostic-expanded-n5-terminal-reconciliation"
        or value.get("campaign_started") is not False
        or value.get("thesis_evidence") is not False
        or value.get("n30_admitted") is not False
        or not isinstance(counts, dict)
        or int(counts.get("schedule_records", 0))
        != int(counts.get("selected_raw_leaves", -1))
        + int(counts.get("aliases", -1))
        + int(counts.get("qualified_prerequisites", -1))
    ):
        fail("terminal reconciliation is invalid")
    selected = value.get("selected")
    aliases = value.get("aliases")
    prerequisites = value.get("qualified_prerequisites")
    unselected = value.get("unselected_attempts")
    if (
        not isinstance(selected, dict)
        or len(selected) != counts["selected_raw_leaves"]
        or not isinstance(aliases, dict)
        or len(aliases) != counts["aliases"]
        or not isinstance(prerequisites, list)
        or len(prerequisites) != counts["qualified_prerequisites"]
        or not isinstance(unselected, list)
        or len(unselected) != counts["unselected_attempts"]
    ):
        fail("terminal reconciliation populations differ from counts")
    logical_keys = [*selected, *aliases, *prerequisites]
    if len(logical_keys) != len(set(logical_keys)) or len(logical_keys) != counts["schedule_records"]:
        fail("terminal reconciliation logical keys are not an exact disjoint set")
    if any(counts.get(name) != count for name, count in expected.items()):
        fail(
            "terminal reconciliation composition differs: "
            f"expected={expected}, observed={counts}"
        )
    if any(item.get("result_key") not in selected for item in unselected):
        fail("unselected attempt does not belong to a selected result key")
    if any(item.get("status") not in {"failed", "interrupted"} for item in unselected):
        fail("unselected attempt must be failed or interrupted")
    if counts.get("unselected_failed_attempts") != sum(
        item.get("status") == "failed" for item in unselected
    ):
        fail("unselected failed-attempt count differs")
    if prerequisites != [EXPANDED_N5_PREREQUISITE]:
        fail("qualified prerequisite differs from B00")
    release_counts: Counter[str] = Counter()
    control_counts: Counter[str] = Counter()
    for key, item in selected.items():
        release_tag = item.get("release_tag")
        if EXPANDED_N5_RELEASES.get(release_tag) != item.get("wafer_git_sha"):
            fail(f"selected release lineage differs: {key}")
        control_generation = item.get("control_generation")
        if control_generation not in EXPANDED_N5_CONTROLS:
            fail(f"selected control generation differs: {key}")
        release_counts[release_tag] += 1
        control_counts[control_generation] += 1
    if dict(sorted(release_counts.items())) != value.get("release_composition"):
        fail("release composition differs from selected records")
    if dict(sorted(control_counts.items())) != value.get("control_composition"):
        fail("control composition differs from selected records")


def build_prerequisite_evidence(
    root: Path, reconciliation: dict[str, Any]
) -> list[dict[str, Any]]:
    batch_id = str(reconciliation["batch_id"])
    ledger = root / "manifests/n5-batches" / f"rpi5-{batch_id}"
    terminal_path = ledger / "batches/B00-host-prerequisite/terminal.json"
    terminal_digest = sha256(terminal_path)
    if terminal_digest != reconciliation.get("batch_terminal_sha256", {}).get(
        "B00-host-prerequisite"
    ):
        fail("B00 terminal digest differs from reconciliation")
    terminal = load_object(terminal_path, "B00 terminal")
    if (
        terminal.get("status") != "passed"
        or terminal.get("disposition") != "qualified-prerequisite-not-reexecuted"
        or terminal.get("schedule_result_keys") != [EXPANDED_N5_PREREQUISITE]
    ):
        fail("B00 terminal is invalid")
    admission_path = ledger / "usb-host-admission-receipt.json"
    admission_digest = sha256(admission_path)
    if admission_digest != terminal.get("admission_receipt_sha256"):
        fail("B00 admission receipt digest differs")
    admission = load_object(admission_path, "B00 admission receipt")
    host = admission.get("host_characterization")
    if (
        admission.get("admission", {}).get("expanded_n5") is not True
        or admission.get("admission", {}).get("n30") is not False
        or not isinstance(host, dict)
        or host.get("diagnostic_admission") is not True
        or host.get("final_admission") is not False
        or host.get("kernel_io_error_count") != 0
        or admission.get("volume", {}).get("uuid") != "EFBF-159A"
        or admission.get("volume", {}).get("label") != "WAF_RESULTS"
        or admission.get("volume", {}).get("filesystem") != "exfat"
    ):
        fail("B00 admission receipt is invalid")
    source_leaf = root / "raw/e-host-thermal-storage" / str(host.get("session_id", ""))
    if not source_leaf.is_dir() or source_leaf.is_symlink():
        fail("B00 source leaf is missing or linked")
    required = ("host-load-ladder.json", "host-telemetry.csv", "kernel-io.log", "usb-integrity.json")
    artifacts = {}
    for name in required:
        path = ensure_inside(source_leaf / name, root / "raw", f"B00 {name}")
        if not path.is_file():
            fail(f"B00 source artifact is missing: {name}")
        artifacts[name] = sha256(path)
    if artifacts["host-load-ladder.json"] != host.get("sha256"):
        fail("B00 host characterization digest differs")
    host_receipt = load_object(source_leaf / "host-load-ladder.json", "B00 host receipt")
    source_state = host_receipt.get("source_state")
    if (
        not isinstance(source_state, dict)
        or source_state.get("git_dirty") is not False
        or source_state.get("git_sha") != PREREQUISITE_RELEASE["wafer_git_sha"]
        or source_state.get("git_tags") != [PREREQUISITE_RELEASE["release_tag"]]
    ):
        fail("B00 source lineage differs")
    return [
        {
            "result_key": EXPANDED_N5_PREREQUISITE,
            "source_leaf": source_leaf.relative_to(root).as_posix(),
            "source_tag": PREREQUISITE_RELEASE["release_tag"],
            "source_git_sha": PREREQUISITE_RELEASE["wafer_git_sha"],
            "control_generation": "B00-prerequisite",
            "source_artifacts": artifacts,
            "batch_terminal_relative": terminal_path.relative_to(root).as_posix(),
            "batch_terminal_sha256": terminal_digest,
            "admission_receipt_relative": admission_path.relative_to(root).as_posix(),
            "admission_receipt_sha256": admission_digest,
        }
    ]


def build_composite(
    root: Path,
    reconciliation: dict[str, Any],
    reconciliation_sha256: str,
    manifest_sha256: str,
) -> dict[str, Any]:
    selected_paths = [str(item.get("path", "")) for item in reconciliation["selected"].values()]
    unselected_paths = [str(item.get("path", "")) for item in reconciliation["unselected_attempts"]]
    if len(selected_paths) != len(set(selected_paths)):
        fail("selected raw path is pooled by multiple logical records")
    if len(unselected_paths) != len(set(unselected_paths)):
        fail("unselected raw path is duplicated")
    if set(selected_paths) & set(unselected_paths):
        fail("selected and unselected raw paths overlap")
    for key, selected in reconciliation["selected"].items():
        path = ensure_inside(root / str(selected.get("path", "")), root / "raw", "selected leaf")
        if not path.is_dir() or path.is_symlink():
            fail(f"selected leaf is missing or linked: {key}")
        for name, field in (("metadata.json", "metadata_sha256"), ("canonical-status.json", "status_sha256")):
            artifact = path / name
            if sha256(artifact) != selected.get(field):
                fail(f"selected leaf digest differs: {key} {name}")
        if load_object(path / "canonical-status.json", "selected status").get("status") != "passed":
            fail(f"selected leaf is not passed: {key}")
    for key, alias in reconciliation["aliases"].items():
        path = ensure_inside(root / str(alias.get("path", "")), root / "manifests", "alias receipt")
        if sha256(path) != alias.get("receipt_sha256"):
            fail(f"alias receipt digest differs: {key}")
        source_key = str(alias.get("source_result_key", ""))
        selected_source = reconciliation["selected"].get(source_key)
        if (
            not isinstance(selected_source, dict)
            or selected_source.get("path") != alias.get("source_leaf")
            or selected_source.get("status_sha256") != alias.get("source_status_sha256")
        ):
            fail(f"alias source selection differs: {key}")
        source = ensure_inside(root / str(alias.get("source_leaf", "")), root / "raw", "alias source")
        if sha256(source / "canonical-status.json") != alias.get("source_status_sha256"):
            fail(f"alias source status digest differs: {key}")
    for item in reconciliation["unselected_attempts"]:
        path = ensure_inside(
            root / str(item.get("path", "")), root / "raw", "unselected attempt"
        )
        if not path.is_dir() or path.is_symlink():
            fail(f"unselected attempt is missing or linked: {path}")
        status = path / "canonical-status.json"
        if sha256(status) != item.get("status_sha256"):
            fail(f"unselected attempt status digest differs: {path}")
        if load_object(status, "unselected attempt status").get("status") != item.get("status"):
            fail(f"unselected attempt status differs: {path}")
    expected_attempts = {
        str(item["path"])
        for item in reconciliation["selected"].values()
    } | {
        str(item["path"])
        for item in reconciliation["unselected_attempts"]
    }
    campaign_name = f"rpi5-{reconciliation['batch_id']}"
    observed_attempts = set()
    for campaign in (root / "raw").glob(f"*/{campaign_name}"):
        for path in campaign.rglob("run-*-attempt-*"):
            if path.is_symlink() or not path.is_dir():
                fail(f"campaign attempt path is malformed or linked: {path}")
            observed_attempts.add(path.relative_to(root).as_posix())
    if observed_attempts != expected_attempts:
        fail("campaign attempt paths differ from terminal reconciliation")
    counts = reconciliation["counts"]
    return {
        "schema_version": 1,
        "classification": "diagnostic-expanded-n5-composite",
        "batch_id": reconciliation["batch_id"],
        "campaign_started": False,
        "thesis_evidence": False,
        "n30_admitted": False,
        "selection_rule": "exact terminal reconciliation selection; no attempt pooling",
        "terminal_reconciliation_sha256": reconciliation_sha256,
        "raw_manifest_sha256": manifest_sha256,
        "schedule_sha256": reconciliation["schedule_sha256"],
        "supervisor_terminal_sha256": reconciliation["supervisor_terminal_sha256"],
        "counts": {
            "schedule_records": counts["schedule_records"],
            "selected_raw_leaves": counts["selected_raw_leaves"],
            "aliases": counts["aliases"],
            "qualified_prerequisites": counts["qualified_prerequisites"],
            "unselected_attempts": counts["unselected_attempts"],
            "unselected_failed_attempts": counts["unselected_failed_attempts"],
        },
        "release_composition": reconciliation["release_composition"],
        "control_composition": reconciliation["control_composition"],
        "selected": reconciliation["selected"],
        "aliases": reconciliation["aliases"],
        "qualified_prerequisites": build_prerequisite_evidence(root, reconciliation),
        "unselected_attempts": reconciliation["unselected_attempts"],
    }


def verify_sealed_evidence(
    root: Path,
    verified_path: Path,
    manifest: Path,
    source_seal_path: Path,
    composite_path: Path,
    expected: dict[str, int],
) -> tuple[dict[str, int], dict[str, str]]:
    verified = load_object(verified_path, "verified receipt")
    source_seal = load_object(source_seal_path, "source seal")
    composite = load_object(composite_path, "composite index")
    manifest_sha256 = sha256(manifest)
    counts, failures = verify_manifest(root, manifest, prefix="raw/")
    if failures:
        fail(f"raw manifest verification failed: counts={counts}, paths={failures[:5]}")
    stable_id = str(verified["device"]["approved_stable_device_id"])
    if (
        source_seal.get("state") != "source-host-sealed"
        or source_seal.get("host") != "pi"
        or not safe_source_host_state(source_seal.get("host_state"))
        or source_seal.get("raw_open_mode") != "read-only"
        or source_seal.get("raw_sync_completed_before_seal") is not True
        or source_seal.get("manifest_and_composite_sync_completed") is not True
        or source_seal.get("sync_required_before_unmount") is not True
        or source_seal.get("execution_mode")
        != ("fixture-synthetic" if expected != EXPANDED_N5_COMPOSITION else "hardware-production")
        or source_seal.get("expected_composition") != expected
        or source_seal.get("tool_sha256") != sha256(Path(__file__).resolve())
        or source_seal.get("verified_receipt_sha256") != sha256(verified_path)
        or source_seal.get("device", {}).get("approved_stable_device_id") != stable_id
        or source_seal.get("filesystem", {}).get("uuid") != verified["filesystem"]["uuid"]
        or source_seal.get("filesystem", {}).get("label") != "WAF_RESULTS"
        or source_seal.get("filesystem", {}).get("type") != "exfat"
        or source_seal.get("manifest_relative") != manifest.relative_to(root).as_posix()
        or source_seal.get("manifest_sha256") != manifest_sha256
        or source_seal.get("composite_relative") != composite_path.relative_to(root).as_posix()
        or source_seal.get("composite_sha256") != sha256(composite_path)
        or composite.get("batch_id") != source_seal.get("batch_id")
        or composite.get("raw_manifest_sha256") != manifest_sha256
    ):
        fail("source seal or composite binding differs")
    reconciliation_path = ensure_inside(
        root / str(source_seal.get("terminal_reconciliation_relative", "")),
        root / "manifests",
        "terminal reconciliation",
    )
    reconciliation_digest = str(source_seal.get("terminal_reconciliation_sha256", ""))
    if sha256(reconciliation_path) != reconciliation_digest:
        fail("terminal reconciliation digest differs from source seal")
    reconciliation = load_object(reconciliation_path, "terminal reconciliation")
    validate_reconciliation(reconciliation, expected)
    if (
        reconciliation.get("batch_id") != source_seal.get("batch_id")
        or reconciliation.get("supervisor_terminal_sha256")
        != source_seal.get("supervisor_terminal_sha256")
        or reconciliation.get("final_health", {}).get("boot_id")
        != source_seal.get("host_state", {}).get("boot_id")
    ):
        fail("source seal differs from terminal reconciliation")
    expected_composite = build_composite(
        root, reconciliation, reconciliation_digest, manifest_sha256
    )
    if composite != expected_composite:
        fail("composite index differs from current selected evidence")
    _, current_raw_tree = raw_manifest_bytes(root)
    source_raw_tree = source_seal.get("raw_tree")
    portable_fields = ("file_count", "byte_count", "path_size_sha256")
    if (
        not isinstance(source_raw_tree, dict)
        or any(source_raw_tree.get(field) != current_raw_tree[field] for field in portable_fields)
        or source_seal.get("verification") != counts
    ):
        fail("raw tree differs from Pi source seal")
    return counts, {
        "batch_id": str(source_seal["batch_id"]),
        "source_seal_relative": source_seal_path.relative_to(root).as_posix(),
        "source_seal_sha256": sha256(source_seal_path),
        "composite_relative": composite_path.relative_to(root).as_posix(),
        "composite_sha256": sha256(composite_path),
    }


def seal(args: argparse.Namespace) -> None:
    root = args.results_root.resolve()
    validate_layout(root)
    facts = load_object(args.facts_json, "storage facts")
    verified_path = ensure_inside(args.verified_receipt, root / "manifests", "verified receipt")
    verified = load_object(verified_path, "verified receipt")
    if verified.get("state") != "verified-after-remount":
        fail("verified receipt has the wrong state")
    stable_id = str(verified["device"]["approved_stable_device_id"])
    validate_facts(
        facts,
        root,
        expected_device_id=stable_id,
        expected_uuid=str(verified["filesystem"]["uuid"]),
        min_free_bytes=0,
    )
    active = active_evidence_writers(
        Path(os.environ.get("WAFER_PROC_ROOT", "/proc")), root / "raw"
    )
    if active:
        fail(f"evidence writers are active: {active[:5]}")
    reconciliation_path = ensure_inside(
        args.terminal_reconciliation, root / "manifests", "terminal reconciliation"
    )
    if sha256(reconciliation_path) != args.expected_reconciliation_sha256:
        fail("terminal reconciliation digest differs")
    reconciliation = load_object(reconciliation_path, "terminal reconciliation")
    expected = expected_composition(args)
    validate_reconciliation(reconciliation, expected)
    final_health = reconciliation.get("final_health")
    if not isinstance(final_health, dict) or not isinstance(final_health.get("boot_id"), str):
        fail("terminal reconciliation final health is invalid")
    host_state = seal_host_state(final_health["boot_id"])
    batch_id = str(reconciliation["batch_id"])
    ledger = root / "manifests/n5-batches" / f"rpi5-{batch_id}"
    supervisor = load_object(ledger / "supervisor-terminal.json", "supervisor terminal")
    if supervisor.get("status") != "passed" or supervisor.get("batch_id") != batch_id:
        fail("campaign supervisor terminal is not passed")
    if sha256(ledger / "supervisor-terminal.json") != reconciliation["supervisor_terminal_sha256"]:
        fail("supervisor terminal digest differs from reconciliation")

    manifest = root / "manifests/expanded-n5.sha256"
    composite_path = ledger / "composite-index.json"
    seal_path = ledger / "source-seal.json"
    existing = [path.exists() for path in (manifest, composite_path, seal_path)]
    if seal_path.exists():
        if not all(existing):
            fail("sealed campaign artifacts are incomplete")
        verify_sealed_evidence(
            root, verified_path, manifest, seal_path, composite_path, expected
        )
        fail(
            "sealed campaign artifact already exists and verifies: "
            f"manifest_sha256={sha256(manifest)}"
        )
    if composite_path.exists() and not manifest.exists():
        fail("sealed campaign artifacts are incomplete")
    manifest_content, before = raw_manifest_bytes(root)
    manifest_content_again, before_again = raw_manifest_bytes(root)
    if manifest_content != manifest_content_again or before != before_again:
        fail("raw tree changed while building the manifest")
    manifest_sha256 = hashlib.sha256(manifest_content).hexdigest()
    composite = build_composite(
        root, reconciliation, args.expected_reconciliation_sha256, manifest_sha256
    )
    run_sync(root)
    manifest_content_after_sync, final = raw_manifest_bytes(root)
    if manifest_content_after_sync != manifest_content or final != before:
        fail("raw tree changed during source sealing")
    manifest.parent.mkdir(parents=True, exist_ok=True)
    if manifest.exists():
        if manifest.read_bytes() != manifest_content:
            fail("existing partial raw manifest differs")
    else:
        with manifest.open("xb") as stream:
            stream.write(manifest_content)
            stream.flush()
            os.fsync(stream.fileno())
    counts, failures = verify_manifest(root, manifest, prefix="raw/")
    if failures:
        fail(f"raw manifest verification failed: counts={counts}, paths={failures[:5]}")
    if composite_path.exists():
        if load_object(composite_path, "composite index") != composite:
            fail("existing partial composite index differs")
    else:
        write_new_json(composite_path, composite)
    run_sync(root)
    active = active_evidence_writers(
        Path(os.environ.get("WAFER_PROC_ROOT", "/proc")), root / "raw"
    )
    final_content, final = raw_manifest_bytes(root)
    if active or final_content != manifest_content or final != before:
        fail("raw tree or writer state changed after sealing artifacts")
    if (
        sha256(reconciliation_path) != args.expected_reconciliation_sha256
        or sha256(ledger / "supervisor-terminal.json")
        != reconciliation["supervisor_terminal_sha256"]
        or sha256(manifest) != manifest_sha256
        or load_object(composite_path, "composite index")
        != build_composite(
            root,
            reconciliation,
            args.expected_reconciliation_sha256,
            manifest_sha256,
        )
    ):
        fail("campaign evidence changed before source seal creation")
    host_state = seal_host_state(final_health["boot_id"])
    receipt = base_receipt(facts, root, stable_id)
    receipt.update(
        {
            "state": "source-host-sealed",
            "host": "pi",
            "execution_mode": (
                "fixture-synthetic" if expected != EXPANDED_N5_COMPOSITION else "hardware-production"
            ),
            "tool_sha256": sha256(Path(__file__).resolve()),
            "batch_id": batch_id,
            "verified_receipt_sha256": sha256(verified_path),
            "expected_composition": expected,
            "terminal_reconciliation_relative": reconciliation_path.relative_to(root).as_posix(),
            "terminal_reconciliation_sha256": args.expected_reconciliation_sha256,
            "manifest_relative": manifest.relative_to(root).as_posix(),
            "manifest_sha256": manifest_sha256,
            "composite_relative": composite_path.relative_to(root).as_posix(),
            "composite_sha256": sha256(composite_path),
            "supervisor_terminal_sha256": sha256(ledger / "supervisor-terminal.json"),
            "host_state": host_state,
            "raw_open_mode": "read-only",
            "raw_sync_completed_before_seal": True,
            "manifest_and_composite_sync_completed": True,
            "sync_required_before_unmount": True,
            "raw_tree": final,
            "verification": counts,
        }
    )
    write_new_json(seal_path, receipt)
    print(f"source host sealed: {seal_path}")


def handoff(args: argparse.Namespace) -> None:
    root = args.results_root.resolve()
    validate_layout(root)
    verified_path = ensure_inside(args.verified_receipt, root / "manifests", "verified receipt")
    verified = load_object(verified_path, "verified receipt")
    if verified.get("state") != "verified-after-remount":
        fail("verified receipt has the wrong state")
    facts = load_object(args.facts_json, "storage facts")
    validate_handoff_platform(facts, args.host)
    stable_id = str(verified["device"]["approved_stable_device_id"])
    validate_facts(
        facts,
        root,
        expected_device_id=stable_id if args.host == "jetson" else None,
        expected_uuid=str(verified["filesystem"]["uuid"]),
        min_free_bytes=0,
    )
    manifest = ensure_inside(args.manifest, root / "manifests", "raw manifest")
    before_digest = sha256(manifest)
    counts, failures = verify_manifest(root, manifest, prefix="raw/")
    if failures:
        fail(f"raw manifest verification failed: counts={counts}, paths={failures[:5]}")
    output = args.analysis_output.resolve()
    derived = (root / "derived").resolve()
    reports = (root / "reports").resolve()
    if not (output == derived or derived in output.parents or output == reports or reports in output.parents):
        fail("analysis output must be beneath derived/ or reports/, never raw/")
    if sha256(manifest) != before_digest:
        fail("raw manifest changed during read-only verification")
    sealed_evidence: dict[str, str] = {}
    if (args.source_seal is None) != (args.composite is None):
        fail("source seal and composite must be supplied together")
    if args.source_seal is not None and args.composite is not None:
        source_seal_path = ensure_inside(args.source_seal, root / "manifests", "source seal")
        composite_path = ensure_inside(args.composite, root / "manifests", "composite index")
        sealed_counts, sealed_evidence = verify_sealed_evidence(
            root,
            verified_path,
            manifest,
            source_seal_path,
            composite_path,
            expected_composition(args),
        )
        if sealed_counts != counts:
            fail("destination manifest counts differ from Pi source seal")
    qualification_id = str(verified["qualification_id"])
    destination = (
        root
        / "manifests/storage-qualification"
        / qualification_id
        / f"handoff-{args.host}.json"
    )
    receipt = base_receipt(facts, root, stable_id)
    receipt.update(
        {
            "state": "handoff-verified",
            "qualification_id": qualification_id,
            "host": args.host,
            "verified_receipt_sha256": sha256(verified_path),
            "manifest_relative": manifest.relative_to(root).as_posix(),
            "manifest_sha256": before_digest,
            "raw_open_mode": "read-only",
            "analysis_output_relative": output.relative_to(root).as_posix(),
            "verification": counts,
            **sealed_evidence,
        }
    )
    write_new_json(destination, receipt)
    print(f"storage handoff verified: {destination}")


def command(name: str, default: str) -> list[str]:
    return shlex.split(os.environ.get(name, default))


def decode_mountinfo_path(value: str) -> Path:
    decoded = re.sub(
        r"\\([0-7]{3})",
        lambda match: chr(int(match.group(1), 8)),
        value,
    )
    return Path(decoded)


def linux_facts(root: Path) -> dict[str, Any]:
    findmnt = subprocess.run(
        [*command("WAFER_FINDMNT", "findmnt"), "--json", "-T", str(root), "-o", "TARGET,SOURCE,FSTYPE,OPTIONS"],
        capture_output=True,
        check=True,
        text=True,
    )
    filesystems = json.loads(findmnt.stdout).get("filesystems", [])
    if len(filesystems) != 1:
        fail("findmnt did not return one filesystem")
    mounted = filesystems[0]
    source = Path(str(mounted["source"]).split("[", 1)[0])
    lsblk = subprocess.run(
        [*command("WAFER_LSBLK", "lsblk"), "--json", "-o", "PATH,UUID,LABEL,FSTYPE", str(source)],
        capture_output=True,
        check=True,
        text=True,
    )
    devices = json.loads(lsblk.stdout).get("blockdevices", [])
    if len(devices) != 1:
        fail("lsblk did not return one block device")
    device = devices[0]
    stable_ids = []
    by_id = Path(os.environ.get("WAFER_BY_ID", "/dev/disk/by-id"))
    if by_id.is_dir():
        for path in sorted(by_id.iterdir()):
            try:
                if path.resolve() == source.resolve():
                    stable_ids.append(f"by-id:{path.name}")
            except OSError:
                continue
    mount_id = ""
    mountinfo = Path(os.environ.get("WAFER_MOUNTINFO", "/proc/self/mountinfo"))
    if mountinfo.is_file():
        target = Path(str(mounted["target"])).resolve()
        for line in mountinfo.read_text().splitlines():
            parts = line.split()
            if len(parts) > 4 and decode_mountinfo_path(parts[4]).resolve() == target:
                mount_id = parts[0]
                break
    options = [part for part in str(mounted.get("options", "")).split(",") if part]
    stats = os.statvfs(root)
    return {
        "schema_version": 1,
        "platform": "linux",
        "stable_device_ids": stable_ids,
        "source": str(source),
        "mount_path": str(Path(str(mounted["target"])).resolve()),
        "mount_id": mount_id,
        "uuid": device.get("uuid"),
        "label": device.get("label"),
        "filesystem": device.get("fstype"),
        "mount_options": options,
        "read_write": "rw" in options and "ro" not in options,
        "available_bytes": stats.f_bavail * stats.f_frsize,
        "path_device": root.stat().st_dev,
    }


def macos_facts(root: Path) -> dict[str, Any]:
    completed = subprocess.run(
        [*command("WAFER_DISKUTIL", "diskutil"), "info", "-plist", str(root)],
        capture_output=True,
        check=True,
    )
    value = plistlib.loads(completed.stdout)
    read_write = not bool(value.get("ReadOnly", value.get("VolumeReadOnly", False)))
    options = ["rw" if read_write else "ro"]
    stats = os.statvfs(root)
    device = str(value.get("DeviceIdentifier", ""))
    stable_ids = [
        f"{key}:{value[key]}"
        for key in ("DiskUUID", "VolumeUUID", "MediaUUID")
        if value.get(key)
    ]
    return {
        "schema_version": 1,
        "platform": "macos",
        "stable_device_ids": stable_ids,
        "source": f"/dev/{device}",
        "mount_path": str(Path(str(value.get("MountPoint", ""))).resolve()),
        "mount_id": device,
        "uuid": value.get("VolumeUUID"),
        "label": value.get("VolumeName"),
        "filesystem": value.get("FilesystemType"),
        "mount_options": options,
        "read_write": read_write,
        "available_bytes": stats.f_bavail * stats.f_frsize,
        "path_device": root.stat().st_dev,
    }


def facts(args: argparse.Namespace) -> None:
    root = args.results_root.resolve()
    platform = os.environ.get("WAFER_PLATFORM", sys.platform)
    if platform == "darwin":
        value = macos_facts(root)
    else:
        value = linux_facts(root)
    json.dump(value, sys.stdout, indent=2, sort_keys=True)
    sys.stdout.write("\n")


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description="Non-destructive WAF_RESULTS qualification")
    subparsers = root.add_subparsers(dest="command", required=True)

    facts_parser = subparsers.add_parser("facts")
    facts_parser.add_argument("--results-root", type=Path, required=True)
    facts_parser.set_defaults(handler=facts)

    prepare_parser = subparsers.add_parser("prepare")
    prepare_parser.add_argument("--results-root", type=Path, required=True)
    prepare_parser.add_argument("--facts-json", type=Path, required=True)
    prepare_parser.add_argument("--expected-device-id", required=True)
    prepare_parser.add_argument("--expected-uuid", required=True)
    prepare_parser.add_argument("--qualification-id", required=True)
    prepare_parser.add_argument("--min-free-bytes", type=int, required=True)
    prepare_parser.add_argument("--large-bytes", type=int, default=64 * 1024 * 1024)
    prepare_parser.add_argument("--small-files", type=int, default=1024)
    prepare_parser.add_argument("--small-bytes", type=int, default=4096)
    prepare_parser.set_defaults(handler=prepare)

    verify_parser = subparsers.add_parser("verify-remount")
    verify_parser.add_argument("--results-root", type=Path, required=True)
    verify_parser.add_argument("--facts-json", type=Path, required=True)
    verify_parser.add_argument("--prepared-receipt", type=Path, required=True)
    verify_parser.set_defaults(handler=verify_remount)

    seal_parser = subparsers.add_parser("seal")
    seal_parser.add_argument("--results-root", type=Path, required=True)
    seal_parser.add_argument("--facts-json", type=Path, required=True)
    seal_parser.add_argument("--verified-receipt", type=Path, required=True)
    seal_parser.add_argument("--terminal-reconciliation", type=Path, required=True)
    seal_parser.add_argument("--expected-reconciliation-sha256", required=True)
    composition_arguments(seal_parser)
    seal_parser.set_defaults(handler=seal)

    handoff_parser = subparsers.add_parser("handoff")
    handoff_parser.add_argument("--results-root", type=Path, required=True)
    handoff_parser.add_argument("--facts-json", type=Path, required=True)
    handoff_parser.add_argument("--verified-receipt", type=Path, required=True)
    handoff_parser.add_argument("--manifest", type=Path, required=True)
    handoff_parser.add_argument("--source-seal", type=Path)
    handoff_parser.add_argument("--composite", type=Path)
    handoff_parser.add_argument("--analysis-output", type=Path, required=True)
    handoff_parser.add_argument("--host", choices=("macos", "jetson"), required=True)
    composition_arguments(handoff_parser)
    handoff_parser.set_defaults(handler=handoff)
    return root


def main() -> int:
    args = parser().parse_args()
    try:
        args.handler(args)
    except (OSError, ValueError, KeyError, TypeError, subprocess.CalledProcessError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
