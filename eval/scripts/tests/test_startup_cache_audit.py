#!/usr/bin/env python3

import importlib.util
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "eval/scripts/audit-startup-cache.py"
spec = importlib.util.spec_from_file_location("audit_startup_cache", SCRIPT)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)


def test_aot_label_requires_compiled_artifact_evidence() -> None:
    with pytest.raises(module.StartupCacheAuditError, match="AOT label requires"):
        module.validate_startup_claims(
            ["AOT cold versus warm"],
            {"runtime_wired": False, "artifact_paths": []},
        )


def test_repository_labels_match_filesystem_cache_evidence() -> None:
    result = module.audit_repository(ROOT)

    assert result["os_page_cache"]["cold_preparation_present"] is True
    assert result["in_memory_component_cache"] == {
        "implemented": True,
        "initial_launch_uses_cache": False,
        "lifetime": "one wafer-runtime process",
        "survives_process_restart": False,
    }
    assert result["persistent_compiled_cache"]["implementation_present"] is True
    assert result["persistent_compiled_cache"]["disk_constructor_present"] is True
    assert result["persistent_compiled_cache"]["runtime_wired"] is False
    assert result["persistent_compiled_cache"]["runtime_constructor_present"] is True
    assert result["persistent_compiled_cache"]["artifact_count"] == 0
    assert result["pilot_artifacts"] == {
        "result_root": "eval/results/e-perf-9/rpi5-pilot-v11-n3-20260831",
        "startup_artifact_count": 18,
        "startup_fields": ["cache_state", "wall_duration_ns"],
        "compiled_cache_evidence_fields": [],
    }
    assert result["labels"]["aot_claim_present"] is False
    assert result["decision"] == "relabel-filesystem-page-cache"
