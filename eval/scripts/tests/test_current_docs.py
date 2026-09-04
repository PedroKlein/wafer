import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "current_docs", ROOT / "scripts/check-current-docs.py"
)
assert SPEC and SPEC.loader
DOCS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(DOCS)


def test_current_document_audit_passes() -> None:
    assert DOCS.audit() == []


def test_stale_current_claims_fail_but_historical_suffix_is_excluded(
    tmp_path: Path,
) -> None:
    assert DOCS.stale_claims("E-Perf-1: saturation throughput")
    assert DOCS.stale_claims("E-Perf-9 proves an AOT cache benefit")
    assert DOCS.stale_claims("Pipeline A is a four-stage chain")
    assert DOCS.stale_claims("E-Swap-4 performs 50 swaps at constant 2000 msg/s")

    path = tmp_path / "doc.md"
    path.write_text(
        "E-Perf-1 is target load.\n"
        f"{DOCS.HISTORICAL_MARKER}\n"
        "E-Perf-1 saturation throughput was an old diagnostic label.\n"
    )
    assert DOCS.stale_claims(DOCS.current_text(path)) == []


def test_documented_config_values_match_source_and_final_configs() -> None:
    assert DOCS.config_errors() == []


def test_historical_benchmark_docs_are_explicitly_marked() -> None:
    for relative in DOCS.HISTORICAL_DOCS:
        assert DOCS.HISTORICAL_FILE_MARKER in (DOCS.ROOT / relative).read_text()
