#!/usr/bin/env python3

import argparse
import csv
import hashlib
import json
import os
import re
import signal
import subprocess
import sys
import time
import tomllib
from collections import Counter
from pathlib import Path
from typing import Any

SUT_PROCESS_NAMES = ("wafer-runtime", "kuiperd")


class DiagnosticError(ValueError):
    pass


def _read_csv(path: Path, required_fields: tuple[str, ...]) -> list[dict[str, int]]:
    with path.open(newline="") as handle:
        reader = csv.DictReader(handle)
        fields = tuple(reader.fieldnames or ())
        missing = [field for field in required_fields if field not in fields]
        if missing:
            raise DiagnosticError(f"{path}: missing columns: {', '.join(missing)}")
        try:
            return [{field: int(row[field]) for field in required_fields} for row in reader]
        except (TypeError, ValueError) as error:
            raise DiagnosticError(f"{path}: invalid integer field: {error}") from error


def analyze_traces(published_path: Path, received_path: Path, metadata_path: Path) -> dict[str, Any]:
    published = _read_csv(published_path, ("seq", "ts_ns"))
    received = _read_csv(received_path, ("seq", "payload_ts_ns", "receive_ns", "latency_ns"))
    metadata = json.loads(metadata_path.read_text())

    published_by_seq = {row["seq"]: row for row in published}
    if len(published_by_seq) != len(published):
        raise DiagnosticError("publisher trace contains duplicate sequences")

    received_counts = Counter(row["seq"] for row in received)
    missing = sorted(set(published_by_seq) - set(received_counts))
    unexpected = sorted(set(received_counts) - set(published_by_seq))
    duplicates = sum(count - 1 for count in received_counts.values() if count > 1)
    errors: list[str] = []
    if missing:
        errors.append(f"missing sequences: {', '.join(map(str, missing))}")
    if unexpected:
        errors.append(f"unexpected sequences: {', '.join(map(str, unexpected))}")
    if duplicates:
        errors.append(f"duplicate sequences: {duplicates}")

    paired: list[dict[str, Any]] = []
    for received_row in received:
        published_row = published_by_seq.get(received_row["seq"])
        if published_row is None or received_counts[received_row["seq"]] != 1:
            continue
        if received_row["receive_ns"] < received_row["payload_ts_ns"]:
            errors.append(f"negative latency for sequence {received_row['seq']}")
            continue
        if received_row["payload_ts_ns"] != published_row["ts_ns"]:
            errors.append(f"timestamp changed for sequence {received_row['seq']}")
            continue
        latency_ns = received_row["receive_ns"] - received_row["payload_ts_ns"]
        if received_row["latency_ns"] != latency_ns:
            errors.append(f"latency mismatch for sequence {received_row['seq']}")
            continue
        paired.append(
            {
                "published": {"seq": published_row["seq"], "ts_ns": published_row["ts_ns"]},
                "received": {
                    "seq": received_row["seq"],
                    "payload_ts_ns": received_row["payload_ts_ns"],
                    "receive_ns": received_row["receive_ns"],
                },
                "latency_ns": latency_ns,
            }
        )

    sequence = metadata.get("sequence", {})
    expected_metadata = {
        "total_messages": len(received),
        "total_recorded": len(paired),
        "parse_errors": 0,
        "negative_latency_count": 0,
    }
    for field, expected in expected_metadata.items():
        if metadata.get(field) != expected:
            errors.append(f"subscriber metadata {field}={metadata.get(field)!r}, expected {expected}")
    if sequence.get("total_received") != len(paired):
        errors.append("subscriber sequence total does not match paired trace")
    if sequence.get("total_gaps") != len(missing):
        errors.append("subscriber gap count does not match paired trace")
    if sequence.get("total_duplicates") != duplicates:
        errors.append("subscriber duplicate count does not match paired trace")

    if errors:
        raise DiagnosticError("; ".join(errors))

    return {
        "offered_count": len(published),
        "received_count": len(received),
        "gaps": len(missing),
        "duplicates": duplicates,
        "latency_ns": {
            "p50": int(metadata["latency_p50_ns"]),
            "p95": int(metadata["latency_p95_ns"]),
            "p99": int(metadata["latency_p99_ns"]),
        },
        "raw_pairs": paired,
    }


def profile_provenance(profile_path: Path, rendered_template_sha256: str) -> dict[str, str]:
    raw = profile_path.read_bytes()
    profile = tomllib.loads(raw.decode())
    loadgen = profile.get("loadgen", {})
    configured_sha256 = str(loadgen.get("payload_template_sha256", ""))
    if configured_sha256 != rendered_template_sha256:
        raise DiagnosticError(
            "template SHA-256 mismatch: "
            f"profile={configured_sha256 or '<missing>'}, renderer={rendered_template_sha256}"
        )
    return {
        "path": str(profile_path),
        "sha256": hashlib.sha256(raw).hexdigest(),
        "payload_template": str(loadgen.get("payload_template", "")),
        "payload_template_sha256": configured_sha256,
    }


def process_audit() -> dict[str, Any]:
    completed = subprocess.run(
        ["ps", "-axo", "pid=,comm=,args="],
        check=True,
        capture_output=True,
        text=True,
    )
    matches = []
    for line in completed.stdout.splitlines():
        fields = line.strip().split(maxsplit=2)
        if len(fields) < 2:
            continue
        pid, command = fields[:2]
        if Path(command).name in SUT_PROCESS_NAMES:
            matches.append({"pid": int(pid), "command": command})
    return {"checked_processes": list(SUT_PROCESS_NAMES), "sut_pids": matches}


def build_artifact(
    measurements: dict[str, Any],
    provenance: dict[str, str],
    audit: dict[str, Any],
) -> dict[str, Any]:
    if audit.get("sut_pids"):
        raise DiagnosticError("SUT process detected during MQTT loopback diagnostic")
    return {
        "schema_version": 1,
        "system": "mqtt-loopback",
        "thesis_evidence": False,
        "measurement_boundary": "publisher wall-clock ts_ns to subscriber wall-clock receive_ns",
        "units": {"timestamps": "ns since Unix epoch", "latency": "ns", "counts": "messages"},
        "profile": provenance,
        "process_audit": audit,
        "measurements": measurements,
    }


def _renderer_sha256(binary: Path, profile: Path) -> str:
    environment = os.environ.copy()
    environment.setdefault("RUST_LOG", "info")
    completed = subprocess.run(
        [str(binary), "publish", "--profile-file", str(profile), "--dry-run"],
        check=True,
        capture_output=True,
        text=True,
        env=environment,
    )
    match = re.search(r"fingerprint\s*=\s*sha256:([0-9a-f]{64})", completed.stdout + completed.stderr)
    if not match:
        raise DiagnosticError("wafer-loadgen dry-run did not report a payload-template fingerprint")
    return match.group(1)


def _split_broker(broker: str) -> tuple[str, int]:
    host, separator, port = broker.rpartition(":")
    if separator and host and port.isdigit():
        return host, int(port)
    return broker, 1883


def _wait(process: subprocess.Popen[Any], timeout: float, name: str) -> None:
    try:
        return_code = process.wait(timeout=timeout)
    except subprocess.TimeoutExpired as error:
        raise DiagnosticError(f"{name} did not finish within {timeout:.0f}s") from error
    if return_code != 0:
        raise DiagnosticError(f"{name} exited with status {return_code}")


def run_live(args: argparse.Namespace) -> Path:
    binary = args.wafer_loadgen.resolve()
    profile_path = args.profile.resolve()
    output_dir = args.output_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=False)

    profile = tomllib.loads(profile_path.read_text())
    loadgen = profile["loadgen"]
    if loadgen.get("profile") != "steady":
        raise DiagnosticError("MQTT loopback diagnostic requires a steady profile")
    rate = int(loadgen["rate"])
    duration_secs = int(loadgen["duration_secs"])
    topic = str(loadgen["topic"])
    offered_count = rate * duration_secs
    host, port = _split_broker(args.broker)

    provenance = profile_provenance(profile_path, _renderer_sha256(binary, profile_path))
    before = process_audit()
    if before["sut_pids"]:
        raise DiagnosticError("SUT process already running before MQTT loopback diagnostic")

    published_trace = output_dir / "published.csv"
    received_trace = output_dir / "received.csv"
    subscriber_log = (output_dir / "subscriber.log").open("w")
    publisher_log = (output_dir / "publisher.log").open("w")
    environment = os.environ.copy()
    environment.setdefault("RUST_LOG", "info")
    subscriber = subprocess.Popen(
        [
            str(binary),
            "subscribe",
            "--broker",
            args.broker,
            "--topic",
            topic,
            "--output-dir",
            str(output_dir),
            "--total-messages",
            str(offered_count),
            "--client-id",
            f"mqtt-loopback-sub-{os.getpid()}",
            "--host-tag",
            args.host_tag,
            "--trace-file",
            str(received_trace),
        ],
        stdout=subscriber_log,
        stderr=subprocess.STDOUT,
        env=environment,
    )
    publisher: subprocess.Popen[Any] | None = None
    try:
        time.sleep(args.subscriber_ready_secs)
        publisher = subprocess.Popen(
            [
                str(binary),
                "publish",
                "--profile-file",
                str(profile_path),
                "--broker-host",
                host,
                "--broker-port",
                str(port),
                "--topic",
                topic,
                "--client-id",
                f"mqtt-loopback-pub-{os.getpid()}",
                "--trace-file",
                str(published_trace),
            ],
            stdout=publisher_log,
            stderr=subprocess.STDOUT,
            env=environment,
        )
        during = process_audit()
        _wait(publisher, duration_secs + args.timeout_grace_secs, "publisher")
        try:
            _wait(subscriber, args.timeout_grace_secs, "subscriber")
        except DiagnosticError:
            subscriber.send_signal(signal.SIGINT)
            _wait(subscriber, args.timeout_grace_secs, "subscriber after SIGINT")
    finally:
        for process in (publisher, subscriber):
            if process is not None and process.poll() is None:
                process.kill()
                process.wait()
        subscriber_log.close()
        publisher_log.close()

    after = process_audit()
    sut_pids = before["sut_pids"] + during["sut_pids"] + after["sut_pids"]
    audit = {
        "checked_processes": list(SUT_PROCESS_NAMES),
        "sut_pids": sut_pids,
        "launched_processes": [
            {"role": "publisher", "pid": publisher.pid if publisher else None, "executable": str(binary)},
            {"role": "subscriber", "pid": subscriber.pid, "executable": str(binary)},
        ],
    }
    measurements = analyze_traces(
        published_trace,
        received_trace,
        output_dir / "subscriber-metadata.json",
    )
    if measurements["offered_count"] != offered_count:
        raise DiagnosticError(
            f"publisher trace has {measurements['offered_count']} rows, expected {offered_count}"
        )
    artifact = build_artifact(measurements, provenance, audit)
    artifact_path = output_dir / "mqtt-loopback-diagnostic.json"
    artifact_path.write_text(json.dumps(artifact, indent=2) + "\n")
    return artifact_path


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Measure direct publisher-to-Mosquitto-to-subscriber latency")
    subparsers = parser.add_subparsers(dest="command", required=True)

    check = subparsers.add_parser("check", help="validate captured publisher/subscriber traces")
    check.add_argument("--published", type=Path, required=True)
    check.add_argument("--received", type=Path, required=True)
    check.add_argument("--subscriber-metadata", type=Path, required=True)
    check.add_argument("--profile", type=Path, required=True)
    check.add_argument("--template-sha256", required=True)
    check.add_argument("--output", type=Path, required=True)

    run = subparsers.add_parser("run", help="run a live loopback through an existing Mosquitto broker")
    run.add_argument("--wafer-loadgen", type=Path, default=Path("target/release/wafer-loadgen"))
    run.add_argument("--profile", type=Path, default=Path("eval/loadgen/telemetry-120b.toml"))
    run.add_argument("--output-dir", type=Path, required=True)
    run.add_argument("--broker", default="127.0.0.1:1883")
    run.add_argument("--host-tag", default="diagnostic")
    run.add_argument("--subscriber-ready-secs", type=float, default=1.0)
    run.add_argument("--timeout-grace-secs", type=float, default=10.0)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv or sys.argv[1:])
    try:
        if args.command == "run":
            artifact_path = run_live(args)
        else:
            measurements = analyze_traces(args.published, args.received, args.subscriber_metadata)
            provenance = profile_provenance(args.profile, args.template_sha256)
            artifact = build_artifact(measurements, provenance, process_audit())
            artifact_path = args.output
            artifact_path.write_text(json.dumps(artifact, indent=2) + "\n")
        print(artifact_path)
        return 0
    except (DiagnosticError, OSError, subprocess.SubprocessError, KeyError, json.JSONDecodeError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
