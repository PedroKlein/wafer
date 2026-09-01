#!/usr/bin/env python3

import argparse
import json
from pathlib import Path


class StartupCacheAuditError(ValueError):
    pass


def validate_startup_claims(labels: list[str], compiled_cache: dict) -> None:
    claims_aot = any("aot" in label.lower() for label in labels)
    has_artifact_evidence = bool(compiled_cache.get("artifact_paths"))
    if claims_aot and not (compiled_cache.get("runtime_wired") and has_artifact_evidence):
        raise StartupCacheAuditError(
            "AOT label requires a runtime-wired persistent compiled cache and recorded artifacts"
        )


def audit_repository(root: Path) -> dict:
    launcher = (root / "crates/wafer-core/src/orchestrator/launcher.rs").read_text()
    loader = (root / "crates/wafer-core/src/engine/loader.rs").read_text()
    cache = (root / "crates/wafer-core/src/engine/cache.rs").read_text()
    runner = (root / "eval/scripts/lib/canonical_runner.py").read_text()
    matrix = json.loads((root / "eval/canonical-matrix.json").read_text())
    config_paths = sorted((root / "eval/configs/e-perf-9").glob("*.toml"))
    notebook_path = root / "eval/analysis/notebooks/10-aot-startup.ipynb"
    labels = [
        matrix["experiments"]["e-perf-9"]["purpose"],
        *(path.read_text() for path in config_paths),
        notebook_path.read_text(),
    ]
    pilot_root = root / "eval/results/e-perf-9/rpi5-pilot-v11-n3-20260831"
    artifact_paths = [
        str(path.relative_to(root))
        for path in pilot_root.rglob("*.cwasm")
    ] if pilot_root.exists() else []
    startup_paths = sorted(pilot_root.rglob("startup.json")) if pilot_root.exists() else []
    startup_fields = sorted({
        field
        for path in startup_paths
        for field in json.loads(path.read_text()).keys()
    })
    compiled_cache = {
        "implementation_present": "struct ComponentCache" in cache,
        "disk_constructor_present": "pub fn with_cache_dir" in loader,
        "runtime_wired": "WaferEngine::with_cache_dir" in launcher,
        "runtime_constructor": "WaferEngine::from_engine_config",
        "runtime_constructor_present": "WaferEngine::from_engine_config" in launcher,
        "artifact_paths": artifact_paths,
        "artifact_count": len(artifact_paths),
    }
    validate_startup_claims(labels, compiled_cache)
    return {
        "schema_version": 1,
        "experiment": "e-perf-9",
        "os_page_cache": {
            "cold_preparation": "sync; echo 3 > /proc/sys/vm/drop_caches",
            "cold_preparation_present": "sync; echo 3 > /proc/sys/vm/drop_caches" in runner,
            "warm_preparation": "no page-cache drop before the paired warm process",
        },
        "in_memory_component_cache": {
            "implemented": "ComponentCache::memory_only()" in loader,
            "lifetime": "one wafer-runtime process",
            "survives_process_restart": False,
        },
        "persistent_compiled_cache": compiled_cache,
        "pilot_artifacts": {
            "result_root": str(pilot_root.relative_to(root)),
            "startup_artifact_count": len(startup_paths),
            "startup_fields": startup_fields,
            "compiled_cache_evidence_fields": [
                field
                for field in startup_fields
                if field in {"cache_artifact_path", "cache_key", "compiled_cache_hit"}
            ],
        },
        "labels": {
            "aot_claim_present": any("aot" in label.lower() for label in labels),
            "files_checked": [
                "eval/canonical-matrix.json",
                *(str(path.relative_to(root)) for path in config_paths),
                str(notebook_path.relative_to(root)),
            ],
        },
        "decision": "relabel-filesystem-page-cache",
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Audit E-Perf-9 cache semantics")
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        result = audit_repository(args.root.resolve())
    except (OSError, KeyError, StartupCacheAuditError, ValueError) as error:
        print(f"ERROR: {error}")
        return 1
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(args.output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
