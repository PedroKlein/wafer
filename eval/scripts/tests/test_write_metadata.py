#!/usr/bin/env python3
"""Regression test for eval/scripts/lib/write_metadata.py.

Reviewer-flagged gap on the F2 pass: the existing wafer-runtime integration
test at crates/wafer-runtime/tests/metadata_provenance.rs only inspects the
runtime-produced `runtime-provenance.json` sidecar. A broken merge in
run-experiment.sh's `_write_metadata` (dropping keys, wrong precedence,
missing sidecar handling) would go undetected.

This test invokes the extracted merger with (a) a full provenance sidecar
and (b) a "null" sidecar, then asserts the resulting metadata.json shape.
"""

import json
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
MERGER = REPO_ROOT / "eval/scripts/lib/write_metadata.py"


def run_merger(
    out_path: Path, provenance_json: str, cwd: Path | None = None
) -> dict:
    cmd = [
        sys.executable,
        str(MERGER),
        str(out_path),
        "e-test",              # experiment
        "shakedown-test",      # host
        "2026-08-02T00-00-00Z",  # finished
        "2026-08-02T00-00-00Z",  # started
        "1000000",             # duration_ns
        "eval/configs/pipeline-shakedown.toml",  # config
        "a" * 64,              # cfg_sha
        "null",                # loadgen
        "null",                # mosquitto
        "0",                   # rc
        provenance_json,
    ]
    subprocess.run(cmd, check=True, capture_output=True, text=True, cwd=cwd)
    return json.loads(out_path.read_text())


def test_merge_promotes_runtime_provenance() -> None:
    """When the sidecar is present every runtime-authoritative key must
    end up in metadata.json — even when it collides with a harness-side
    value like `rustc_version` or `config_sha256`."""
    sidecar = {
        "wasmtime_version": "42.0.0-runtime",
        "rustc_version": "rustc 1.99.0-runtime",
        "wafer_runtime_version": "0.1.0",
        "wafer_runtime_sha256": "b" * 64,
        "wafer_plugin_hashes": {"pass-through": "c" * 64},
        "kernel": "runtime-24.0.0-arm64",
        "config_sha256": "d" * 64,
        "engine_fuel_budgets": {"transform": 10_000_000, "filter": 500_000, "router": 500_000},
        "epoch_deadline": 100,
        "epoch_tick_ms": 10,
        "effective_metering_mode": "fuel-and-epoch",
    }
    with tempfile.TemporaryDirectory() as tmp:
        out = Path(tmp) / "metadata.json"
        meta = run_merger(out, json.dumps(sidecar))

    for key, expected in sidecar.items():
        assert meta.get(key) == expected, (
            f"metadata.json {key} = {meta.get(key)!r}, sidecar had {expected!r}"
        )
    assert isinstance(meta["git_dirty"], bool)
    for key in ("experiment", "host_tag", "duration_ns", "git_sha",
                "git_dirty", "hostname", "arch", "os", "config_path", "loadgen",
                "mosquitto", "exit_codes"):
        assert key in meta, f"harness-side key missing: {key}"


def test_deployed_source_state_fills_git_provenance() -> None:
    source_state = {
        "git_sha": "1" * 40,
        "git_dirty": True,
        "git_tags": ["rpi5-eval-v1"],
    }
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "SOURCE_STATE.json").write_text(json.dumps(source_state))
        meta = run_merger(root / "metadata.json", "null", cwd=root)

    assert meta["git_sha"] == source_state["git_sha"]
    assert meta["git_dirty"] is True
    assert meta["git_tags"] == source_state["git_tags"]


def test_merge_without_sidecar_keeps_harness_values() -> None:
    """When provenance is 'null', metadata.json still contains the six
    RESULT-CONTRACT core keys — but harness-collected values (rustc,
    kernel, config_sha256, hostname) fill in the runtime-authoritative
    slots. Prevents a silent-degradation where a missing sidecar leaves
    metadata.json without any of the fields."""
    with tempfile.TemporaryDirectory() as tmp:
        out = Path(tmp) / "metadata.json"
        meta = run_merger(out, "null")

    for key in ("experiment", "host_tag", "kernel", "arch", "os",
                "rustc_version", "config_path", "config_sha256",
                "duration_ns", "exit_codes", "hardware_model",
                "memory_total_kib", "cpu_governors", "isolated_cpus",
                "temperature_millicelsius", "throttled"):
        assert key in meta and meta[key] not in ("", None), (
            f"harness-side fallback missing/empty for {key}: {meta.get(key)!r}"
        )
    # Sidecar-only keys must NOT be present when the sidecar was absent —
    # otherwise a stale merge would silently claim runtime-verified data.
    for key in ("wasmtime_version", "wafer_runtime_version",
                "wafer_runtime_sha256", "wafer_plugin_hashes"):
        assert key not in meta, (
            f"sidecar-only key {key} appeared in metadata.json when "
            f"provenance was 'null' — merge logic is wrong"
        )


if __name__ == "__main__":
    test_merge_promotes_runtime_provenance()
    test_deployed_source_state_fills_git_provenance()
    test_merge_without_sidecar_keeps_harness_values()
    print("write_metadata merge tests: PASS")
