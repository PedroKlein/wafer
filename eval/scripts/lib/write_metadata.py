#!/usr/bin/env python3
"""Merge run-experiment.sh metadata + runtime-provenance.json sidecar.

Extracted from `eval/scripts/run-experiment.sh::_write_metadata` so both the
production shell path and integration tests exercise the same code. Runtime
provenance keys (wasmtime_version, rustc_version, wafer_runtime_sha256,
wafer_plugin_hashes, kernel, config_sha256) are authoritative when the
sidecar exists — they observed the actual bytes loaded — and fall back to
harness-collected values otherwise.

Args (positional):
    out              Target metadata.json path.
    experiment       Experiment id.
    host             Host tag.
    finished         ISO timestamp when run finished.
    started          ISO timestamp when run started.
    duration_ns      Integer duration.
    config           Config path string.
    cfg_sha          SHA256 of the config bytes.
    loadgen          JSON string or "null".
    mosquitto        JSON string or "null".
    rc               wafer-runtime exit code (int).
    provenance       JSON string of runtime-provenance.json, or "null" when
                     the sidecar is missing (harness-only fallback).
"""

import json
import pathlib
import subprocess
import sys


def _sh(cmd: list[str]) -> str:
    try:
        return subprocess.check_output(cmd, text=True).strip()
    except Exception:
        return "unknown"


def _read_text(path: str, default: str = "unknown") -> str:
    try:
        return pathlib.Path(path).read_text().replace("\x00", "").strip() or default
    except OSError:
        return default


def _source_metadata() -> tuple[str, bool, list[str]]:
    git_sha = _sh(["git", "rev-parse", "HEAD"])
    git_status = _sh(["git", "status", "--porcelain"])
    if git_sha != "unknown":
        tags = [
            tag
            for tag in _sh(["git", "tag", "--points-at", "HEAD"]).splitlines()
            if tag
        ]
        return git_sha, git_status != "", tags

    candidates = (
        pathlib.Path.cwd() / "SOURCE_STATE.json",
        pathlib.Path(__file__).resolve().parents[3] / "SOURCE_STATE.json",
    )
    for path in candidates:
        try:
            state = json.loads(path.read_text())
        except (OSError, ValueError):
            continue
        if isinstance(state.get("git_sha"), str) and isinstance(
            state.get("git_dirty"), bool
        ):
            tags = state.get("git_tags", [])
            if not isinstance(tags, list) or not all(
                isinstance(tag, str) for tag in tags
            ):
                tags = []
            return state["git_sha"], state["git_dirty"], tags

    return "unknown", True, []


def _hardware_metadata() -> dict:
    memory_total_kib: int | str = "unknown"
    try:
        for line in pathlib.Path("/proc/meminfo").read_text().splitlines():
            if line.startswith("MemTotal:"):
                memory_total_kib = int(line.split()[1])
                break
    except (OSError, ValueError, IndexError):
        pass

    governors = sorted({
        path.read_text().strip()
        for path in pathlib.Path("/sys/devices/system/cpu").glob(
            "cpu[0-9]*/cpufreq/scaling_governor"
        )
        if path.is_file()
    })

    temperature: int | str = "unknown"
    raw_temperature = _read_text("/sys/class/thermal/thermal_zone0/temp")
    if raw_temperature.isdigit():
        temperature = int(raw_temperature)

    throttled = _sh(["vcgencmd", "get_throttled"])
    if throttled.startswith("throttled="):
        throttled = throttled.removeprefix("throttled=")

    return {
        "hardware_model": _read_text("/proc/device-tree/model"),
        "memory_total_kib": memory_total_kib,
        "cpu_governors": governors or ["unknown"],
        "isolated_cpus": _read_text("/sys/devices/system/cpu/isolated"),
        "temperature_millicelsius": temperature,
        "throttled": throttled,
    }


def merge_metadata(
    out: str,
    experiment: str,
    host: str,
    finished: str,
    started: str,
    duration_ns: str,
    config: str,
    cfg_sha: str,
    loadgen: str,
    mosquitto: str,
    rc: str,
    provenance: str,
) -> None:
    git_sha, git_dirty, git_tags = _source_metadata()
    meta = {
        "experiment": experiment,
        "host_tag": host,
        "generated_at": finished,
        "started_at": started,
        "finished_at": finished,
        "duration_ns": int(duration_ns),
        "git_sha": git_sha,
        "git_dirty": git_dirty,
        "git_tags": git_tags,
        "hostname": _sh(["hostname"]),
        "kernel": _sh(["uname", "-r"]),
        "arch": _sh(["uname", "-m"]),
        "os": _sh(["uname", "-s"]).lower(),
        "rustc_version": _sh(["rustc", "--version"]),
        "config_path": config,
        "config_sha256": cfg_sha,
        "loadgen": json.loads(loadgen),
        "mosquitto": json.loads(mosquitto),
        "exit_codes": {"wafer_runtime": int(rc)},
        **_hardware_metadata(),
    }
    # Runtime provenance is authoritative when the sidecar is present: it
    # observed the actual bytes loaded and cannot drift from the process
    # image, unlike harness-side `rustc --version` which reports the host
    # toolchain (which may not match the toolchain that built the binary).
    if provenance != "null":
        p = json.loads(provenance)
        for key in (
            "wasmtime_version",
            "rustc_version",
            "wafer_runtime_version",
            "wafer_runtime_sha256",
            "wafer_plugin_hashes",
            "engine_fuel_budgets",
            "epoch_deadline",
            "epoch_tick_ms",
            "effective_metering_mode",
            "kernel",
            "config_sha256",
        ):
            if key in p:
                meta[key] = p[key]
    with open(out, "w") as fh:
        json.dump(meta, fh, indent=2)


def main() -> None:
    if len(sys.argv) != 13:
        sys.stderr.write(
            f"usage: {sys.argv[0]} <out> <experiment> <host> <finished> <started> "
            "<duration_ns> <config> <cfg_sha> <loadgen> <mosquitto> <rc> <provenance>\n"
        )
        sys.exit(2)
    merge_metadata(*sys.argv[1:])


if __name__ == "__main__":
    main()
