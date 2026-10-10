#!/usr/bin/env python3

import csv
import json
import os
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "eval/scripts/lib"))

from proc_telemetry import (  # noqa: E402
    CORE_FIELDS,
    PROCESS_FIELDS,
    SCHED_FIELDS,
    parse_cpu_list,
    parse_pid_stat,
    parse_pid_status,
    parse_proc_stat,
    parse_psi,
)

PROC_STAT = """cpu  100 1 200 3000 40 0 5 6 0 0
cpu0 10 1 20 300 4 0 1 2 0 0
cpu1 90 0 180 2700 36 0 4 0 0 0
intr 1234 0 0
ctxt 450961
btime 1790000000
processes 1792
procs_running 2
procs_blocked 1
"""

PID_STAT = (
    "4242 (wafer load gen) S 1 4242 4242 0 -1 4194560 2424 0 3 0 11 155 0 0 20 0 8 0 38 "
    "24231936 1072 18446744073709551615 1 1 0 0 0 0 0 4096 1088 0 0 0 17 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n"
)

PID_STATUS = """Name:\twafer
Cpus_allowed_list:\t1-3
voluntary_ctxt_switches:\t17
nonvoluntary_ctxt_switches:\t3
"""


def test_proc_stat_parser_keeps_per_core_jiffies_and_scheduler_totals() -> None:
    cores, totals = parse_proc_stat(PROC_STAT)
    assert [core["cpu"] for core in cores] == [0, 1]
    assert cores[1]["user"] == 90 and cores[1]["steal"] == 0 and cores[1]["softirq"] == 4
    assert totals == {"ctxt": 450961, "processes": 1792, "procs_running": 2, "procs_blocked": 1}


def test_pid_stat_parser_survives_spaces_in_comm() -> None:
    stat = parse_pid_stat(PID_STAT)
    assert stat["comm"] == "wafer load gen"
    assert stat["minflt"] == 2424 and stat["majflt"] == 3
    assert stat["utime"] == 11 and stat["stime"] == 155
    assert stat["threads"] == 8 and stat["rss_pages"] == 1072


def test_pid_status_parser_reads_context_switches_and_affinity() -> None:
    status = parse_pid_status(PID_STATUS)
    assert status == {
        "voluntary_ctxt_switches": 17,
        "nonvoluntary_ctxt_switches": 3,
        "cpus_allowed_list": "1-3",
    }


def test_psi_parser_reads_avg10_per_line() -> None:
    text = "some avg10=1.50 avg60=0.10 avg300=0.00 total=1482092\nfull avg10=0.25 avg60=0.00 avg300=0.00 total=0\n"
    assert parse_psi(text) == {"some": 1.5, "full": 0.25}


def test_cpu_list_parser_accepts_ranges_and_blanks() -> None:
    assert parse_cpu_list("0,2-3") == {0, 2, 3}
    assert parse_cpu_list("") == set()


def test_sampler_writes_contract_files_and_tracks_sut_processes_by_comm() -> None:
    with tempfile.TemporaryDirectory() as workdir:
        output = Path(workdir) / "leaf"
        fake_sut = Path(workdir) / "wafer"
        fake_sut.symlink_to(sys.executable)
        sampler = subprocess.Popen(
            [
                sys.executable,
                str(ROOT / "eval/scripts/lib/proc_telemetry.py"),
                str(output),
                "--interval-secs",
                "0.1",
                "--sut-cpus",
                "0",
            ]
        )
        sut = subprocess.Popen([str(fake_sut), "-c", "import time; time.sleep(1.2)"])
        try:
            time.sleep(1.0)
        finally:
            sampler.send_signal(signal.SIGTERM)
            sampler.wait(timeout=10)
            sut.wait(timeout=10)
        assert sampler.returncode == 0

        with (output / "cpu-cores.csv").open() as stream:
            cores = list(csv.DictReader(stream))
        with (output / "host-sched.csv").open() as stream:
            sched = list(csv.DictReader(stream))
        with (output / "sut-processes.csv").open() as stream:
            processes = list(csv.DictReader(stream))
        receipt = json.loads((output / "host-sidecar.json").read_text())

        assert list(cores[0].keys()) == CORE_FIELDS
        assert list(sched[0].keys()) == SCHED_FIELDS
        assert list(processes[0].keys()) == PROCESS_FIELDS
        assert {row["cpu"] for row in cores} == {str(cpu) for cpu in range(os.cpu_count() or 1)}
        assert len(sched) >= 5
        assert int(sched[-1]["ctxt"]) >= int(sched[0]["ctxt"])
        assert all(float(row["sampler_cpu_ms"]) >= 0 for row in sched)

        sut_rows = [row for row in processes if row["pid"] == str(sut.pid)]
        assert sut_rows and {row["comm"] for row in sut_rows} == {"wafer"}
        assert int(sut_rows[-1]["utime"]) + int(sut_rows[-1]["stime"]) >= 0
        assert all(row["cpus_allowed_list"] for row in processes)
        assert not (output / "host-sidecar-error.json").exists()

        assert receipt["samples"] == len(sched)
        assert receipt["sut_cpus"] == "0"
        assert receipt["sampler_cpu_seconds"] < 2.0
        assert receipt["clock_ticks_per_second"] == os.sysconf("SC_CLK_TCK")
        assert receipt["boot_id"]


def test_untracked_pid_is_rechecked_after_exec(tmp_path: Path, monkeypatch) -> None:
    import proc_telemetry

    fake_proc = tmp_path / "proc"
    (fake_proc / "77").mkdir(parents=True)
    (fake_proc / "77" / "comm").write_text("taskset\n")
    monkeypatch.setattr(proc_telemetry, "PROC", fake_proc)
    tracker = proc_telemetry.ProcessTracker()
    assert tracker.tracked_pids() == []
    (fake_proc / "77" / "comm").write_text("wafer\n")
    seen = [tracker.tracked_pids() for _ in range(tracker.RECHECK_EVERY)]
    assert seen[-1] == [77]


def test_sampler_records_a_failure_instead_of_dying_silently(tmp_path: Path) -> None:
    import proc_telemetry

    assert proc_telemetry.run(tmp_path / "leaf", 0.1, "not-a-cpu-list", "") == 1
    error = json.loads((tmp_path / "leaf" / "host-sidecar-error.json").read_text())
    assert error["error"].startswith("ValueError")


def test_only_launcher_pids_are_rechecked(tmp_path: Path, monkeypatch) -> None:
    import proc_telemetry

    fake_proc = tmp_path / "proc"
    for pid, comm in {10: "kworker/0:1", 11: "rsyslogd", 12: "python3", 13: "bash", 14: "(kuiperd)"}.items():
        (fake_proc / str(pid)).mkdir(parents=True)
        (fake_proc / str(pid) / "comm").write_text(f"{comm}\n")
    monkeypatch.setattr(proc_telemetry, "PROC", fake_proc)
    reads: list[int] = []
    real_read_text = proc_telemetry.read_text

    def counting_read_text(path: Path) -> str | None:
        if path.name == "comm":
            reads.append(int(path.parent.name))
        return real_read_text(path)

    monkeypatch.setattr(proc_telemetry, "read_text", counting_read_text)
    tracker = proc_telemetry.ProcessTracker()
    assert tracker.tracked_pids() == []
    assert sorted(reads) == [10, 11, 12, 13, 14]
    for pid in (10, 11, 12, 13, 14):
        (fake_proc / str(pid) / "comm").write_text("wafer\n")
    reads.clear()
    seen = [tracker.tracked_pids() for _ in range(tracker.RECHECK_EVERY)]
    assert sorted(reads) == [12, 13, 14]
    assert seen[-1] == [12, 13, 14]
