#!/usr/bin/env python3

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import os
import platform
import re
import signal
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from pi_telemetry import parse_pmic

PHASES = [
    "idle",
    "sut-core-load-1",
    "sut-core-load-2",
    "sut-core-load-3",
    "cpu-memory",
    "usb-write",
    "usb-read",
    "cpu-memory-usb",
]
USB_PHASES = {"usb-write", "usb-read", "cpu-memory-usb"}
IDLE_SECONDS = 120
PHASE_SECONDS = 300
CLEAN_BOOT_MAX_AGE_SECONDS = 600
MEMORY_MIB = 1024
USB_BYTES = 1024**3
SAMPLE_INTERVAL_SECONDS = 1.0
TELEMETRY_FIELDS = [
    "timestamp_utc",
    "monotonic_ns",
    "phase",
    "phase_elapsed_seconds",
    "boot_id",
    "temperature_millicelsius",
    "cpu_frequency_hz",
    "throttled",
    "pmic_internal_rail_proxy_watts",
    "memory_available_bytes",
    "memory_psi_some_avg10",
    "usb_read_bytes_per_second",
    "usb_write_bytes_per_second",
]
POWER_BOUNDARY = {
    "measurement": "rpi5-pmic-internal-rail-proxy",
    "is_total_input_power": False,
    "excludes": [
        "USB current",
        "devices connected directly to 5V",
        "PMIC conversion losses",
        "power-supply conversion losses",
    ],
}
IO_ERROR = re.compile(
    r"(?:I/O error|Buffer I/O error|blk_update_request|uas_eh_abort_handler|reset SuperSpeed USB device|EXT4-fs error|exfat.*error)",
    re.IGNORECASE,
)


class CharacterizationError(ValueError):
    pass


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def read_text(path: Path) -> str:
    try:
        return path.read_text().strip()
    except OSError as error:
        raise CharacterizationError(f"cannot read {path}: {error}") from error


def command_output(command: list[str]) -> str:
    completed = subprocess.run(command, capture_output=True, text=True, check=False)
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip() or f"exit {completed.returncode}"
        raise CharacterizationError(f"command failed ({' '.join(command)}): {detail}")
    return completed.stdout.strip()


def throttle_value(value: str) -> int:
    normalized = value.removeprefix("throttled=")
    try:
        return int(normalized, 0)
    except ValueError as error:
        raise CharacterizationError(f"invalid throttling value: {value}") from error


def source_state(path: Path) -> dict[str, Any]:
    if path.is_file():
        value = json.loads(path.read_text())
        return {
            "git_sha": value.get("git_sha", "unknown"),
            "git_dirty": value.get("git_dirty", "unknown"),
            "git_tags": value.get("git_tags", []),
            "provisional": bool(value.get("git_dirty", True)),
        }
    return {
        "git_sha": "unknown",
        "git_dirty": "unknown",
        "git_tags": [],
        "provisional": True,
    }


def validate_sample(sample: dict[str, Any], phase: str, boot_id: str) -> str | None:
    missing = [field for field in TELEMETRY_FIELDS if field not in sample]
    if missing:
        raise CharacterizationError(f"{phase} sample lacks fields: {', '.join(missing)}")
    if sample["phase"] != phase:
        raise CharacterizationError(
            f"sample phase mismatch: expected {phase}, got {sample['phase']}"
        )
    try:
        datetime.fromisoformat(str(sample["timestamp_utc"]).replace("Z", "+00:00"))
        numeric = {
            field: float(sample[field])
            for field in (
                "monotonic_ns",
                "phase_elapsed_seconds",
                "temperature_millicelsius",
                "cpu_frequency_hz",
                "pmic_internal_rail_proxy_watts",
                "memory_available_bytes",
                "memory_psi_some_avg10",
                "usb_read_bytes_per_second",
                "usb_write_bytes_per_second",
            )
        }
    except (TypeError, ValueError) as error:
        raise CharacterizationError(f"{phase} sample has an invalid field: {error}") from error
    if any(not math.isfinite(value) or value < 0 for value in numeric.values()):
        raise CharacterizationError(f"{phase} sample has a negative or non-finite metric")
    if numeric["cpu_frequency_hz"] <= 0:
        raise CharacterizationError(f"{phase} sample has no CPU frequency")
    if not str(sample["boot_id"]):
        raise CharacterizationError(f"{phase} sample has no boot ID")
    if sample["boot_id"] != boot_id:
        return "boot-id-changed"
    if int(sample["temperature_millicelsius"]) >= 75_000:
        return "temperature-at-or-above-75c"
    if throttle_value(str(sample["throttled"])) != 0:
        return "nonzero-throttling"
    return None


def validate_usb(phase: str, value: dict[str, Any]) -> str | None:
    if phase not in USB_PHASES:
        return None
    expected = value.get("expected_sha256")
    observed = value.get("observed_sha256")
    if not isinstance(expected, str) or not isinstance(observed, str):
        raise CharacterizationError(f"{phase} lacks USB SHA-256 evidence")
    if expected != observed:
        return "usb-checksum-mismatch"
    if int(value.get("bytes", 0)) <= 0 or float(value.get("elapsed_seconds", 0)) <= 0:
        raise CharacterizationError(f"{phase} lacks positive USB byte/duration evidence")
    return None


def evaluate(
    fixture: dict[str, Any], expected_boot_id: str
) -> tuple[list[dict[str, Any]], list[dict[str, Any]], list[str], list[dict[str, Any]], str | None]:
    if [phase.get("name") for phase in fixture.get("phases", [])] != PHASES:
        raise CharacterizationError("fixture must contain the exact phase order")

    boot_id = str(fixture.get("boot_id", ""))
    initial_throttled = str(fixture.get("initial_throttled", ""))
    stop_reason = None
    if boot_id != expected_boot_id:
        stop_reason = "clean-boot-id-mismatch"
    elif throttle_value(initial_throttled) != 0:
        stop_reason = "clean-boot-throttling-history"
    elif float(fixture.get("boot_age_seconds", 0)) > CLEAN_BOOT_MAX_AGE_SECONDS:
        stop_reason = "clean-boot-age-exceeded"

    telemetry: list[dict[str, Any]] = []
    kernel_errors: list[str] = []
    usb_checks: list[dict[str, Any]] = []
    phase_results: list[dict[str, Any]] = []

    for phase in fixture["phases"]:
        name = phase["name"]
        if stop_reason is not None:
            phase_results.append({"name": name, "status": "not-run", "stop_reason": None, "sample_count": 0})
            continue

        samples = phase.get("samples", [])
        maximum_rows = (
            IDLE_SECONDS if name == "idle" else PHASE_SECONDS
        ) // SAMPLE_INTERVAL_SECONDS + 2
        if len(samples) > maximum_rows:
            raise CharacterizationError(
                f"{name} exceeds maximum telemetry rows: {len(samples)} > {int(maximum_rows)}"
            )
        monotonic_values = [int(sample.get("monotonic_ns", -1)) for sample in samples]
        if any(
            current <= previous
            for previous, current in zip(monotonic_values, monotonic_values[1:])
        ):
            raise CharacterizationError(
                f"{name} samples require strictly increasing monotonic timestamps"
            )
        phase_reason = phase.get("failure_reason")
        if not samples and phase_reason is None:
            raise CharacterizationError(f"{name} has no telemetry samples")
        recorded_samples: list[dict[str, Any]] = []
        for sample in samples:
            sample_reason = validate_sample(sample, name, boot_id)
            recorded = {field: sample[field] for field in TELEMETRY_FIELDS}
            telemetry.append(recorded)
            recorded_samples.append(recorded)
            if sample_reason is not None:
                phase_reason = sample_reason
                break

        phase_kernel_errors = [str(line) for line in phase.get("kernel_io_errors", [])]
        if phase_reason is None and phase_kernel_errors:
            phase_reason = "kernel-io-error"
        kernel_errors.extend(f"{name}: {line}" for line in phase_kernel_errors)

        usb = dict(phase.get("usb", {}))
        if name in USB_PHASES:
            usb_check = {
                "phase": name,
                "bytes": int(usb.get("bytes", 0)),
                "elapsed_seconds": float(usb.get("elapsed_seconds", 0)),
                "expected_sha256": usb.get("expected_sha256"),
                "observed_sha256": usb.get("observed_sha256"),
            }
            usb_checks.append(usb_check)
            if phase_reason is None:
                phase_reason = validate_usb(name, usb_check)

        status = "failed" if phase_reason else "passed"
        phase_results.append(
            {
                "name": name,
                "status": status,
                "stop_reason": phase_reason,
                "sample_count": len(recorded_samples),
                "max_temperature_millicelsius": (
                    max(int(sample["temperature_millicelsius"]) for sample in recorded_samples)
                    if recorded_samples
                    else None
                ),
                "max_throttled": (
                    max(throttle_value(str(sample["throttled"])) for sample in recorded_samples)
                    if recorded_samples
                    else None
                ),
                "kernel_io_error_count": len(phase_kernel_errors),
                "failure_detail": phase.get("failure_detail"),
            }
        )
        stop_reason = phase_reason

    return phase_results, telemetry, kernel_errors, usb_checks, stop_reason


def write_outputs(
    output: Path,
    session_id: str,
    expected_boot_id: str,
    fixture: dict[str, Any],
    source: dict[str, Any],
) -> bool:
    phase_results, telemetry, kernel_errors, usb_checks, stop_reason = evaluate(
        fixture, expected_boot_id
    )
    output.mkdir(parents=False, exist_ok=True)

    with (output / "host-telemetry.csv").open("w", newline="") as stream:
        writer = csv.DictWriter(stream, fieldnames=TELEMETRY_FIELDS)
        writer.writeheader()
        writer.writerows(telemetry)

    (output / "kernel-io.log").write_text(
        "" if not kernel_errors else "\n".join(kernel_errors) + "\n"
    )
    (output / "usb-integrity.json").write_text(
        json.dumps(
            {
                "schema_version": 1,
                "checks": usb_checks,
                "checksum_mismatch_count": sum(
                    item["expected_sha256"] != item["observed_sha256"] for item in usb_checks
                ),
            },
            indent=2,
            sort_keys=True,
        )
        + "\n"
    )

    passed_names = {phase["name"] for phase in phase_results if phase["status"] == "passed"}
    diagnostic_phases = set(PHASES[:-1])
    evaluated_diagnostic_gate = diagnostic_phases.issubset(passed_names)
    evaluated_final_gate = set(PHASES).issubset(passed_names)
    execution_mode = str(fixture.get("execution_mode", "unknown"))
    admission_eligible = execution_mode == "real-hardware"
    diagnostic_admission = admission_eligible and evaluated_diagnostic_gate
    final_admission = admission_eligible and evaluated_final_gate
    status = (
        "passed"
        if final_admission
        else "synthetic-pass"
        if execution_mode == "fixture-synthetic" and evaluated_final_gate
        else "failed"
    )
    receipt = {
        "schema_version": 1,
        "experiment_id": "e-host-thermal-storage",
        "session_id": session_id,
        "sample_unit": "clean-boot host characterization session",
        "evidence_class": "diagnostic",
        "thesis_evidence": False,
        "n30_admitted": False,
        "generated_at": utc_now(),
        "source_state": source,
        "expected_boot_id": expected_boot_id,
        "observed_boot_id": fixture.get("boot_id"),
        "initial_throttled": fixture.get("initial_throttled"),
        "phase_order": PHASES,
        "phase_durations_seconds": {
            "idle": IDLE_SECONDS,
            "all_other_phases": PHASE_SECONDS,
        },
        "sample_interval_seconds": SAMPLE_INTERVAL_SECONDS,
        "clean_boot_max_age_seconds": CLEAN_BOOT_MAX_AGE_SECONDS,
        "memory_workload_mib": MEMORY_MIB,
        "usb_corpus_bytes": USB_BYTES,
        "phases": phase_results,
        "temperature_stop_millicelsius": 75_000,
        "stop_on_first_failure": True,
        "stop_reason": stop_reason,
        "execution_mode": execution_mode,
        "admission_eligible": admission_eligible,
        "evaluated_diagnostic_gate": evaluated_diagnostic_gate,
        "evaluated_final_gate": evaluated_final_gate,
        "diagnostic_admission": diagnostic_admission,
        "final_admission": final_admission,
        "status": status,
        "power_boundary": POWER_BOUNDARY,
        "no_pool_with": ["performance experiments", "prior diagnostic rehearsals"],
    }
    (output / "host-load-ladder.json").write_text(
        json.dumps(receipt, indent=2, sort_keys=True) + "\n"
    )
    return evaluated_final_gate


def boot_age_seconds() -> float:
    return float(read_text(Path("/proc/uptime")).split()[0])


def read_frequency_hz() -> int:
    paths = sorted(Path("/sys/devices/system/cpu").glob("cpu*/cpufreq/scaling_cur_freq"))
    values = [int(read_text(path)) * 1000 for path in paths]
    if not values:
        raise CharacterizationError("CPU frequency is unavailable")
    return max(values)


def read_memory() -> tuple[int, float]:
    memory = read_text(Path("/proc/meminfo"))
    match = re.search(r"^MemAvailable:\s+(\d+)\s+kB$", memory, re.MULTILINE)
    if match is None:
        raise CharacterizationError("MemAvailable is unavailable")
    pressure = read_text(Path("/proc/pressure/memory"))
    pressure_match = re.search(r"^some\s+avg10=([0-9.]+)", pressure, re.MULTILINE)
    if pressure_match is None:
        raise CharacterizationError("memory PSI is unavailable")
    return int(match.group(1)) * 1024, float(pressure_match.group(1))


def host_sample(phase: str, elapsed: float) -> dict[str, Any]:
    rails = parse_pmic(command_output(["vcgencmd", "pmic_read_adc"]))
    if not rails:
        raise CharacterizationError("PMIC internal-rail samples are unavailable")
    available, pressure = read_memory()
    return {
        "timestamp_utc": utc_now(),
        "monotonic_ns": time.monotonic_ns(),
        "phase": phase,
        "phase_elapsed_seconds": round(elapsed, 3),
        "boot_id": read_text(Path("/proc/sys/kernel/random/boot_id")),
        "temperature_millicelsius": int(read_text(Path("/sys/class/thermal/thermal_zone0/temp"))),
        "cpu_frequency_hz": read_frequency_hz(),
        "throttled": command_output(["vcgencmd", "get_throttled"]).removeprefix("throttled="),
        "pmic_internal_rail_proxy_watts": sum(float(rail["power_w"]) for rail in rails),
        "memory_available_bytes": available,
        "memory_psi_some_avg10": pressure,
        "usb_read_bytes_per_second": 0.0,
        "usb_write_bytes_per_second": 0.0,
    }


def worker_command(kind: str, seconds: int, argument: str = "") -> list[str]:
    return [sys.executable, str(Path(__file__).resolve()), "worker", kind, str(seconds), argument]


def start_workers(phase: str, seconds: int, output: Path) -> list[subprocess.Popen[str]]:
    commands: list[tuple[list[str], str | None]] = []
    if phase.startswith("sut-core-load-"):
        count = int(phase.rsplit("-", 1)[1])
        commands.extend((worker_command("cpu", seconds), str(cpu)) for cpu in range(1, count + 1))
    elif phase == "cpu-memory":
        commands.extend((worker_command("cpu", seconds), str(cpu)) for cpu in (2, 3))
        commands.append((worker_command("memory", seconds, str(MEMORY_MIB)), "1"))
    elif phase == "usb-write":
        commands.append((worker_command("write", seconds, f"{output / 'usb-corpus.bin'}:{USB_BYTES}"), None))
    elif phase == "usb-read":
        commands.append((worker_command("read", seconds, str(output / "usb-corpus.bin")), None))
    elif phase == "cpu-memory-usb":
        commands.extend((worker_command("cpu", seconds), str(cpu)) for cpu in (2, 3))
        commands.append((worker_command("memory", seconds, str(MEMORY_MIB)), "1"))
        commands.append((worker_command("write", seconds, f"{output / 'combined-usb-corpus.bin'}:{USB_BYTES}"), None))

    processes = []
    for command, affinity in commands:
        if affinity:
            command = ["taskset", "-c", affinity, *command]
        processes.append(
            subprocess.Popen(
                command,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                start_new_session=True,
            )
        )
    return processes


def stop_workers(processes: list[subprocess.Popen[str]]) -> None:
    for process in processes:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGTERM)
    for process in processes:
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()


def kernel_io_errors(since: str) -> list[str]:
    output = command_output(["journalctl", "-k", "--since", since, "--no-pager", "--output=short-iso"])
    return [line for line in output.splitlines() if IO_ERROR.search(line)][:100]


def parse_worker_result(process: subprocess.Popen[str]) -> dict[str, Any]:
    stdout, stderr = process.communicate()
    if process.returncode != 0:
        raise CharacterizationError(f"load worker failed: {stderr.strip() or process.returncode}")
    lines = [line for line in stdout.splitlines() if line.strip()]
    return json.loads(lines[-1]) if lines else {}


def run_real(args: argparse.Namespace) -> dict[str, Any]:
    if platform.system() != "Linux" or platform.machine() != "aarch64":
        raise CharacterizationError("real characterization requires Linux aarch64")
    output = args.output_dir.resolve()
    approved_root = Path("/mnt/wafer-results/raw/e-host-thermal-storage")
    if not output.is_relative_to(approved_root):
        raise CharacterizationError(f"real output must be below {approved_root}")
    if command_output(["findmnt", "-n", "-o", "FSTYPE", "/mnt/wafer-results"]) != "exfat":
        raise CharacterizationError("/mnt/wafer-results is not an exFAT mount")

    boot_id = read_text(Path("/proc/sys/kernel/random/boot_id"))
    initial = command_output(["vcgencmd", "get_throttled"]).removeprefix("throttled=")
    fixture: dict[str, Any] = {
        "boot_id": boot_id,
        "boot_age_seconds": boot_age_seconds(),
        "initial_throttled": initial,
        "execution_mode": "real-hardware",
        "phases": [],
    }
    if (
        boot_id != args.expected_boot_id
        or throttle_value(initial) != 0
        or fixture["boot_age_seconds"] > CLEAN_BOOT_MAX_AGE_SECONDS
    ):
        fixture["phases"] = [{"name": phase, "samples": [], "kernel_io_errors": [], "usb": {}} for phase in PHASES]
        return fixture

    output.parent.mkdir(parents=True, exist_ok=True)
    output.mkdir(parents=False)
    try:
        for phase in PHASES:
            seconds = IDLE_SECONDS if phase == "idle" else PHASE_SECONDS
            started_utc = utc_now()
            started = time.monotonic()
            processes: list[subprocess.Popen[str]] = []
            samples: list[dict[str, Any]] = []
            failure = None
            failure_detail = None
            errors: list[str] = []
            try:
                processes = start_workers(phase, seconds, output)
                while time.monotonic() - started < seconds:
                    row = host_sample(phase, time.monotonic() - started)
                    samples.append(row)
                    print(
                        f"[{row['timestamp_utc']}] phase={phase} "
                        f"elapsed={row['phase_elapsed_seconds']}s "
                        f"temperature={row['temperature_millicelsius']} "
                        f"throttled={row['throttled']}",
                        flush=True,
                    )
                    failure = validate_sample(row, phase, boot_id)
                    errors = kernel_io_errors(started_utc)
                    if failure is None and errors:
                        failure = "kernel-io-error"
                    if failure is None and any(
                        process.poll() not in (None, 0) for process in processes
                    ):
                        failure = "workload-failure"
                    if failure is not None:
                        break
                    time.sleep(SAMPLE_INTERVAL_SECONDS)
            except KeyboardInterrupt:
                failure = "interrupted"
                failure_detail = "received interrupt"
                stop_workers(processes)
            except (CharacterizationError, OSError) as error:
                failure = "instrumentation-or-workload-error"
                failure_detail = str(error)
                stop_workers(processes)
            except BaseException:
                stop_workers(processes)
                raise
            if failure is not None:
                print(
                    f"[{utc_now()}] phase={phase} stopped reason={failure}",
                    flush=True,
                )
                stop_workers(processes)

            try:
                results = (
                    [parse_worker_result(process) for process in processes]
                    if failure is None
                    else []
                )
            except CharacterizationError as error:
                results = []
                failure = "workload-failure"
                failure_detail = str(error)
            usb = next(
                (result for result in results if result.get("kind") in {"write", "read"}),
                {},
            )
            if usb:
                field = "usb_write_bytes_per_second" if usb["kind"] == "write" else "usb_read_bytes_per_second"
                for row in samples:
                    row[field] = usb["bytes"] / usb["elapsed_seconds"]
            if failure is None and phase in USB_PHASES:
                failure = validate_usb(phase, usb)
            if failure is not None:
                stop_workers(processes)
            fixture["phases"].append(
                {
                    "name": phase,
                    "samples": samples,
                    "kernel_io_errors": errors,
                    "usb": usb,
                    "failure_reason": failure,
                    "failure_detail": failure_detail,
                }
            )
            if failure is not None or errors:
                break
    finally:
        existing = {phase["name"] for phase in fixture["phases"]}
        fixture["phases"].extend(
            {"name": phase, "samples": [], "kernel_io_errors": [], "usb": {}, "failure_reason": None}
            for phase in PHASES
            if phase not in existing
        )
    return fixture


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while chunk := stream.read(4 * 1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def sha256_zero_bytes(size: int) -> str:
    digest = hashlib.sha256()
    chunk = b"\0" * min(4 * 1024 * 1024, size)
    remaining = size
    while remaining:
        block = chunk[:remaining]
        digest.update(block)
        remaining -= len(block)
    return digest.hexdigest()


def run_worker(kind: str, seconds: int, argument: str) -> int:
    started = time.monotonic()
    deadline = started + seconds
    if kind == "cpu":
        value = 1
        while time.monotonic() < deadline:
            value = (value * 1_664_525 + 1_013_904_223) & 0xFFFFFFFF
        print(json.dumps({"kind": kind, "value": value}))
        return 0
    if kind == "memory":
        data = bytearray(int(argument) * 1024 * 1024)
        while time.monotonic() < deadline:
            for index in range(0, len(data), 4096):
                data[index] = (data[index] + 1) & 0xFF
        print(json.dumps({"kind": kind, "bytes": len(data)}))
        return 0
    if kind == "write":
        path_text, byte_text = argument.rsplit(":", 1)
        path = Path(path_text)
        target_bytes = int(byte_text)
        chunk = b"\0" * min(4 * 1024 * 1024, target_bytes)
        expected_digest = sha256_zero_bytes(target_bytes)
        written = 0
        while time.monotonic() < deadline or written == 0:
            with path.open("wb") as stream:
                remaining = target_bytes
                while remaining:
                    block = chunk[:remaining]
                    stream.write(block)
                    written += len(block)
                    remaining -= len(block)
                stream.flush()
                os.fsync(stream.fileno())
        elapsed = time.monotonic() - started
        observed_digest = sha256_file(path)
        if hasattr(os, "posix_fadvise"):
            with path.open("rb") as stream:
                os.posix_fadvise(stream.fileno(), 0, 0, os.POSIX_FADV_DONTNEED)
        print(
            json.dumps(
                {
                    "kind": kind,
                    "bytes": written,
                    "elapsed_seconds": elapsed,
                    "expected_sha256": expected_digest,
                    "observed_sha256": observed_digest,
                }
            )
        )
        return 0
    if kind == "read":
        path = Path(argument)
        if hasattr(os, "posix_fadvise"):
            with path.open("rb") as stream:
                os.posix_fadvise(stream.fileno(), 0, 0, os.POSIX_FADV_DONTNEED)
        expected = sha256_zero_bytes(path.stat().st_size)
        read_bytes = 0
        observed = expected
        while time.monotonic() < deadline or read_bytes == 0:
            digest = hashlib.sha256()
            with path.open("rb") as stream:
                while chunk := stream.read(4 * 1024 * 1024):
                    digest.update(chunk)
                    read_bytes += len(chunk)
            observed = digest.hexdigest()
            if observed != expected:
                break
        elapsed = time.monotonic() - started
        print(json.dumps({"kind": kind, "bytes": read_bytes, "elapsed_seconds": elapsed, "expected_sha256": expected, "observed_sha256": observed}))
        return 0
    raise CharacterizationError(f"unknown worker kind: {kind}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Stop-on-first-failure Raspberry Pi 5 host characterization")
    subparsers = parser.add_subparsers(dest="command")
    worker = subparsers.add_parser("worker")
    worker.add_argument("kind", choices=["cpu", "memory", "write", "read"])
    worker.add_argument("seconds", type=int)
    worker.add_argument("argument", nargs="?", default="")

    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--session-id")
    parser.add_argument("--expected-boot-id")
    parser.add_argument("--fixture-json", type=Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        if args.command == "worker":
            return run_worker(args.kind, args.seconds, args.argument)
        if not args.output_dir or not args.session_id or not args.expected_boot_id:
            raise CharacterizationError(
                "--output-dir, --session-id, and --expected-boot-id are required"
            )
        if args.output_dir.exists():
            raise CharacterizationError(f"output directory already exists: {args.output_dir}")
        state_path = Path(
            os.environ.get(
                "WAFER_SOURCE_STATE",
                Path(__file__).resolve().parents[3] / "SOURCE_STATE.json",
            )
        )
        state = source_state(state_path)
        if args.fixture_json:
            fixture = json.loads(args.fixture_json.read_text())
            fixture["execution_mode"] = "fixture-synthetic"
            evaluate(fixture, args.expected_boot_id)
        else:
            if state["provisional"] or not re.fullmatch(
                r"[0-9a-f]{40}", str(state["git_sha"])
            ):
                raise CharacterizationError(
                    "real characterization requires a clean deployed SOURCE_STATE.json"
                )
            fixture = run_real(args)
        passed = write_outputs(
            args.output_dir,
            args.session_id,
            args.expected_boot_id,
            fixture,
            state,
        )
        print(f"host characterization {'passed' if passed else 'failed'}: {args.output_dir}")
        return 0 if passed else 1
    except (CharacterizationError, OSError, json.JSONDecodeError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
