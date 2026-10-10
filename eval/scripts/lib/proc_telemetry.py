#!/usr/bin/env python3
"""Host-neutral /proc sampler that runs beside a canonical leaf.

Every second it appends cumulative counters, so a reader takes differences
between rows. It reads only /proc and /sys, so it works the same on a
Raspberry Pi, a Jetson or an x86 box, and it records its own CPU time so each
leaf carries the evidence of what the sampling cost.
"""

from __future__ import annotations

import argparse
import csv
import json
import os
import resource
import signal
import sys
import time
from pathlib import Path

PROC = Path("/proc")
SYS_CPU = Path("/sys/devices/system/cpu")
TRACKED_COMMS = frozenset(
    {"wafer", "wafer-runtime", "wafer-loadgen", "kuiperd", "mosquitto"}
)
CORE_FIELDS = [
    "timestamp_ns",
    "cpu",
    "user",
    "nice",
    "system",
    "idle",
    "iowait",
    "irq",
    "softirq",
    "steal",
    "frequency_hz",
]
SCHED_FIELDS = [
    "timestamp_ns",
    "ctxt",
    "processes",
    "procs_running",
    "procs_blocked",
    "mem_available_bytes",
    "psi_cpu_some_avg10",
    "psi_memory_some_avg10",
    "psi_memory_full_avg10",
    "psi_io_some_avg10",
    "psi_io_full_avg10",
    "sampler_cpu_ms",
]
PROCESS_FIELDS = [
    "timestamp_ns",
    "pid",
    "comm",
    "utime",
    "stime",
    "minflt",
    "majflt",
    "voluntary_ctxt_switches",
    "nonvoluntary_ctxt_switches",
    "threads",
    "rss_bytes",
    "cpus_allowed_list",
]


def parse_cpu_list(text: str) -> set[int]:
    cpus: set[int] = set()
    for part in text.split(","):
        part = part.strip()
        if not part:
            continue
        if "-" in part:
            low, high = part.split("-", 1)
            cpus.update(range(int(low), int(high) + 1))
        else:
            cpus.add(int(part))
    return cpus


def parse_proc_stat(text: str) -> tuple[list[dict[str, int | str]], dict[str, int]]:
    cores: list[dict[str, int | str]] = []
    totals: dict[str, int] = {}
    for line in text.splitlines():
        fields = line.split()
        if not fields:
            continue
        key = fields[0]
        if key.startswith("cpu") and key != "cpu":
            values = [int(value) for value in fields[1:9]]
            cores.append(
                {
                    "cpu": int(key[3:]),
                    **dict(zip(CORE_FIELDS[2:10], values, strict=True)),
                }
            )
        elif key in {"ctxt", "processes", "procs_running", "procs_blocked"}:
            totals[key] = int(fields[1])
    return cores, totals


def parse_pid_stat(text: str) -> dict[str, int | str]:
    """Fields after the parenthesised comm, which may itself contain spaces."""
    start = text.index("(") + 1
    end = text.rindex(")")
    comm = text[start:end]
    rest = text[end + 2 :].split()
    return {
        "comm": comm,
        "minflt": int(rest[7]),
        "majflt": int(rest[9]),
        "utime": int(rest[11]),
        "stime": int(rest[12]),
        "threads": int(rest[17]),
        "rss_pages": int(rest[21]),
    }


def parse_pid_status(text: str) -> dict[str, int | str]:
    values: dict[str, int | str] = {
        "voluntary_ctxt_switches": 0,
        "nonvoluntary_ctxt_switches": 0,
        "cpus_allowed_list": "",
    }
    for line in text.splitlines():
        key, _, value = line.partition(":")
        if key in {"voluntary_ctxt_switches", "nonvoluntary_ctxt_switches"}:
            values[key] = int(value.strip())
        elif key == "Cpus_allowed_list":
            values[key.lower()] = value.strip()
    return values


def parse_psi(text: str) -> dict[str, float]:
    values: dict[str, float] = {}
    for line in text.splitlines():
        fields = line.split()
        if not fields:
            continue
        for field in fields[1:]:
            if field.startswith("avg10="):
                values[fields[0]] = float(field.removeprefix("avg10="))
    return values


def read_text(path: Path) -> str | None:
    try:
        return path.read_text()
    except (OSError, UnicodeDecodeError):
        return None


def read_psi(resource_name: str) -> dict[str, float]:
    text = read_text(PROC / "pressure" / resource_name)
    return parse_psi(text) if text else {}


def read_mem_available_bytes() -> int | None:
    text = read_text(PROC / "meminfo")
    if text is None:
        return None
    for line in text.splitlines():
        if line.startswith("MemAvailable:"):
            return int(line.split()[1]) * 1024
    return None


def read_frequency_hz(cpu: int) -> int | None:
    text = read_text(SYS_CPU / f"cpu{cpu}" / "cpufreq" / "scaling_cur_freq")
    if text is None:
        return None
    try:
        return int(text.strip()) * 1000
    except ValueError:
        return None


class ProcessTracker:
    """Keeps the pid to comm map so most seconds only new pids cost a read.

    A pid seen between fork and exec still carries the wrapper's name (the
    runner launches the SUT through taskset), so untracked pids are re-read
    every RECHECK_EVERY samples instead of being written off for good.
    """

    RECHECK_EVERY = 5

    def __init__(self, comms: frozenset[str] = TRACKED_COMMS) -> None:
        self.comms = comms
        self.known: dict[int, str | None] = {}
        self.calls = 0

    def tracked_pids(self) -> list[int]:
        try:
            live = {int(name) for name in os.listdir(PROC) if name.isdigit()}
        except OSError:
            return []
        recheck = self.calls % self.RECHECK_EVERY == 0
        self.calls += 1
        for pid in list(self.known):
            if pid not in live:
                del self.known[pid]
        for pid in live:
            if pid in self.known and (self.known[pid] is not None or not recheck):
                continue
            text = read_text(PROC / str(pid) / "comm")
            comm = text.strip() if text is not None else None
            self.known[pid] = comm if comm in self.comms else None
        return sorted(pid for pid, comm in self.known.items() if comm is not None)


def thread_context_switches(pid: int) -> dict[str, int] | None:
    """/proc/<pid>/status counts only the thread-group leader, so sum every task."""
    try:
        tids = os.listdir(PROC / str(pid) / "task")
    except OSError:
        return None
    totals = {"voluntary_ctxt_switches": 0, "nonvoluntary_ctxt_switches": 0}
    for tid in tids:
        text = read_text(PROC / str(pid) / "task" / tid / "status")
        if text is None:
            continue
        status = parse_pid_status(text)
        for key in totals:
            totals[key] += int(status[key])
    return totals


def sample_processes(tracker: ProcessTracker, page_size: int) -> list[dict[str, int | str]]:
    rows = []
    for pid in tracker.tracked_pids():
        stat_text = read_text(PROC / str(pid) / "stat")
        status_text = read_text(PROC / str(pid) / "status")
        if stat_text is None or status_text is None:
            continue
        stat = parse_pid_stat(stat_text)
        status = parse_pid_status(status_text)
        switches = thread_context_switches(pid)
        if switches is not None:
            status.update(switches)
        rows.append(
            {
                "pid": pid,
                "comm": stat["comm"],
                "utime": stat["utime"],
                "stime": stat["stime"],
                "minflt": stat["minflt"],
                "majflt": stat["majflt"],
                "voluntary_ctxt_switches": status["voluntary_ctxt_switches"],
                "nonvoluntary_ctxt_switches": status["nonvoluntary_ctxt_switches"],
                "threads": stat["threads"],
                "rss_bytes": int(stat["rss_pages"]) * page_size,
                "cpus_allowed_list": status["cpus_allowed_list"],
            }
        )
    return rows


def sampler_cpu_seconds() -> float:
    usage = resource.getrusage(resource.RUSAGE_SELF)
    return usage.ru_utime + usage.ru_stime


def pin_to(cpus: str) -> str:
    wanted = parse_cpu_list(cpus)
    if wanted:
        os.sched_setaffinity(0, wanted)
    return ",".join(str(cpu) for cpu in sorted(os.sched_getaffinity(0)))


class Sampler:
    def __init__(self, output_dir: Path, interval_secs: float, pin_cpus: str, sut_cpus: str) -> None:
        self.output_dir = output_dir
        self.interval_secs = interval_secs
        self.sut_cpus = sut_cpus
        self.affinity = pin_to(pin_cpus)
        self.page_size = os.sysconf("SC_PAGE_SIZE")
        self.tracker = ProcessTracker()
        self.samples = 0
        self.cpu_before = sampler_cpu_seconds()
        self.started_ns = time.time_ns()

    def sample(self, core_writer: csv.DictWriter, sched_writer: csv.DictWriter, process_writer: csv.DictWriter) -> None:
        timestamp_ns = time.time_ns()
        stat_text = read_text(PROC / "stat") or ""
        cores, totals = parse_proc_stat(stat_text)
        for core in cores:
            core_writer.writerow(
                {"timestamp_ns": timestamp_ns, **core, "frequency_hz": read_frequency_hz(int(core["cpu"]))}
            )
        for row in sample_processes(self.tracker, self.page_size):
            process_writer.writerow({"timestamp_ns": timestamp_ns, **row})
        cpu_psi = read_psi("cpu")
        memory_psi = read_psi("memory")
        io_psi = read_psi("io")
        cpu_after = sampler_cpu_seconds()
        sched_writer.writerow(
            {
                "timestamp_ns": timestamp_ns,
                "ctxt": totals.get("ctxt"),
                "processes": totals.get("processes"),
                "procs_running": totals.get("procs_running"),
                "procs_blocked": totals.get("procs_blocked"),
                "mem_available_bytes": read_mem_available_bytes(),
                "psi_cpu_some_avg10": cpu_psi.get("some"),
                "psi_memory_some_avg10": memory_psi.get("some"),
                "psi_memory_full_avg10": memory_psi.get("full"),
                "psi_io_some_avg10": io_psi.get("some"),
                "psi_io_full_avg10": io_psi.get("full"),
                "sampler_cpu_ms": round((cpu_after - self.cpu_before) * 1000, 3),
            }
        )
        self.cpu_before = cpu_after
        self.samples += 1

    def receipt(self) -> dict[str, object]:
        usage = resource.getrusage(resource.RUSAGE_SELF)
        boot_id = read_text(PROC / "sys/kernel/random/boot_id")
        cmdline = read_text(PROC / "cmdline")
        uptime = read_text(PROC / "uptime")
        return {
            "schema_version": 1,
            "sampler": "proc_telemetry.py",
            "interval_secs": self.interval_secs,
            "started_unix_epoch_ns": self.started_ns,
            "finished_unix_epoch_ns": time.time_ns(),
            "samples": self.samples,
            "sampler_affinity": self.affinity,
            "sut_cpus": self.sut_cpus,
            "sampler_cpu_seconds": round(usage.ru_utime + usage.ru_stime, 6),
            "sampler_max_rss_bytes": usage.ru_maxrss * 1024,
            "clock_ticks_per_second": os.sysconf("SC_CLK_TCK"),
            "page_size_bytes": self.page_size,
            "cpu_count": os.cpu_count(),
            "boot_id": boot_id.strip() if boot_id else None,
            "kernel_cmdline": cmdline.strip() if cmdline else None,
            "uptime_secs_at_start": float(uptime.split()[0]) if uptime else None,
            "tracked_comms": sorted(TRACKED_COMMS),
        }


def write_error(output_dir: Path, error: BaseException) -> None:
    (output_dir / "host-sidecar-error.json").write_text(
        json.dumps({"error": f"{type(error).__name__}: {error}", "timestamp_ns": time.time_ns()}) + "\n"
    )


def run(output_dir: Path, interval_secs: float, pin_cpus: str, sut_cpus: str) -> int:
    output_dir.mkdir(parents=True, exist_ok=True)
    try:
        return sample_until_stopped(output_dir, interval_secs, pin_cpus, sut_cpus)
    except Exception as error:
        write_error(output_dir, error)
        return 1


def sample_until_stopped(output_dir: Path, interval_secs: float, pin_cpus: str, sut_cpus: str) -> int:
    sampler = Sampler(output_dir, interval_secs, pin_cpus, sut_cpus)
    stop = False

    def request_stop(_signum: int, _frame: object) -> None:
        nonlocal stop
        stop = True

    signal.signal(signal.SIGTERM, request_stop)
    signal.signal(signal.SIGINT, request_stop)
    with (
        (output_dir / "cpu-cores.csv").open("w", newline="") as core_file,
        (output_dir / "host-sched.csv").open("w", newline="") as sched_file,
        (output_dir / "sut-processes.csv").open("w", newline="") as process_file,
    ):
        core_writer = csv.DictWriter(core_file, fieldnames=CORE_FIELDS)
        sched_writer = csv.DictWriter(sched_file, fieldnames=SCHED_FIELDS)
        process_writer = csv.DictWriter(process_file, fieldnames=PROCESS_FIELDS)
        for writer in (core_writer, sched_writer, process_writer):
            writer.writeheader()
        while not stop:
            started = time.monotonic()
            sampler.sample(core_writer, sched_writer, process_writer)
            for stream in (core_file, sched_file, process_file):
                stream.flush()
            time.sleep(max(0.0, interval_secs - (time.monotonic() - started)))
        sampler.sample(core_writer, sched_writer, process_writer)
    (output_dir / "host-sidecar.json").write_text(json.dumps(sampler.receipt(), indent=2) + "\n")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("output_dir", type=Path)
    parser.add_argument("--interval-secs", type=float, default=1.0)
    parser.add_argument("--pin-cpus", default="", help="CPU list the sampler pins itself to")
    parser.add_argument("--sut-cpus", default="", help="CPU list recorded as the system under test")
    args = parser.parse_args(argv)
    return run(args.output_dir, args.interval_secs, args.pin_cpus, args.sut_cpus)


if __name__ == "__main__":
    sys.exit(main())
