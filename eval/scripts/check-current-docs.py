#!/usr/bin/env python3
"""Check current evaluation docs against source and frozen matrix semantics."""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

import tomllib

ROOT = Path(__file__).resolve().parents[2]
CURRENT_DOCS = (
    "docs/rfcs/RFC-008-evaluation-harness.md",
    "docs/adr/0009-filter-as-first-class-node.md",
    "docs/adr/0013-aot-cache-and-metering.md",
    "docs/interfaces/config-schema.md",
    "docs/requirements/non-functional.md",
    "docs/architecture/01-goals-and-constraints.md",
    "docs/architecture/05-deployment.md",
    "docs/architecture/06-crosscutting-concepts.md",
    "docs/architecture/07-quality-requirements.md",
    "docs/architecture/08-risks.md",
    "docs/eval/pi5-experiment-runbook.md",
    "docs/operations/configuration.md",
    "docs/status/evaluation-progress.md",
    "docs/status/canonical-readiness.md",
    "docs/benchmarks/README.md",
    "docs/benchmarks/ekuiper-comparator.md",
    "docs/benchmarks/hot-swap.md",
    "eval/analysis/notebooks/README.md",
)
HISTORICAL_MARKER = "<!-- historical-diagnostic-below -->"
HISTORICAL_FILE_MARKER = "<!-- historical-diagnostic-file -->"
HISTORICAL_DOCS = (
    "docs/benchmarks/binary-sizes.md",
    "docs/benchmarks/ekuiper-tail-diagnostic.md",
    "docs/benchmarks/methodology-validation.md",
    "docs/benchmarks/rq-summary.md",
    "docs/benchmarks/rq2-attacks.md",
)
FORBIDDEN = (
    (
        re.compile(
            r"E-Perf-1\s*(?:\||:)\s*(?:throughput saturation|saturation|capacity)",
            re.IGNORECASE,
        ),
        "E-Perf-1 described as capacity",
    ),
    (
        re.compile(
            r"E-Perf-9[^\n]{0,60}(?:AOT cold|AOT-cache benefit|proves[^\n]{0,20}AOT)",
            re.IGNORECASE,
        ),
        "E-Perf-9 described as compiled-cache evidence",
    ),
    (
        re.compile(
            r"Pipeline A[^\n]{0,100}(?:four[- ]stage|4[- ]stage)", re.IGNORECASE
        ),
        "Pipeline A described as four-stage",
    ),
    (
        re.compile(
            r"E-Swap-4[^\n]{0,160}(?:constant[- ]2,?000|50 swaps|52 swaps)",
            re.IGNORECASE,
        ),
        "E-Swap-4 described as the historical pseudo-burst",
    ),
    (
        re.compile(
            r"(?:fuel|epoch)[^\n]{0,50}default[^\n]{0,50}(?:10,?000,?000|500,?000|100 ticks)",
            re.IGNORECASE,
        ),
        "evaluation budget described as runtime default",
    ),
)
REQUIRED = (
    "Runtime fuel budgets and the epoch deadline default to `None`",
    "[1,000, 4,000, 8,000, 15,000, 16,000]",
    "E-Perf-1 is a matched 1,000 msg/s operating point, not capacity",
    "E-Perf-9 is Linux filesystem page-cache evidence",
    "E-Perf-5 remains `PENDING`",
    "PMIC telemetry is an internal-rail proxy",
    "Pipeline A is `MQTT source -> threshold filter -> MQTT sink`",
    "100 separate drain buckets over `[120s,130s)`",
    "campaign_started=false",
)


def current_text(path: Path) -> str:
    text = path.read_text()
    return text.split(HISTORICAL_MARKER, maxsplit=1)[0]


def stale_claims(text: str) -> list[str]:
    return [message for pattern, message in FORBIDDEN if pattern.search(text)]


def check_links(path: Path, text: str) -> list[str]:
    errors = []
    for target in re.findall(r"\[[^\]]+\]\(([^)]+)\)", text):
        clean = target.split("#", maxsplit=1)[0]
        if not clean or re.match(r"(?:https?|mailto):", clean):
            continue
        if not (path.parent / clean).resolve().exists():
            errors.append(f"{path.relative_to(ROOT)}: missing link target {target}")
    return errors


def load_toml(relative: str) -> dict:
    return tomllib.loads((ROOT / relative).read_text())


def config_errors() -> list[str]:
    errors = []
    engine_source = (ROOT / "crates/wafer-types/src/config/engine.rs").read_text()
    if "epoch_deadline: None" not in engine_source:
        errors.append("EngineConfig source no longer defaults epoch_deadline to None")
    fuel_default = re.search(
        r"derive\([^)]*Default[^)]*\)\]\s*pub struct FuelBudgets", engine_source
    )
    if fuel_default is None or any(
        f"pub {category}: Option<NonZeroU64>" not in engine_source
        for category in ("transform", "filter", "router")
    ):
        errors.append(
            "FuelBudgets source no longer defines all-None optional category defaults"
        )
    source_defaults = {
        "default_epoch_tick_ms": "10",
        "default_queue_capacity": "1024",
        "default_memory_transform": "64 * 1024 * 1024",
        "default_memory_filter": "16 * 1024 * 1024",
        "default_memory_router": "16 * 1024 * 1024",
    }
    for function, value in source_defaults.items():
        if (
            re.search(
                rf"const fn {function}\(\) -> \w+ \{{\s*{re.escape(value)}\s*\}}",
                engine_source,
            )
            is None
        ):
            errors.append(f"EngineConfig source default changed: {function}")

    modes = {
        "neither": ("eval/configs/pipeline-c-neither.toml", None, None),
        "fuel-only": ("eval/configs/pipeline-c-fuel-only.toml", 10_000_000, None),
        "epoch-only": ("eval/configs/pipeline-c-epoch-only.toml", None, 100),
        "both": ("eval/configs/pipeline-c-passthrough.toml", 10_000_000, 100),
    }
    for mode, (path, fuel, epoch) in modes.items():
        config = load_toml(path)
        actual_fuel = config.get("engine", {}).get("fuel", {}).get("transform")
        actual_epoch = config.get("engine", {}).get("epoch_deadline")
        if (actual_fuel, actual_epoch) != (fuel, epoch):
            errors.append(
                f"{mode} parsed values {(actual_fuel, actual_epoch)} != {(fuel, epoch)}"
            )

    matrix = json.loads((ROOT / "eval/canonical-matrix.json").read_text())
    campaign = matrix["final_campaign"]
    if campaign.get("canonical_metering") != {
        "policy": "explicit-fuel-and-epoch",
        "fuel": {"transform": 10_000_000, "filter": 500_000, "router": 500_000},
        "epoch_deadline": 100,
        "epoch_tick_ms": 10,
    }:
        errors.append("final canonical metering policy differs from documented values")
    if campaign.get("expected_schedule_records") != 2105:
        errors.append("final schedule record count is not 2105")
    if campaign.get("expected_measured_leaves") != 1893:
        errors.append("final measured leaf count is not 1893")
    if campaign.get("capacity_grid", {}).get("common_rate_points_msg_s") != [
        1_000,
        4_000,
        8_000,
        15_000,
        16_000,
    ]:
        errors.append("final capacity grid differs from the documented common grid")
    if matrix["experiments"]["e-swap-4"].get("sink_tail_policy") != {
        "alignment_clock": "unix-epoch-source-sink-alignment",
        "primary_start_secs": 0,
        "primary_end_secs": 120,
        "primary_bucket_count": 1_200,
        "drain_start_secs": 120,
        "drain_end_secs": 130,
        "drain_bucket_count": 100,
        "bucket_width_ms": 100,
        "after_drain_events_allowed": 0,
        "source_completion_deadline_secs": 130,
        "require_full_sequence_reconciliation": True,
    }:
        errors.append("E-Swap-4 sink tail policy differs from current documentation")
    return errors


def audit() -> list[str]:
    errors = []
    combined = []
    for relative in CURRENT_DOCS:
        path = ROOT / relative
        if not path.is_file():
            errors.append(f"missing current document: {relative}")
            continue
        text = current_text(path)
        combined.append(text)
        errors.extend(f"{relative}: {message}" for message in stale_claims(text))
        errors.extend(check_links(path, text))
    for relative in HISTORICAL_DOCS:
        path = ROOT / relative
        if not path.is_file() or HISTORICAL_FILE_MARKER not in path.read_text():
            errors.append(f"historical diagnostic document lacks marker: {relative}")
    corpus = "\n".join(combined)
    for required in REQUIRED:
        if required not in corpus:
            errors.append(f"current docs lack required statement: {required}")
    errors.extend(config_errors())
    return errors


def main() -> int:
    errors = audit()
    if errors:
        print("current documentation audit: FAIL", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    print(
        f"current documentation audit: PASS ({len(CURRENT_DOCS)} documents; "
        "runtime defaults, metering modes, matrix counts, rates, claims, and links checked)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
