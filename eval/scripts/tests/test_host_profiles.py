#!/usr/bin/env python3

import json
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "eval/scripts/lib"))

from host_profiles import host_profile, host_profiles  # noqa: E402

MATRIX = ROOT / "eval/canonical-matrix.json"
VALIDATOR = ROOT / "eval/scripts/validate-canonical.py"
RUNNER = ROOT / "eval/scripts/lib/canonical_runner.py"


def facts(host: str, **changes: object) -> dict:
    base = {
        "rpi5": {"arch": "aarch64", "hardware_model": "Raspberry Pi 5 Model B Rev 1.1"},
        "jetson": {
            "arch": "aarch64",
            "hardware_model": "NVIDIA Jetson Orin Nano Engineering Reference Developer Kit Super",
            "online_cpus": "0-3",
            "power_mode": "25W",
        },
        "x86": {
            "arch": "x86_64",
            "hardware_model": "Dell Inc. OptiPlex 7090",
            "smt": "off",
            "turbo": "off",
        },
    }[host]
    return {
        "host_tag": host,
        **base,
        "git_sha": "1" * 40,
        "git_dirty": False,
        "git_tags": ["eval-v1"],
        "cpu_governors": ["performance"],
        "isolated_cpus": "",
        "housekeeping_cpus": "0",
        "irq_default_cpus": "0",
        "throttled": "0x0",
        "broker_ready": True,
        "ekuiper_ready": True,
        "ekuiper_version": "2.1.0",
        **changes,
    }


def preflight(host: str, value: dict) -> subprocess.CompletedProcess[str]:
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "facts.json"
        path.write_text(json.dumps(value))
        return subprocess.run(
            [sys.executable, str(VALIDATOR), "preflight", str(path), "--host", host],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
        )


def test_rpi5_profile_keeps_the_frozen_pi_host_policy() -> None:
    profile = host_profile("rpi5")
    assert (profile.role, profile.arch, profile.hardware_model_contains) == (
        "canonical",
        "aarch64",
        "Raspberry Pi 5",
    )
    assert (profile.housekeeping_cpus, profile.sut_cpus, profile.support_cpus) == ("0", "1-3", "0")
    assert profile.cpu_governors == ("performance",) and profile.throttled == "0x0"
    assert profile.allowed_facts == {}
    matrix = json.loads(MATRIX.read_text())
    assert matrix["schema_version"] == 2 and matrix["canonical_host"] == "rpi5"
    assert {tag: p.role for tag, p in host_profiles(matrix).items()} == {
        "rpi5": "canonical",
        "jetson": "replication",
        "x86": "replication",
    }


def test_each_host_passes_its_own_preflight_and_fails_another_hosts() -> None:
    for host in ("rpi5", "jetson", "x86"):
        result = preflight(host, facts(host))
        assert result.returncode == 0, (host, result.stderr)
    wrong = preflight("jetson", facts("rpi5"))
    assert wrong.returncode == 1
    assert "host tag must be 'jetson'" in wrong.stderr
    assert "hardware model must contain 'Jetson Orin Nano'" in wrong.stderr


def test_host_specific_facts_are_enforced() -> None:
    jetson = preflight("jetson", facts("jetson", power_mode="MAXN_SUPER", online_cpus="0-5"))
    assert jetson.returncode == 1
    assert "power_mode must be one of ['25W'], got 'MAXN_SUPER'" in jetson.stderr
    assert "online_cpus must be one of ['0-3'], got '0-5'" in jetson.stderr
    x86 = preflight("x86", facts("x86", smt="on", turbo="on", arch="aarch64"))
    assert x86.returncode == 1
    for message in ("smt must be one of", "turbo must be one of", "architecture must be 'x86_64'"):
        assert message in x86.stderr
    assert preflight("x86", facts("x86", smt="notsupported")).returncode == 0
    unknown = preflight("rpi4", facts("rpi5"))
    assert unknown.returncode == 1 and "no host profile 'rpi4'" in unknown.stderr


def test_every_host_requires_cpu_0_housekeeping_and_no_isolated_cpus() -> None:
    for host in ("rpi5", "jetson", "x86"):
        result = preflight(
            host, facts(host, isolated_cpus="1-3", housekeeping_cpus="0-3", irq_default_cpus="0-3")
        )
        assert result.returncode == 1
        assert "isolated CPUs must be empty, got '1-3'" in result.stderr
        assert "housekeeping CPUs must be '0', got '0-3'" in result.stderr
        assert "default IRQ CPUs must be '0', got '0-3'" in result.stderr
        unreadable = preflight(host, facts(host, isolated_cpus=None))
        assert "isolated CPUs must be empty, got None" in unreadable.stderr


def test_matrix_validation_rejects_a_missing_or_overlapping_host() -> None:
    sys.path.insert(0, str(ROOT / "eval/scripts"))
    import importlib.util

    spec = importlib.util.spec_from_file_location("validate_canonical", VALIDATOR)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    matrix = json.loads(MATRIX.read_text())
    assert module.validate_hosts(matrix) == []
    del matrix["hosts"]["x86"]
    assert any("hosts must be rpi5, jetson and x86" in e for e in module.validate_hosts(matrix))
    matrix = json.loads(MATRIX.read_text())
    matrix["hosts"]["jetson"]["support_cpus"] = "1"
    assert any("jetson SUT and support CPUs overlap" in e for e in module.validate_hosts(matrix))
    assert any("jetson support CPUs must be housekeeping CPUs" in e for e in module.validate_hosts(matrix))
    matrix = json.loads(MATRIX.read_text())
    matrix["hosts"]["x86"]["housekeeping_cpus"] = "0-1"
    assert module.validate_hosts(matrix) == ["host x86 housekeeping and SUT CPUs overlap"]
    matrix["schema_version"] = 1
    assert module.validate_hosts(matrix) == ["schema_version must be 2"]


def test_runner_plans_a_batch_for_the_selected_host() -> None:
    import canonical_runner

    try:
        canonical_runner.select_host("x86")
        item = canonical_runner.RunItem("e-val-1", "delay-50ms", 1, "c.toml", 1, 1)
        assert canonical_runner.batch_name("b1") == "x86-b1"
        assert (item.runtime_cpus, item.support_cpus) == ("1-3", "0")
    finally:
        canonical_runner.select_host("rpi5")

    command = [sys.executable, str(RUNNER), "--experiments", "e-val-1", "--batch-id", "b1", "--dry-run"]
    plan = subprocess.run([*command, "--host", "jetson"], cwd=ROOT, capture_output=True, text=True)
    assert plan.returncode == 0, plan.stderr
    assert "host=jetson" in plan.stdout
    unknown = subprocess.run([*command, "--host", "rpi4"], cwd=ROOT, capture_output=True, text=True)
    assert unknown.returncode == 2 and "no host profile 'rpi4'" in unknown.stderr
