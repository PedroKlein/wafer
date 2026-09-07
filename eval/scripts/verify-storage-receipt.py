#!/usr/bin/env python3

from __future__ import annotations

import argparse
import hashlib
import json
import os
import plistlib
import re
import shlex
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

CHUNK = bytes(range(256)) * 4096
SAFE_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
SHA_LINE = re.compile(r"^([0-9a-f]{64})  (.+)$")
LAYOUT = ("raw", "manifests", "derived", "reports")


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

    handoff_parser = subparsers.add_parser("handoff")
    handoff_parser.add_argument("--results-root", type=Path, required=True)
    handoff_parser.add_argument("--facts-json", type=Path, required=True)
    handoff_parser.add_argument("--verified-receipt", type=Path, required=True)
    handoff_parser.add_argument("--manifest", type=Path, required=True)
    handoff_parser.add_argument("--analysis-output", type=Path, required=True)
    handoff_parser.add_argument("--host", choices=("macos", "jetson"), required=True)
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
