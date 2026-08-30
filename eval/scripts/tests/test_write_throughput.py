#!/usr/bin/env python3

import csv
import json
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "eval/scripts/lib"))

from write_throughput import write_throughput  # noqa: E402


def test_throughput_uses_subscriber_measurement_window() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        metadata = root / "subscriber-metadata.json"
        output = root / "throughput.csv"
        metadata.write_text(
            json.dumps(
                {
                    "started_at_ns": 1_000_000_000,
                    "ended_at_ns": 11_000_000_000,
                    "total_recorded": 9_000,
                }
            )
        )
        write_throughput(metadata, output)
        with output.open(newline="") as stream:
            row = next(csv.DictReader(stream))
    assert int(row["messages_received"]) == 9_000
    assert float(row["throughput_msg_s"]) == 900.0
    assert int(row["duration_ns"]) == 10_000_000_000


if __name__ == "__main__":
    test_throughput_uses_subscriber_measurement_window()
    print("throughput summary tests: PASS")
