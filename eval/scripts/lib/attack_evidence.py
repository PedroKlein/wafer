from __future__ import annotations

import hashlib
import json
import re
import subprocess
from pathlib import Path
from typing import Any

HEALTHY_REFERENCE_ID = "healthy-reference"
EXPECTED_SCENARIOS = ("S1", "S2", "S3", "S4", "S5", "S6")
EXPECTED_OUTCOMES = (
    "buffer-overflow-trap",
    "cross-read-trap",
    "epoch-timeout",
    "memory-limit-trap",
    "fs-read-denied",
    "guest-panic-trap",
)
_EXPECTED_MANIFEST_IDS = {HEALTHY_REFERENCE_ID, *EXPECTED_SCENARIOS}
_EXPECTED_RECEIPT_IDS = set(EXPECTED_SCENARIOS)
_SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
_GIT_SHA_RE = re.compile(r"^[0-9a-f]{40}$")


def sha256_path(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _require_mapping(value: Any, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be an object")
    return value


def _require_list(value: Any, label: str) -> list[Any]:
    if not isinstance(value, list):
        raise ValueError(f"{label} must be a list")
    return value


def _require_nonempty_str(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value:
        raise ValueError(f"{label} is invalid")
    return value


def _require_sha(value: Any, label: str, pattern: re.Pattern[str]) -> str:
    if not isinstance(value, str) or not pattern.fullmatch(value):
        raise ValueError(f"{label} is invalid")
    return value


def detect_component_world(path: Path) -> str:
    result = subprocess.run(
        ["wasm-tools", "component", "wit", str(path)],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        raise ValueError(f"failed to inspect component world for {path}: {result.stderr.strip()}")
    match = re.search(r"^world\s+([^\s{]+)\s*\{", result.stdout, re.MULTILINE)
    if not match:
        raise ValueError(f"component world is missing for {path}")
    return match.group(1)


def build_manifest_entry(
    scenario_id: str,
    path: Path,
    *,
    wit_world: str,
    source_git_sha: str,
    release_git_sha: str,
) -> dict[str, Any]:
    return {
        "scenario_id": scenario_id,
        "path": path.as_posix(),
        "byte_length": path.stat().st_size,
        "sha256": sha256_path(path),
        "wit_world": wit_world,
        "component_world": detect_component_world(path),
        "source_git_sha": source_git_sha,
        "release_git_sha": release_git_sha,
    }


def resolve_wasmtime_revision(lockfile: Path) -> dict[str, str]:
    source = lockfile.read_text()
    match = re.search(
        r'name = "wasmtime"\nversion = "([^"]+)"\nsource = "git\+https://github.com/bytecodealliance/wasmtime\?rev=([0-9a-f]{40})#([0-9a-f]{40})"',
        source,
    )
    if not match:
        raise ValueError("wasmtime git revision is missing from Cargo.lock")
    version, requested_rev, resolved_rev = match.groups()
    if requested_rev != resolved_rev:
        raise ValueError("wasmtime requested and resolved revisions differ")
    return {"version": version, "resolved_revision": resolved_rev}


def validate_attack_manifest(manifest: dict[str, Any]) -> None:
    if manifest.get("schema_version") != 1:
        raise ValueError("manifest schema_version must be 1")
    manifest_source_git_sha = _require_sha(manifest.get("source_git_sha"), "manifest source_git_sha", _GIT_SHA_RE)
    manifest_release_git_sha = _require_sha(manifest.get("release_git_sha"), "manifest release_git_sha", _GIT_SHA_RE)
    artifacts = _require_list(manifest.get("artifacts"), "manifest artifacts")
    seen: set[str] = set()
    seen_attack_hashes: dict[str, str] = {}
    for entry in artifacts:
        item = _require_mapping(entry, "manifest artifact")
        scenario_id = item.get("scenario_id")
        if not isinstance(scenario_id, str):
            raise ValueError("manifest artifact scenario_id is invalid")
        if scenario_id in seen:
            raise ValueError(f"duplicate scenario in manifest: {scenario_id}")
        seen.add(scenario_id)
        _require_nonempty_str(item.get("path"), f"manifest artifact path for {scenario_id}")
        if type(item.get("byte_length")) is not int or item["byte_length"] <= 0:
            raise ValueError(f"manifest artifact byte_length is invalid: {scenario_id}")
        artifact_sha256 = _require_sha(item.get("sha256"), f"manifest artifact sha256 for {scenario_id}", _SHA256_RE)
        if scenario_id != HEALTHY_REFERENCE_ID:
            previous_scenario = seen_attack_hashes.get(artifact_sha256)
            if previous_scenario is not None:
                raise ValueError(
                    f"duplicate attack artifact sha256 in manifest: {scenario_id} matches {previous_scenario}"
                )
            seen_attack_hashes[artifact_sha256] = scenario_id
        _require_nonempty_str(item.get("wit_world"), f"manifest artifact wit_world for {scenario_id}")
        _require_nonempty_str(item.get("component_world"), f"manifest artifact component_world for {scenario_id}")
        artifact_source_git_sha = _require_sha(
            item.get("source_git_sha"),
            f"manifest artifact source_git_sha for {scenario_id}",
            _GIT_SHA_RE,
        )
        artifact_release_git_sha = _require_sha(
            item.get("release_git_sha"),
            f"manifest artifact release_git_sha for {scenario_id}",
            _GIT_SHA_RE,
        )
        if artifact_source_git_sha != manifest_source_git_sha:
            raise ValueError(f"manifest artifact source_git_sha does not match manifest: {scenario_id}")
        if artifact_release_git_sha != manifest_release_git_sha:
            raise ValueError(f"manifest artifact release_git_sha does not match manifest: {scenario_id}")
    if seen != _EXPECTED_MANIFEST_IDS:
        raise ValueError(f"manifest scenario set is invalid: {sorted(seen)}")


def validate_attack_receipt(receipt: dict[str, Any]) -> None:
    if receipt.get("schema_version") != 1:
        raise ValueError("receipt schema_version must be 1")
    _require_sha(receipt.get("source_git_sha"), "receipt source_git_sha", _GIT_SHA_RE)
    _require_sha(receipt.get("release_git_sha"), "receipt release_git_sha", _GIT_SHA_RE)
    _require_nonempty_str(receipt.get("started_at"), "receipt started_at")
    _require_nonempty_str(receipt.get("finished_at"), "receipt finished_at")
    _require_nonempty_str(receipt.get("rustc_version"), "receipt rustc_version")
    _require_nonempty_str(receipt.get("rustc_verbose"), "receipt rustc_verbose")
    _require_nonempty_str(receipt.get("toolchain"), "receipt toolchain")
    wasmtime = _require_mapping(receipt.get("wasmtime"), "receipt wasmtime")
    _require_nonempty_str(wasmtime.get("version"), "receipt wasmtime version")
    _require_sha(wasmtime.get("resolved_revision"), "receipt wasmtime resolved_revision", _GIT_SHA_RE)
    _require_nonempty_str(receipt.get("test_command"), "receipt test_command")
    manifest = _require_mapping(receipt.get("manifest"), "receipt manifest")
    _require_nonempty_str(manifest.get("path"), "receipt manifest path")
    _require_sha(manifest.get("sha256"), "receipt manifest sha256", _SHA256_RE)
    execution = _require_mapping(receipt.get("execution"), "receipt execution")
    healthy_reference = _require_mapping(execution.get("healthy_reference"), "healthy reference execution")
    if healthy_reference.get("executed") is not True:
        raise ValueError("healthy reference must execute")
    scenarios = _require_list(execution.get("scenarios"), "receipt scenarios")
    unique_outcomes = _require_list(execution.get("unique_outcomes"), "receipt unique outcomes")
    seen_ids: set[str] = set()
    outcomes: list[str] = []
    for entry in scenarios:
        item = _require_mapping(entry, "receipt scenario")
        scenario_id = item.get("scenario_id")
        if not isinstance(scenario_id, str):
            raise ValueError("receipt scenario_id is invalid")
        if scenario_id in seen_ids:
            raise ValueError(f"duplicate scenario in receipt: {scenario_id}")
        seen_ids.add(scenario_id)
        if item.get("executed") is not True:
            raise ValueError(f"scenario must execute: {scenario_id}")
        if item.get("healthy_before") is not True or item.get("healthy_after") is not True:
            raise ValueError(f"healthy reference must succeed before and after {scenario_id}")
        outcome = item.get("outcome")
        if not isinstance(outcome, str) or not outcome:
            raise ValueError(f"scenario outcome is invalid: {scenario_id}")
        outcomes.append(outcome)
    if seen_ids != _EXPECTED_RECEIPT_IDS:
        raise ValueError(f"receipt scenario set is invalid: {sorted(seen_ids)}")
    if set(outcomes) != set(EXPECTED_OUTCOMES) or len(set(outcomes)) != len(EXPECTED_OUTCOMES):
        raise ValueError("receipt unique outcomes are invalid")
    if unique_outcomes != outcomes:
        raise ValueError("receipt unique outcomes do not match executed order")


def validate_attack_evidence_bundle(
    manifest: dict[str, Any],
    receipt: dict[str, Any],
    *,
    output_dir: Path,
    manifest_path: Path,
) -> None:
    validate_attack_manifest(manifest)
    validate_attack_receipt(receipt)

    expected_manifest_path = manifest_path.relative_to(output_dir).as_posix()
    receipt_manifest = _require_mapping(receipt.get("manifest"), "receipt manifest")
    receipt_manifest_path = _require_nonempty_str(receipt_manifest.get("path"), "receipt manifest path")
    receipt_manifest_sha256 = _require_sha(receipt_manifest.get("sha256"), "receipt manifest sha256", _SHA256_RE)
    if receipt_manifest_path != expected_manifest_path:
        raise ValueError("receipt manifest path does not match manifest file")
    if receipt_manifest_sha256 != sha256_path(manifest_path):
        raise ValueError("receipt manifest digest does not match manifest file")

    receipt_source_git_sha = _require_sha(receipt.get("source_git_sha"), "receipt source_git_sha", _GIT_SHA_RE)
    receipt_release_git_sha = _require_sha(receipt.get("release_git_sha"), "receipt release_git_sha", _GIT_SHA_RE)
    manifest_source_git_sha = _require_sha(manifest.get("source_git_sha"), "manifest source_git_sha", _GIT_SHA_RE)
    manifest_release_git_sha = _require_sha(manifest.get("release_git_sha"), "manifest release_git_sha", _GIT_SHA_RE)
    if receipt_source_git_sha != manifest_source_git_sha or receipt_release_git_sha != manifest_release_git_sha:
        raise ValueError("receipt and manifest git bindings do not match")


def load_execution(path: Path) -> dict[str, Any]:
    return _require_mapping(json.loads(path.read_text()), f"execution payload {path}")


def write_json(path: Path, value: dict[str, Any]) -> None:
    path.write_text(json.dumps(value, indent=2) + "\n")
