#!/usr/bin/env python3

import copy
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "eval/scripts/lib"))

import attack_evidence  # noqa: E402


def base_manifest() -> dict:
    source_git_sha = "1" * 40
    release_git_sha = "2" * 40
    artifacts = [
        {
            "scenario_id": "healthy-reference",
            "path": "plugins/pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm",
            "byte_length": 123,
            "sha256": "a" * 64,
            "wit_world": "transform-node",
            "component_world": "root",
            "source_git_sha": source_git_sha,
            "release_git_sha": release_git_sha,
        }
    ]
    for index in range(1, 7):
        artifacts.append(
            {
                "scenario_id": f"S{index}",
                "path": f"plugins/attacks/s{index}.wasm",
                "byte_length": 100 + index,
                "sha256": str(index) * 64,
                "wit_world": "transform-node",
                "component_world": "root",
                "source_git_sha": source_git_sha,
                "release_git_sha": release_git_sha,
            }
        )
    return {
        "schema_version": 1,
        "source_git_sha": source_git_sha,
        "release_git_sha": release_git_sha,
        "artifacts": artifacts,
    }


def base_receipt() -> dict:
    scenarios = []
    for index, outcome in enumerate(attack_evidence.EXPECTED_OUTCOMES, start=1):
        scenarios.append(
            {
                "scenario_id": f"S{index}",
                "executed": True,
                "healthy_before": True,
                "healthy_after": True,
                "outcome": outcome,
            }
        )
    return {
        "schema_version": 1,
        "source_git_sha": "1" * 40,
        "release_git_sha": "2" * 40,
        "started_at": "2026-01-01T00:00:00Z",
        "finished_at": "2026-01-01T00:00:01Z",
        "rustc_version": "rustc 1.98.1 (48a229cea 2026-09-01)",
        "rustc_verbose": "host: aarch64-apple-darwin",
        "toolchain": "1.98.1-aarch64-apple-darwin",
        "wasmtime": {
            "version": "48.0.2",
            "resolved_revision": "3" * 40,
        },
        "test_command": "cargo test ...",
        "manifest": {
            "path": "attack-manifest.json",
            "sha256": "4" * 64,
        },
        "execution": {
            "healthy_reference": {"executed": True},
            "scenarios": scenarios,
            "unique_outcomes": list(attack_evidence.EXPECTED_OUTCOMES),
        },
    }


def test_manifest_rejects_duplicate_or_missing_scenarios() -> None:
    manifest = base_manifest()
    attack_evidence.validate_attack_manifest(manifest)

    duplicate = copy.deepcopy(manifest)
    duplicate["artifacts"][1]["scenario_id"] = "S2"
    with pytest.raises(ValueError, match="duplicate scenario"):
        attack_evidence.validate_attack_manifest(duplicate)

    missing = copy.deepcopy(manifest)
    missing["artifacts"] = [item for item in missing["artifacts"] if item["scenario_id"] != "S6"]
    with pytest.raises(ValueError, match="scenario set"):
        attack_evidence.validate_attack_manifest(missing)


def test_manifest_requires_unique_attack_hashes_but_not_unique_healthy_reference_hash() -> None:
    manifest = base_manifest()
    attack_evidence.validate_attack_manifest(manifest)

    duplicate_attack_hash = copy.deepcopy(manifest)
    duplicate_attack_hash["artifacts"][2]["sha256"] = duplicate_attack_hash["artifacts"][1]["sha256"]
    with pytest.raises(ValueError, match="duplicate attack artifact sha256"):
        attack_evidence.validate_attack_manifest(duplicate_attack_hash)

    healthy_reference_can_match = copy.deepcopy(manifest)
    healthy_reference_can_match["artifacts"][0]["sha256"] = healthy_reference_can_match["artifacts"][1]["sha256"]
    attack_evidence.validate_attack_manifest(healthy_reference_can_match)


def test_receipt_rejects_duplicate_outcomes_and_incomplete_execution() -> None:
    receipt = base_receipt()
    attack_evidence.validate_attack_receipt(receipt)

    duplicate_outcome = copy.deepcopy(receipt)
    duplicate_outcome["execution"]["scenarios"][1]["outcome"] = duplicate_outcome["execution"]["scenarios"][0]["outcome"]
    duplicate_outcome["execution"]["unique_outcomes"] = duplicate_outcome["execution"]["unique_outcomes"][1:]
    with pytest.raises(ValueError, match="unique outcomes"):
        attack_evidence.validate_attack_receipt(duplicate_outcome)

    unexecuted = copy.deepcopy(receipt)
    unexecuted["execution"]["scenarios"][2]["executed"] = False
    with pytest.raises(ValueError, match="must execute"):
        attack_evidence.validate_attack_receipt(unexecuted)

    incomplete = copy.deepcopy(receipt)
    incomplete["execution"]["scenarios"].pop()
    incomplete["execution"]["unique_outcomes"].pop()
    with pytest.raises(ValueError, match="scenario set"):
        attack_evidence.validate_attack_receipt(incomplete)


def test_receipt_rejects_missing_required_provenance_fields() -> None:
    receipt = base_receipt()
    attack_evidence.validate_attack_receipt(receipt)

    missing_source = copy.deepcopy(receipt)
    missing_source.pop("source_git_sha")
    with pytest.raises(ValueError, match="source_git_sha"):
        attack_evidence.validate_attack_receipt(missing_source)

    malformed_release = copy.deepcopy(receipt)
    malformed_release["release_git_sha"] = "not-a-sha"
    with pytest.raises(ValueError, match="release_git_sha"):
        attack_evidence.validate_attack_receipt(malformed_release)

    empty_rustc_version = copy.deepcopy(receipt)
    empty_rustc_version["rustc_version"] = ""
    with pytest.raises(ValueError, match="rustc_version"):
        attack_evidence.validate_attack_receipt(empty_rustc_version)

    empty_rustc_verbose = copy.deepcopy(receipt)
    empty_rustc_verbose["rustc_verbose"] = ""
    with pytest.raises(ValueError, match="rustc_verbose"):
        attack_evidence.validate_attack_receipt(empty_rustc_verbose)

    empty_toolchain = copy.deepcopy(receipt)
    empty_toolchain["toolchain"] = ""
    with pytest.raises(ValueError, match="toolchain"):
        attack_evidence.validate_attack_receipt(empty_toolchain)

    missing_wasmtime = copy.deepcopy(receipt)
    missing_wasmtime.pop("wasmtime")
    with pytest.raises(ValueError, match="wasmtime"):
        attack_evidence.validate_attack_receipt(missing_wasmtime)

    empty_wasmtime_version = copy.deepcopy(receipt)
    empty_wasmtime_version["wasmtime"]["version"] = ""
    with pytest.raises(ValueError, match="version"):
        attack_evidence.validate_attack_receipt(empty_wasmtime_version)

    bad_wasmtime_revision = copy.deepcopy(receipt)
    bad_wasmtime_revision["wasmtime"]["resolved_revision"] = "abc"
    with pytest.raises(ValueError, match="resolved_revision"):
        attack_evidence.validate_attack_receipt(bad_wasmtime_revision)

    missing_manifest = copy.deepcopy(receipt)
    missing_manifest.pop("manifest")
    with pytest.raises(ValueError, match="manifest"):
        attack_evidence.validate_attack_receipt(missing_manifest)

    empty_manifest_path = copy.deepcopy(receipt)
    empty_manifest_path["manifest"]["path"] = ""
    with pytest.raises(ValueError, match="path"):
        attack_evidence.validate_attack_receipt(empty_manifest_path)

    bad_manifest_digest = copy.deepcopy(receipt)
    bad_manifest_digest["manifest"]["sha256"] = "abc"
    with pytest.raises(ValueError, match="sha256"):
        attack_evidence.validate_attack_receipt(bad_manifest_digest)


def test_bundle_validation_rejects_binding_and_digest_mismatches(tmp_path: Path) -> None:
    manifest = base_manifest()
    manifest_path = tmp_path / "attack-manifest.json"
    attack_evidence.write_json(manifest_path, manifest)

    receipt = base_receipt()
    receipt["manifest"]["path"] = manifest_path.relative_to(tmp_path).as_posix()
    receipt["manifest"]["sha256"] = attack_evidence.sha256_path(manifest_path)

    attack_evidence.validate_attack_evidence_bundle(
        manifest,
        receipt,
        output_dir=tmp_path,
        manifest_path=manifest_path,
    )

    bad_source_binding = copy.deepcopy(receipt)
    bad_source_binding["source_git_sha"] = "9" * 40
    with pytest.raises(ValueError, match="git bindings"):
        attack_evidence.validate_attack_evidence_bundle(
            manifest,
            bad_source_binding,
            output_dir=tmp_path,
            manifest_path=manifest_path,
        )

    bad_manifest_path = copy.deepcopy(receipt)
    bad_manifest_path["manifest"]["path"] = "different-manifest.json"
    with pytest.raises(ValueError, match="path"):
        attack_evidence.validate_attack_evidence_bundle(
            manifest,
            bad_manifest_path,
            output_dir=tmp_path,
            manifest_path=manifest_path,
        )

    bad_manifest_digest = copy.deepcopy(receipt)
    bad_manifest_digest["manifest"]["sha256"] = "f" * 64
    with pytest.raises(ValueError, match="digest"):
        attack_evidence.validate_attack_evidence_bundle(
            manifest,
            bad_manifest_digest,
            output_dir=tmp_path,
            manifest_path=manifest_path,
        )
