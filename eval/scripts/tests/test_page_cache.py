#!/usr/bin/env python3

import json
import mmap
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "eval/scripts/lib"))

import page_cache  # noqa: E402


def test_residency_counts_the_pages_of_a_file(tmp_path: Path) -> None:
    path = tmp_path / "plugin.wasm"
    path.write_bytes(b"\1" * (3 * mmap.PAGESIZE + 1))
    result = page_cache.residency(path)
    assert result["path"] == str(path)
    assert result["total_pages"] == 4
    assert 0 <= result["resident_pages"] <= 4


def test_residency_of_an_empty_or_missing_file(tmp_path: Path) -> None:
    empty = tmp_path / "empty.toml"
    empty.write_bytes(b"")
    assert page_cache.resident_pages(empty) == (0, 0)
    missing = page_cache.residency(tmp_path / "missing.wasm")
    assert missing["resident_pages"] is None and missing["total_pages"] is None


def test_main_adds_residency_to_the_receipt_and_prints_a_timestamp(tmp_path: Path, capsys) -> None:
    receipt = tmp_path / "startup-preparation.json"
    receipt.write_text('{"cache_state":"cold","action":"drop-linux-page-cache","completed_before_timing":true}\n')
    binary = tmp_path / "wafer"
    binary.write_bytes(b"\0" * 100)
    config = tmp_path / "config.toml"
    config.write_text("[pipeline]\n")
    plugin = tmp_path / "plugin.wasm"
    plugin.write_bytes(b"\0" * 100)

    assert page_cache.main(["page_cache.py", str(receipt), str(binary), str(config), str(plugin)]) == 0

    written = json.loads(receipt.read_text())
    assert written["cache_state"] == "cold"
    residency = written["page_cache_residency"]
    assert residency["runtime_binary"]["path"] == str(binary)
    assert residency["config"]["path"] == str(config)
    assert [entry["path"] for entry in residency["plugins"]] == [str(plugin)]
    assert residency["plugins"][0]["total_pages"] == 1
    assert int(capsys.readouterr().out) > 1_600_000_000_000_000_000
