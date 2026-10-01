"""Host profiles from the `hosts` map of the canonical matrix.

The canonical runner, the preflight validator and the result verifier all
read the expected host state from here, so a Pi, Jetson or x86 batch is
checked against its own profile and never against another host's.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

CANONICAL_MATRIX = Path(__file__).resolve().parents[2] / "canonical-matrix.json"


@dataclass(frozen=True)
class HostProfile:
    tag: str
    role: str
    arch: str
    hardware_model_contains: str
    cpu_governors: tuple[str, ...]
    housekeeping_cpus: str
    sut_cpus: str
    support_cpus: str
    throttled: str
    allowed_facts: dict[str, tuple[str, ...]]
    power_measurement: str

    def fact_errors(self, facts: dict) -> list[str]:
        """What in host facts or leaf metadata differs from this profile."""
        errors = []
        for label, key, value in (
            ("host tag", "host_tag", self.tag),
            ("architecture", "arch", self.arch),
            ("housekeeping CPUs", "housekeeping_cpus", self.housekeeping_cpus),
            ("default IRQ CPUs", "irq_default_cpus", self.housekeeping_cpus),
            ("throttling", "throttled", self.throttled),
        ):
            if facts.get(key) != value:
                errors.append(f"{label} must be {value!r}, got {facts.get(key)!r}")
        if facts.get("isolated_cpus") != "":
            errors.append(f"isolated CPUs must be empty, got {facts.get('isolated_cpus')!r}")
        if self.hardware_model_contains not in str(facts.get("hardware_model", "")):
            errors.append(f"hardware model must contain {self.hardware_model_contains!r}")
        if list(facts.get("cpu_governors") or []) != list(self.cpu_governors):
            errors.append(f"CPU governor must be {' + '.join(self.cpu_governors)}")
        for key, allowed in self.allowed_facts.items():
            if facts.get(key) not in allowed:
                errors.append(f"{key} must be one of {list(allowed)!r}, got {facts.get(key)!r}")
        return errors


def host_profiles(matrix: dict) -> dict[str, HostProfile]:
    hosts = matrix.get("hosts")
    if not isinstance(hosts, dict) or not hosts:
        raise ValueError("canonical matrix has no hosts map")
    return {
        tag: HostProfile(
            tag=tag,
            role=str(entry["role"]),
            arch=str(entry["arch"]),
            hardware_model_contains=str(entry["hardware_model_contains"]),
            cpu_governors=tuple(entry["cpu_governors"]),
            housekeeping_cpus=str(entry["housekeeping_cpus"]),
            sut_cpus=str(entry["sut_cpus"]),
            support_cpus=str(entry["support_cpus"]),
            throttled=str(entry["throttled"]),
            allowed_facts={key: tuple(values) for key, values in entry["allowed_facts"].items()},
            power_measurement=str(entry["power_measurement"]),
        )
        for tag, entry in hosts.items()
    }


def host_profile(tag: str, matrix_path: Path = CANONICAL_MATRIX) -> HostProfile:
    profiles = host_profiles(json.loads(matrix_path.read_text()))
    if tag not in profiles:
        raise ValueError(f"no host profile {tag!r}; known: {', '.join(sorted(profiles))}")
    return profiles[tag]
