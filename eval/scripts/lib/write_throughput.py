#!/usr/bin/env python3

import csv
import json
import sys
from pathlib import Path


def write_throughput(metadata_path: Path, output_path: Path) -> None:
    metadata = json.loads(metadata_path.read_text())
    started = int(metadata["started_at_ns"])
    ended = int(metadata["ended_at_ns"])
    duration_ns = ended - started
    if duration_ns <= 0:
        raise ValueError("subscriber duration must be positive")
    messages = int(metadata["total_recorded"])
    throughput = messages / (duration_ns / 1_000_000_000)
    with output_path.open("w", newline="") as stream:
        writer = csv.writer(stream)
        writer.writerow(
            ["timestamp_ns", "messages_received", "throughput_msg_s", "duration_ns"]
        )
        writer.writerow([ended, messages, f"{throughput:.6f}", duration_ns])


def main() -> int:
    if len(sys.argv) != 3:
        print(f"usage: {sys.argv[0]} <subscriber-metadata.json> <throughput.csv>", file=sys.stderr)
        return 2
    try:
        write_throughput(Path(sys.argv[1]), Path(sys.argv[2]))
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
