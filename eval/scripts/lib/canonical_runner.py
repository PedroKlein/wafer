from __future__ import annotations

import argparse
import csv
import datetime as dt
import hashlib
import json
import os
import random
import re
import shutil
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path

from write_metadata import merge_metadata


@dataclass(frozen=True)
class Condition:
    name: str
    config: str
    loadgen_profile: str | None = None
    total_messages: int | None = None
    shared_from: str | None = None
    startup_mode: str | None = None
    system: str = "wafer"
    events_per_run: int | None = None


@dataclass(frozen=True)
class RunItem:
    experiment: str
    condition: str
    run_index: int
    config: str
    warmup_secs: int
    measurement_secs: int
    runtime_cpus: str = "1-3"
    support_cpus: str = "0"
    loadgen_profile: str | None = None
    total_messages: int | None = None
    shared_from: str | None = None
    startup_mode: str | None = None
    system: str = "wafer"
    events_per_run: int | None = None

    @property
    def result_key(self) -> str:
        return f"{self.experiment}/{self.condition}/run-{self.run_index:02d}"


@dataclass(frozen=True)
class AttemptSelection:
    path: Path
    skip: bool


@dataclass(frozen=True)
class ValidationGate:
    passed: bool
    failed_runs: list[int]
    observed_runs: int


CONDITIONS: dict[str, tuple[Condition, ...]] = {
    "e-perf-1": tuple(
        Condition(
            system,
            "eval/configs/pipeline-a-wafer.toml"
            if system == "wafer"
            else "eval/configs/pipeline-a-native.toml"
            if system == "native"
            else "eval/configs/canonical/e-perf-1-ekuiper.toml",
            "eval/loadgen/telemetry-120b.toml",
            60_000,
            system=system,
        )
        for system in ("wafer", "native", "ekuiper")
    ),
    "e-perf-2": tuple(
        Condition(
            system,
            "eval/configs/pipeline-a-wafer.toml"
            if system == "wafer"
            else "eval/configs/pipeline-a-native.toml"
            if system == "native"
            else "eval/configs/canonical/e-perf-1-ekuiper.toml",
            "eval/loadgen/telemetry-120b.toml",
            60_000,
            shared_from="e-perf-1",
            system=system,
        )
        for system in ("wafer", "native", "ekuiper")
    ),
    "e-val-1": (
        Condition("delay-50ms", "eval/configs/canonical/e-val-1.toml"),
    ),
    "e-perf-3": tuple(
        Condition(
            f"depth-{depth}",
            f"eval/configs/e-perf-3/pipeline-mqtt-depth-{depth}.toml",
            "eval/loadgen/telemetry-120b.toml",
            60_000,
        )
        for depth in (1, 3, 5, 10)
    ),
    "e-perf-4": tuple(
        Condition(
            size,
            f"eval/configs/canonical/e-perf-4-{size}.toml",
        )
        for size in ("120b", "1kb", "10kb", "100kb")
    ),
    "e-perf-6": tuple(
        Condition(
            f"depth-{depth}",
            f"eval/configs/pipeline-depth-{depth}.toml",
        )
        for depth in (1, 3, 5, 10)
    ),
    "e-perf-8": tuple(
        Condition(
            f"depth-{depth}",
            f"eval/configs/pipeline-depth-{depth}.toml",
            shared_from="e-perf-6",
        )
        for depth in (1, 3, 5, 10)
    ),
    "e-perf-7": tuple(
        Condition(
            mode,
            f"eval/configs/pipeline-c-{mode}.toml",
        )
        for mode in ("neither", "fuel-only", "epoch-only", "passthrough")
    ),
    "e-perf-9": tuple(
        Condition(
            f"{tier}-{cache}",
            f"eval/configs/e-perf-9/pipeline-tier-{tier}.toml",
            startup_mode=cache,
        )
        for tier in ("small", "medium", "large")
        for cache in ("cold", "warm")
    ),
    "e-perf-5": (
        Condition("wafer", "eval/configs/pipeline-c-passthrough.toml", system="wafer"),
        Condition("native", "eval/configs/pipeline-d-native.toml", system="native"),
    ),
    "e-swap-3": tuple(
        Condition(
            strategy,
            "eval/configs/e-swap/pipeline-swap3-mqtt.toml"
            if strategy != "ekuiper-restart"
            else "eval/configs/canonical/e-perf-1-ekuiper.toml",
            "eval/loadgen/canonical-hotswap.toml"
            if strategy == "wafer-hotswap"
            else "eval/loadgen/telemetry-120b.toml",
            120_000,
            system="ekuiper" if strategy == "ekuiper-restart" else "wafer",
        )
        for strategy in ("wafer-hotswap", "wafer-restart", "ekuiper-restart")
    ),
    "e-iso-1": (Condition("buffer-overflow", "eval/configs/e-iso-1/pipeline.toml"),),
    "e-iso-2": (Condition("cross-read", "eval/configs/e-iso-2/pipeline.toml"),),
    "e-iso-3": (Condition("fs-access", "eval/configs/e-iso-3/pipeline.toml"),),
    "e-iso-4": (Condition("infinite-loop", "eval/configs/e-iso-4/pipeline.toml"),),
    "e-iso-5": (Condition("memory-exhaust", "eval/configs/e-iso-5/pipeline.toml"),),
    "e-iso-6": (Condition("panic", "eval/configs/e-iso-6/pipeline.toml"),),
    "e-iso-7": (
        Condition("control", "eval/configs/e-iso-7/pipeline-control.toml"),
        Condition("attack", "eval/configs/e-iso-7/pipeline.toml"),
    ),
    "e-iso-8": (Condition("panic-recovery", "eval/configs/e-iso-8/pipeline.toml"),),
    "e-swap-1": (
        Condition("steady", "eval/configs/e-swap/pipeline-hotswap.toml", events_per_run=50),
    ),
    "e-swap-2": (
        Condition(
            "steady",
            "eval/configs/e-swap/pipeline-hotswap.toml",
            shared_from="e-swap-1",
            events_per_run=50,
        ),
    ),
    "e-swap-4": (
        Condition("burst-2x", "eval/configs/e-swap/pipeline-hotswap-burst.toml", events_per_run=50),
    ),
    "e-swap-5": (
        Condition(
            "process-trap-rollback",
            "eval/configs/e-swap/pipeline-hotswap-rollback.toml",
            events_per_run=50,
        ),
    ),
    "e-swap-6": (
        Condition(
            "steady",
            "eval/configs/e-swap/pipeline-hotswap.toml",
            shared_from="e-swap-1",
            events_per_run=50,
        ),
    ),
    "e-backpressure": (
        Condition(
            "burst-2x",
            "eval/configs/e-backpressure/pipeline-burst.toml",
            "eval/loadgen/canonical-burst.toml",
            350_000,
        ),
    ),
}


EXPERIMENT_ORDER = (
    "e-val-1",
    "e-perf-1",
    "e-perf-2",
    "e-perf-3",
    "e-perf-4",
    "e-perf-5",
    "e-perf-6",
    "e-perf-8",
    "e-perf-7",
    "e-perf-9",
    "e-backpressure",
    "e-iso-1",
    "e-iso-2",
    "e-iso-3",
    "e-iso-4",
    "e-iso-5",
    "e-iso-6",
    "e-iso-7",
    "e-iso-8",
    "e-swap-1",
    "e-swap-2",
    "e-swap-3",
    "e-swap-4",
    "e-swap-5",
    "e-swap-6",
)


def build_schedule(experiments: set[str], seed: int) -> list[RunItem]:
    unknown = experiments - CONDITIONS.keys()
    if unknown:
        raise ValueError(f"unsupported experiments: {', '.join(sorted(unknown))}")

    matrix_path = Path(__file__).resolve().parents[2] / "canonical-matrix.json"
    matrix = json.loads(matrix_path.read_text())["experiments"]
    schedule: list[RunItem] = []

    for experiment in (item for item in EXPERIMENT_ORDER if item in experiments):
        definition = matrix[experiment]
        conditions = CONDITIONS[experiment]
        for run_index in range(1, definition["repetitions"] + 1):
            if experiment == "e-perf-9":
                ordered = list(conditions)
            else:
                ordered = list(conditions)
                random.Random(f"{seed}:{experiment}:{run_index}").shuffle(ordered)
            for condition in ordered:
                schedule.append(
                    RunItem(
                        experiment=experiment,
                        condition=condition.name,
                        run_index=run_index,
                        config=condition.config,
                        warmup_secs=definition["warmup_secs"],
                        measurement_secs=definition["measurement_secs"],
                        loadgen_profile=condition.loadgen_profile,
                        total_messages=condition.total_messages,
                        shared_from=condition.shared_from,
                        startup_mode=condition.startup_mode,
                        system=condition.system,
                        events_per_run=condition.events_per_run or definition.get("events_per_run"),
                    )
                )
    return schedule


def select_attempt(condition_dir: Path, run_index: int) -> AttemptSelection:
    attempts = sorted(condition_dir.glob(f"run-{run_index:02d}-attempt-*"))
    for attempt in attempts:
        status_path = attempt / "canonical-status.json"
        try:
            status = json.loads(status_path.read_text())
        except (OSError, ValueError):
            continue
        if status.get("status") == "passed":
            return AttemptSelection(attempt, True)

    next_index = len(attempts) + 1
    path = condition_dir / f"run-{run_index:02d}-attempt-{next_index:02d}"
    return AttemptSelection(path, False)


def evaluate_validation_gate(root: Path, expected_runs: int) -> ValidationGate:
    failed: list[int] = []
    observed = 0
    for run_index in range(1, expected_runs + 1):
        attempts = sorted(root.glob(f"run-{run_index:02d}-attempt-*"))
        passed_attempt = None
        for attempt in attempts:
            try:
                status = json.loads((attempt / "canonical-status.json").read_text())
            except (OSError, ValueError):
                continue
            if status.get("status") == "passed":
                passed_attempt = attempt
                break
        if passed_attempt is None:
            failed.append(run_index)
            continue
        observed += 1
        try:
            summary = json.loads((passed_attempt / "percentiles.json").read_text())
            p99_ms = int(summary["p99_ns"]) / 1_000_000
            count = int(summary["total_count"])
        except (OSError, ValueError, KeyError, TypeError):
            failed.append(run_index)
            continue
        if count <= 0 or not 45 <= p99_ms <= 55:
            failed.append(run_index)

    return ValidationGate(not failed and observed == expected_runs, failed, observed)


def derive_containment(output: Path) -> dict:
    with (output / "per_node_metrics.csv").open(newline="") as stream:
        rows = [
            row
            for row in csv.DictReader(stream)
            if row.get("node_id") and not row["node_id"].startswith("#")
        ]
    if not rows:
        raise ValueError("per_node_metrics.csv contains no runtime metric rows")
    try:
        traps_total = sum(int(row["traps_total"]) for row in rows)
        healthy_messages = max(
            (int(row["messages_out"]) for row in rows if row["node_id"] not in {"attack", "branch_b"}),
            default=0,
        )
    except (KeyError, TypeError, ValueError) as error:
        raise ValueError("per_node_metrics.csv contains invalid runtime metrics") from error
    log = (output / "stdout.log").read_text(errors="replace")
    runtime_panic = bool(re.search(r"thread .* panicked|panicked at", log))
    return {
        "contained": traps_total > 0 and not runtime_panic,
        "traps_total": traps_total,
        "healthy_messages_out": healthy_messages,
        "runtime_panic": runtime_panic,
        "nodes": rows,
    }


def summarize_recovery(path: Path) -> dict:
    with path.open(newline="") as stream:
        samples = [int(row["duration_ns"]) for row in csv.DictReader(stream)]
    if not samples:
        raise ValueError("recovery.csv contains no samples")
    ordered = sorted(samples)

    def percentile(fraction: float) -> int:
        index = max(0, min(len(ordered) - 1, int(len(ordered) * fraction + 0.999999) - 1))
        return ordered[index]

    return {
        "sample_count": len(ordered),
        "min_ns": ordered[0],
        "p50_ns": percentile(0.50),
        "p95_ns": percentile(0.95),
        "p99_ns": percentile(0.99),
        "max_ns": ordered[-1],
    }


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def write_progress(
    ledger: Path,
    event: str,
    completed: int,
    total: int,
    item: str = "",
    failures: int = 0,
) -> None:
    try:
        temperature_c = int(
            Path("/sys/class/thermal/thermal_zone0/temp").read_text().strip()
        ) / 1000
    except (OSError, ValueError):
        temperature_c = None
    entry = {
        "timestamp": utc_now(),
        "event": event,
        "completed": completed,
        "total": total,
        "item": item,
        "failures": failures,
        "temperature_c": temperature_c,
    }
    with (ledger / "progress.jsonl").open("a") as stream:
        stream.write(json.dumps(entry, separators=(",", ":")) + "\n")
    print(
        f"[{entry['timestamp']}] PROGRESS {completed}/{total} event={event} "
        f"item={item or '-'} failures={failures} temp_c={temperature_c}",
        flush=True,
    )


def write_status(path: Path, item: RunItem, status: str, detail: str = "") -> None:
    path.mkdir(parents=True, exist_ok=True)
    receipt = {
        "status": status,
        "experiment": item.experiment,
        "condition": item.condition,
        "run_index": item.run_index,
        "updated_at": utc_now(),
    }
    if detail:
        receipt["detail"] = detail
    (path / "canonical-status.json").write_text(json.dumps(receipt, indent=2) + "\n")


def find_passed_attempt(condition_dir: Path, run_index: int) -> Path | None:
    selection = select_attempt(condition_dir, run_index)
    return selection.path if selection.skip else None


def copy_shared_result(root: Path, batch_id: str, item: RunItem) -> Path:
    if item.shared_from is None:
        raise ValueError("shared result has no source experiment")
    source_dir = (
        root
        / "eval/results"
        / item.shared_from
        / f"rpi5-{batch_id}"
        / item.condition
    )
    source = find_passed_attempt(source_dir, item.run_index)
    if source is None:
        raise RuntimeError(f"shared source is incomplete: {source_dir}")
    target_dir = (
        root
        / "eval/results"
        / item.experiment
        / f"rpi5-{batch_id}"
        / item.condition
    )
    selection = select_attempt(target_dir, item.run_index)
    if selection.skip:
        return selection.path
    shutil.copytree(source, selection.path)
    status_path = selection.path / "canonical-status.json"
    status_path.unlink(missing_ok=True)
    metadata_path = selection.path / "metadata.json"
    metadata = json.loads(metadata_path.read_text())
    metadata["experiment"] = item.experiment
    metadata["shared_from"] = str(source.relative_to(root))
    metadata_path.write_text(json.dumps(metadata, indent=2) + "\n")
    return selection.path


def start_pi_telemetry(root: Path, output: Path) -> subprocess.Popen:
    return subprocess.Popen(
        [sys.executable, str(root / "eval/scripts/lib/pi_telemetry.py"), str(output)],
        cwd=root,
    )


def stop_pi_telemetry(process: subprocess.Popen | None) -> None:
    if process is None or process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def wait_for_api(url: str, timeout_secs: float = 10.0) -> None:
    deadline = time.monotonic() + timeout_secs
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=1):
                return
        except (OSError, urllib.error.URLError):
            time.sleep(0.1)
    raise RuntimeError(f"API did not become ready: {url}")


def post_hot_swap(node_id: str, plugin: Path) -> dict:
    request = urllib.request.Request(
        f"http://127.0.0.1:9090/api/v1/nodes/{node_id}/hot-swap",
        data=json.dumps({"wasm_path": str(plugin)}).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            body = response.read().decode()
            return {"http_status": response.status, "body": json.loads(body)}
    except urllib.error.HTTPError as error:
        body = error.read().decode(errors="replace")
        return {"http_status": error.code, "body": body}


def run_hot_swap_item(root: Path, item: RunItem, selection: AttemptSelection) -> bool:
    output = selection.path
    output.mkdir(parents=True)
    config = root / item.config
    shutil.copy2(config, output / "config.toml")
    print(f"[{utc_now()}] START {item.result_key} -> {output}", flush=True)
    started_ns = time.time_ns()
    started_at = utc_now()
    runtime: subprocess.Popen | None = None
    telemetry = start_pi_telemetry(root, output)
    try:
        set_ekuiper_active(root, False)
        subprocess.run(
            [
                sys.executable,
                str(root / "eval/scripts/validate-canonical.py"),
                "host",
                "--root",
                str(root),
            ],
            cwd=root,
            check=True,
        )
        environment = os.environ.copy()
        environment["WAFER_BENCH_OUTPUT_DIR"] = str(output)
        with (output / "stdout.log").open("ab") as log:
            runtime = subprocess.Popen(
                [
                    "taskset", "-c", item.runtime_cpus,
                    str(root / "target/release/wafer"), "--config", str(config),
                ],
                cwd=root,
                env=environment,
                stdout=log,
                stderr=log,
            )
            wait_for_api("http://127.0.0.1:9090/health")
            time.sleep(item.warmup_secs)
            v1 = root / "plugins/pass-through-v1/target/wasm32-wasip2/release/wafer_pass_through_v1.wasm"
            v2 = root / "plugins/pass-through-v2/target/wasm32-wasip2/release/wafer_pass_through_v2.wasm"
            panics = root / "plugins/pass-through-v2-panics/target/wasm32-wasip2/release/wafer_pass_through_v2_panics.wasm"
            event_count = item.events_per_run or 50
            interval = item.measurement_secs / event_count
            requests: list[dict] = []
            for event_index in range(event_count):
                event_started = time.monotonic()
                plugin = panics if item.experiment == "e-swap-5" else (v2 if event_index % 2 == 0 else v1)
                request_started_ns = time.time_ns()
                response = post_hot_swap("transform", plugin)
                request_finished_ns = time.time_ns()
                requests.append(
                    {
                        "event_index": event_index,
                        "plugin": plugin.name,
                        "request_started_ns": request_started_ns,
                        "request_finished_ns": request_finished_ns,
                        "request_duration_ns": request_finished_ns - request_started_ns,
                        **response,
                    }
                )
                remaining = interval - (time.monotonic() - event_started)
                if remaining > 0:
                    time.sleep(remaining)
            (output / "swap_requests.json").write_text(json.dumps(requests, indent=2) + "\n")
            try:
                runtime_exit = runtime.wait(timeout=30)
            except subprocess.TimeoutExpired:
                runtime.terminate()
                runtime_exit = runtime.wait(timeout=10)
            runtime = None
        if runtime_exit != 0:
            raise RuntimeError(f"wafer runtime exited with {runtime_exit}")
        if not (output / "swap_timeline.json").is_file():
            (output / "swap_timeline.json").write_text(
                json.dumps({"requests": requests}, indent=2) + "\n"
            )
        if item.experiment == "e-swap-5":
            rolled_back = sum(
                1
                for request in requests
                if request["http_status"] == 200
                and isinstance(request["body"], dict)
                and request["body"].get("status") == "rolled_back"
            )
            rollback = {
                "attempts": len(requests),
                "rolled_back": rolled_back,
                "all_rolled_back": rolled_back == len(requests),
            }
            (output / "rollback.json").write_text(json.dumps(rollback, indent=2) + "\n")
            if not rollback["all_rolled_back"]:
                raise RuntimeError(
                    f"only {rolled_back}/{len(requests)} failed swaps rolled back"
                )
        else:
            successful = sum(request["http_status"] == 200 for request in requests)
            if successful != len(requests):
                raise RuntimeError(f"only {successful}/{len(requests)} hot swaps succeeded")

        with (output / "sequence.csv").open(newline="") as stream:
            sequence = next(csv.DictReader(stream))
        if int(sequence["gap_msgs"]) != 0 or int(sequence["duplicates_count"]) != 0:
            raise RuntimeError(
                "hot-swap sequence integrity failed: "
                f"gaps={sequence['gap_msgs']}, duplicates={sequence['duplicates_count']}"
            )
        stop_pi_telemetry(telemetry)
        finished_ns = time.time_ns()
        provenance_path = output / "runtime-provenance.json"
        provenance = provenance_path.read_text() if provenance_path.is_file() else "null"
        merge_metadata(
            str(output / "metadata.json"),
            item.experiment,
            "rpi5",
            utc_now(),
            started_at,
            str(finished_ns - started_ns),
            item.config,
            hashlib.sha256(config.read_bytes()).hexdigest(),
            "null",
            json.dumps({"broker": "127.0.0.1:1883", "managed_by_harness": False}),
            str(runtime_exit),
            provenance,
        )
        postprocess_run(root, item, output)
        verify_result(root, output)
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        if runtime is not None and runtime.poll() is None:
            runtime.terminate()
        stop_pi_telemetry(telemetry)
        write_status(output, item, "failed", str(error))
        print(f"[{utc_now()}] FAIL {item.result_key}: {error}", flush=True)
        return False
    write_status(output, item, "passed")
    print(f"[{utc_now()}] PASS {item.result_key}", flush=True)
    return True


def wait_for_subscriber(process: subprocess.Popen, timeout: int = 30) -> int:
    try:
        return process.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        process.send_signal(signal.SIGINT)
        return process.wait(timeout=10)


def loadgen_command(
    root: Path,
    item: RunItem,
    action: str,
    output: Path | None = None,
    duration: int | None = None,
) -> list[str]:
    loadgen = root / "target/release/wafer-loadgen"
    if action == "subscribe":
        if output is None:
            raise ValueError("subscriber requires an output directory")
        return [
            "taskset", "-c", item.support_cpus,
            str(loadgen), "subscribe", "--broker", "127.0.0.1:1883",
            "--topic", "wafer/telemetry/hot", "--output-dir", str(output),
            "--total-messages", str(item.total_messages or 0),
            "--host-tag", "rpi5",
        ]
    return [
        "taskset", "-c", item.support_cpus,
        str(loadgen), "publish",
        "--broker-host", "127.0.0.1", "--broker-port", "1883",
        "--topic", "wafer/telemetry", "--profile-file", str(root / str(item.loadgen_profile)),
        "--duration-secs", str(duration if duration is not None else item.measurement_secs),
    ]


def run_restart_item(
    root: Path,
    item: RunItem,
    selection: AttemptSelection,
) -> bool:
    output = selection.path
    output.mkdir(parents=True)
    config = root / item.config
    shutil.copy2(config, output / "config.toml")
    print(f"[{utc_now()}] START {item.result_key} -> {output}", flush=True)
    started_ns = time.time_ns()
    started_at = utc_now()
    runtime: subprocess.Popen | None = None
    runtime_exit = 0
    ekuiper_audit: Path | None = None
    telemetry = start_pi_telemetry(root, output)
    try:
        is_ekuiper = item.system == "ekuiper"
        set_ekuiper_active(root, is_ekuiper)
        facts_path = output / "host-facts.json"
        host_command = [
            sys.executable,
            str(root / "eval/scripts/validate-canonical.py"),
            "host",
            "--root",
            str(root),
            "--output",
            str(facts_path),
        ]
        if is_ekuiper:
            host_command.append("--require-ekuiper")
        subprocess.run(host_command, cwd=root, check=True)
        facts = json.loads(facts_path.read_text())
        if is_ekuiper:
            ekuiper_audit = capture_ekuiper_audit(root, output, item.runtime_cpus)
        environment = os.environ.copy()
        environment["WAFER_GIT_SHA"] = facts["git_sha"]
        environment["WAFER_BENCH_OUTPUT_DIR"] = str(output)

        with (output / "stdout.log").open("ab") as log:
            runtime_command = [
                "taskset", "-c", item.runtime_cpus,
                str(root / "target/release/wafer"), "--config", str(config),
            ]
            if not is_ekuiper:
                runtime = subprocess.Popen(
                    runtime_command,
                    cwd=root,
                    env=environment,
                    stdout=log,
                    stderr=log,
                )
                time.sleep(1)
                if runtime.poll() is not None:
                    raise RuntimeError("wafer runtime exited during startup")

            subprocess.run(
                loadgen_command(root, item, "publish", duration=item.warmup_secs),
                cwd=root,
                env=environment,
                stdout=log,
                stderr=log,
                check=True,
            )
            measurement_started_ns = time.time_ns()
            subscriber = subprocess.Popen(
                loadgen_command(root, item, "subscribe", output=output),
                cwd=root,
                env=environment,
                stdout=log,
                stderr=log,
            )
            time.sleep(0.5)
            publisher = subprocess.Popen(
                loadgen_command(root, item, "publish"),
                cwd=root,
                env=environment,
                stdout=log,
                stderr=log,
            )
            time.sleep(item.measurement_secs / 2)
            action_started_ns = time.time_ns()
            if is_ekuiper:
                subprocess.run(
                    ["curl", "-fsS", "-X", "POST", "http://127.0.0.1:9081/rules/pipeline_a/stop"],
                    stdout=log,
                    stderr=log,
                    check=True,
                )
                subprocess.run(
                    ["curl", "-fsS", "-X", "POST", "http://127.0.0.1:9081/rules/pipeline_a/start"],
                    stdout=log,
                    stderr=log,
                    check=True,
                )
            else:
                assert runtime is not None
                runtime.terminate()
                runtime.wait(timeout=10)
                runtime = subprocess.Popen(
                    runtime_command,
                    cwd=root,
                    env=environment,
                    stdout=log,
                    stderr=log,
                )
                time.sleep(1)
                if runtime.poll() is not None:
                    raise RuntimeError("wafer runtime failed to restart")
            action_finished_ns = time.time_ns()
            publisher_code = publisher.wait(timeout=item.measurement_secs + 30)
            subscriber_code = wait_for_subscriber(subscriber)
            if publisher_code != 0 or subscriber_code != 0:
                raise RuntimeError(
                    f"loadgen failed: publisher={publisher_code}, subscriber={subscriber_code}"
                )
            measurement_finished_ns = time.time_ns()

        (output / "measurement-window.json").write_text(
            json.dumps(
                {
                    "started_ns": measurement_started_ns,
                    "finished_ns": measurement_finished_ns,
                },
                indent=2,
            )
            + "\n"
        )
        if runtime is not None:
            runtime.terminate()
            try:
                runtime_exit = runtime.wait(timeout=10)
            except subprocess.TimeoutExpired:
                runtime.kill()
                runtime_exit = runtime.wait(timeout=5)
            runtime = None

        (output / "swap_timeline.json").write_text(
            json.dumps(
                {
                    "strategy": item.condition,
                    "action_started_ns": action_started_ns,
                    "action_finished_ns": action_finished_ns,
                    "action_duration_ns": action_finished_ns - action_started_ns,
                },
                indent=2,
            )
            + "\n"
        )
        stop_pi_telemetry(telemetry)
        finished_ns = time.time_ns()
        config_sha = hashlib.sha256(config.read_bytes()).hexdigest()
        if is_ekuiper:
            metadata = {
                "experiment": item.experiment,
                "system": "ekuiper",
                "condition": item.condition,
                "run_index": item.run_index,
                "host_tag": "rpi5",
                "generated_at": utc_now(),
                "started_at": started_at,
                "duration_ns": finished_ns - started_ns,
                "config_path": item.config,
                "config_sha256": config_sha,
                "loadgen": {"profile_path": item.loadgen_profile, "warmup_secs": item.warmup_secs},
                "exit_codes": {"ekuiper": 0},
                "comparator_audit": {
                    "path": ekuiper_audit.name,
                    "sha256": hashlib.sha256(ekuiper_audit.read_bytes()).hexdigest(),
                },
                **facts,
            }
            (output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
        else:
            provenance_path = output / "runtime-provenance.json"
            provenance = provenance_path.read_text() if provenance_path.is_file() else "null"
            merge_metadata(
                str(output / "metadata.json"),
                item.experiment,
                "rpi5",
                utc_now(),
                started_at,
                str(finished_ns - started_ns),
                item.config,
                config_sha,
                json.dumps({"profile_path": item.loadgen_profile, "warmup_secs": item.warmup_secs}),
                json.dumps({"broker": "127.0.0.1:1883", "managed_by_harness": False}),
                str(runtime_exit),
                provenance,
            )
        postprocess_run(root, item, output)
        verify_result(root, output)
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        if runtime is not None and runtime.poll() is None:
            runtime.terminate()
        stop_pi_telemetry(telemetry)
        write_status(output, item, "failed", str(error))
        print(f"[{utc_now()}] FAIL {item.result_key}: {error}", flush=True)
        return False
    write_status(output, item, "passed")
    print(f"[{utc_now()}] PASS {item.result_key}", flush=True)
    return True


def _expand_cpu_list(value: str) -> set[int]:
    cpus: set[int] = set()
    for part in value.split(","):
        bounds = part.strip().split("-", maxsplit=1)
        if len(bounds) == 1:
            cpus.add(int(bounds[0]))
        else:
            cpus.update(range(int(bounds[0]), int(bounds[1]) + 1))
    return cpus


def validate_ekuiper_process_snapshot(snapshot: dict, allowed_cpus: str) -> None:
    processes = snapshot.get("processes", [])
    if not processes or snapshot.get("main_pid") not in {process.get("pid") for process in processes}:
        raise ValueError("eKuiper main PID is absent from process snapshot")
    if snapshot.get("other_suts"):
        raise ValueError(f"concurrent SUT processes detected: {snapshot['other_suts']}")
    allowed = _expand_cpu_list(allowed_cpus)
    for process in processes:
        observed = _expand_cpu_list(str(process.get("cpus_allowed_list", "")))
        if not observed or not observed <= allowed:
            raise ValueError(
                f"eKuiper PID {process.get('pid')} affinity {sorted(observed)} is outside {allowed_cpus}"
            )


def _process_snapshot(main_pid: int) -> dict:
    output = subprocess.check_output(
        ["ps", "-e", "-o", "pid=,ppid=,comm=,args="], text=True
    )
    rows = []
    for line in output.splitlines():
        fields = line.strip().split(maxsplit=3)
        if len(fields) < 3:
            continue
        rows.append(
            {
                "pid": int(fields[0]),
                "ppid": int(fields[1]),
                "name": Path(fields[2]).name,
                "args": fields[3] if len(fields) == 4 else "",
            }
        )
    descendants = {main_pid}
    while True:
        children = {row["pid"] for row in rows if row["ppid"] in descendants}
        expanded = descendants | children
        if expanded == descendants:
            break
        descendants = expanded
    processes = []
    for row in rows:
        if row["pid"] not in descendants:
            continue
        status = Path(f"/proc/{row['pid']}/status").read_text()
        affinity = next(
            line.split(":", maxsplit=1)[1].strip()
            for line in status.splitlines()
            if line.startswith("Cpus_allowed_list:")
        )
        processes.append({**row, "cpus_allowed_list": affinity})
    other_suts = [
        row for row in rows if row["name"] in {"wafer", "wafer-runtime"}
    ]
    return {"main_pid": main_pid, "processes": processes, "other_suts": other_suts}


def _service_properties() -> dict[str, str]:
    properties = (
        "MainPID",
        "User",
        "Group",
        "ExecStart",
        "Environment",
        "CPUAffinity",
        "Restart",
        "RestartUSec",
        "WorkingDirectory",
        "FragmentPath",
        "DropInPaths",
    )
    command = ["systemctl", "show", "kuiper.service"]
    command.extend(f"--property={name}" for name in properties)
    output = subprocess.check_output(command, text=True)
    return dict(line.split("=", maxsplit=1) for line in output.splitlines() if "=" in line)


def _url_value(url: str) -> object:
    with urllib.request.urlopen(url, timeout=5) as response:
        text = response.read().decode()
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        return text


def capture_ekuiper_audit(root: Path, output: Path, allowed_cpus: str) -> Path:
    service = _service_properties()
    main_pid = int(service.get("MainPID", "0"))
    snapshot = _process_snapshot(main_pid)
    validate_ekuiper_process_snapshot(snapshot, allowed_cpus)
    unit_text = subprocess.check_output(
        ["systemctl", "cat", "kuiper.service"], text=True
    )
    source_config = Path("/etc/kuiper/mqtt_source.yaml")
    source_text = source_config.read_text()
    expected_source_text = (root / "eval/ekuiper/mqtt-source-default.yaml").read_text()
    if source_text != expected_source_text:
        raise ValueError("eKuiper MQTT source configuration differs from the canonical file")
    install_receipt = json.loads(
        Path("/var/lib/kuiper/wafer-install-receipt.json").read_text()
    )
    version = subprocess.check_output(
        ["dpkg-query", "-W", "-f=${Version}", "kuiper"], text=True
    ).strip()
    if install_receipt.get("version") != version or not re.fullmatch(
        r"[0-9a-f]{64}", str(install_receipt.get("sha256", ""))
    ):
        raise ValueError("eKuiper package version/checksum does not match install receipt")
    audit = {
        "captured_at": utc_now(),
        "system": "ekuiper",
        "version": version,
        "install_receipt": install_receipt,
        "service": {
            "properties": service,
            "unit_sha256": hashlib.sha256(unit_text.encode()).hexdigest(),
            "unit_text": unit_text,
        },
        "mqtt_source_config": {
            "path": str(source_config),
            "sha256": hashlib.sha256(source_text.encode()).hexdigest(),
            "settings": {
                "server": "tcp://127.0.0.1:1883",
                "qos": 1,
                "protocol_version": "3.1.1",
                "insecure_skip_verify": False,
            },
        },
        "process_snapshot": snapshot,
        "stream": _url_value("http://127.0.0.1:9081/streams/wafer_telemetry"),
        "rule": _url_value("http://127.0.0.1:9081/rules/pipeline_a"),
        "seed_dry_run": json.loads(
            subprocess.check_output(
                [str(root / "eval/ekuiper/seed-pipeline-a.sh"), "--dry-run"],
                cwd=root,
                text=True,
            )
        ),
    }
    path = output / "ekuiper-audit.json"
    path.write_text(json.dumps(audit, indent=2) + "\n")
    return path


def set_ekuiper_active(root: Path, active: bool) -> None:
    action = "start" if active else "stop"
    subprocess.run(["sudo", "systemctl", action, "kuiper.service"], check=True)
    if active:
        subprocess.run([str(root / "eval/ekuiper/seed-pipeline-a.sh")], cwd=root, check=True)


def run_ekuiper_item(
    root: Path,
    item: RunItem,
    selection: AttemptSelection,
) -> bool:
    output = selection.path
    output.mkdir(parents=True)
    config = root / item.config
    shutil.copy2(config, output / "config.toml")
    print(f"[{utc_now()}] START {item.result_key} -> {output}", flush=True)
    started_ns = time.time_ns()
    started_at = utc_now()
    telemetry = start_pi_telemetry(root, output)
    try:
        set_ekuiper_active(root, True)
        facts_path = output / "host-facts.json"
        subprocess.run(
            [
                sys.executable,
                str(root / "eval/scripts/validate-canonical.py"),
                "host",
                "--root",
                str(root),
                "--require-ekuiper",
                "--output",
                str(facts_path),
            ],
            cwd=root,
            check=True,
        )
        environment = os.environ.copy()
        environment["WAFER_GIT_SHA"] = json.loads(facts_path.read_text())["git_sha"]
        ekuiper_audit = capture_ekuiper_audit(root, output, item.runtime_cpus)
        with (output / "stdout.log").open("ab") as log:
            subprocess.run(
                loadgen_command(root, item, "publish", duration=item.warmup_secs),
                cwd=root,
                env=environment,
                stdout=log,
                stderr=log,
                check=True,
            )
            measurement_started_ns = time.time_ns()
            subscriber = subprocess.Popen(
                loadgen_command(root, item, "subscribe", output=output),
                cwd=root,
                env=environment,
                stdout=log,
                stderr=log,
            )
            time.sleep(0.5)
            publisher = subprocess.run(
                loadgen_command(root, item, "publish"),
                cwd=root,
                env=environment,
                stdout=log,
                stderr=log,
                check=False,
            )
            subscriber_code = wait_for_subscriber(subscriber)
            measurement_finished_ns = time.time_ns()
        if publisher.returncode != 0 or subscriber_code != 0:
            raise RuntimeError(
                f"loadgen failed: publisher={publisher.returncode}, subscriber={subscriber_code}"
            )
        (output / "measurement-window.json").write_text(
            json.dumps(
                {
                    "started_ns": measurement_started_ns,
                    "finished_ns": measurement_finished_ns,
                },
                indent=2,
            )
            + "\n"
        )

        stop_pi_telemetry(telemetry)
        finished_ns = time.time_ns()
        facts = json.loads(facts_path.read_text())
        metadata = {
            "experiment": item.experiment,
            "system": "ekuiper",
            "condition": item.condition,
            "run_index": item.run_index,
            "host_tag": "rpi5",
            "generated_at": utc_now(),
            "started_at": started_at,
            "duration_ns": finished_ns - started_ns,
            "config_path": item.config,
            "config_sha256": hashlib.sha256(config.read_bytes()).hexdigest(),
            "loadgen": {
                "profile_path": item.loadgen_profile,
                "warmup_secs": item.warmup_secs,
            },
            "exit_codes": {"ekuiper": 0},
            "comparator_audit": {
                "path": ekuiper_audit.name,
                "sha256": hashlib.sha256(ekuiper_audit.read_bytes()).hexdigest(),
            },
            **facts,
        }
        (output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
        postprocess_run(root, item, output)
        verify_result(root, output)
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        stop_pi_telemetry(telemetry)
        write_status(output, item, "failed", str(error))
        print(f"[{utc_now()}] FAIL {item.result_key}: {error}", flush=True)
        return False
    write_status(output, item, "passed")
    print(f"[{utc_now()}] PASS {item.result_key}", flush=True)
    return True


def run_item(root: Path, batch_id: str, item: RunItem) -> bool:
    condition_dir = (
        root
        / "eval/results"
        / item.experiment
        / f"rpi5-{batch_id}"
        / item.condition
    )
    selection = select_attempt(condition_dir, item.run_index)
    if selection.skip:
        print(f"[{utc_now()}] SKIP {item.result_key}: {selection.path}", flush=True)
        return True

    if item.shared_from:
        try:
            result = copy_shared_result(root, batch_id, item)
            verify_result(root, result)
            write_status(result, item, "passed", f"shared from {item.shared_from}")
            print(f"[{utc_now()}] PASS {item.result_key} (shared)", flush=True)
            return True
        except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
            write_status(selection.path, item, "failed", str(error))
            print(f"[{utc_now()}] FAIL {item.result_key}: {error}", flush=True)
            return False

    if item.experiment in {"e-swap-1", "e-swap-4", "e-swap-5"}:
        return run_hot_swap_item(root, item, selection)
    if item.experiment == "e-swap-3" and item.condition != "wafer-hotswap":
        return run_restart_item(root, item, selection)
    if item.system == "ekuiper":
        return run_ekuiper_item(root, item, selection)

    output = selection.path
    set_ekuiper_active(root, False)
    command = [
        str(root / "eval/scripts/run-experiment.sh"),
        "--config",
        str(root / item.config),
        "--experiment",
        item.experiment,
        "--host",
        "rpi5",
        "--canonical",
        "--defer-verification",
        "--skip-build",
        "--broker",
        "127.0.0.1:1883",
        "--duration",
        str(max(30, item.warmup_secs + item.measurement_secs + 30)),
        "--output-dir",
        str(output),
    ]
    if item.loadgen_profile:
        command.extend(
            [
                "--loadgen-profile",
                str(root / item.loadgen_profile),
                "--warmup-secs",
                str(item.warmup_secs),
            ]
        )
    if item.total_messages is not None:
        command.extend(["--total-messages", str(item.total_messages)])

    env = os.environ.copy()
    env["WAFER_RUNTIME_CPUSET"] = item.runtime_cpus
    env["WAFER_LOADGEN_CPUSET"] = item.support_cpus
    print(f"[{utc_now()}] START {item.result_key} -> {output}", flush=True)
    try:
        if item.startup_mode == "cold":
            subprocess.run(
                ["sudo", "sh", "-c", "sync; echo 3 > /proc/sys/vm/drop_caches"],
                check=True,
            )
        subprocess.run(command, cwd=root, env=env, check=True)
        postprocess_run(root, item, output)
        verify_result(root, output)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        write_status(output, item, "failed", str(error))
        print(f"[{utc_now()}] FAIL {item.result_key}: {error}", flush=True)
        return False
    write_status(output, item, "passed")
    print(f"[{utc_now()}] PASS {item.result_key}", flush=True)
    return True


def postprocess_run(root: Path, item: RunItem, output: Path) -> None:
    metadata_path = output / "metadata.json"
    metadata = json.loads(metadata_path.read_text())
    metadata["system"] = item.system
    metadata["condition"] = item.condition
    metadata["run_index"] = item.run_index
    metadata["config_path"] = item.config
    if item.loadgen_profile:
        profile = root / item.loadgen_profile
        loadgen = metadata.get("loadgen") or {}
        loadgen["profile_path"] = item.loadgen_profile
        loadgen["profile_sha256"] = hashlib.sha256(profile.read_bytes()).hexdigest()
        loadgen["warmup_secs"] = item.warmup_secs
        metadata["loadgen"] = loadgen
    boundary_path = output / "power-boundary.json"
    if boundary_path.is_file():
        boundary = json.loads(boundary_path.read_text())
        telemetry_path = output / "pi-telemetry.csv"
        boundary["valid"] = (
            telemetry_path.is_file()
            and telemetry_path.stat().st_size > 100
            and not (output / "telemetry-error.json").exists()
        )
        metadata["power_measurement"] = boundary
    metadata_path.write_text(json.dumps(metadata, indent=2) + "\n")

    subscriber_metadata = output / "subscriber-metadata.json"
    if subscriber_metadata.is_file():
        subprocess.run(
            [
                sys.executable,
                str(root / "eval/scripts/lib/write_throughput.py"),
                str(subscriber_metadata),
                str(output / "throughput.csv"),
            ],
            check=True,
        )

    if item.experiment.startswith("e-iso-"):
        containment = derive_containment(output)
        containment["experiment"] = item.experiment
        containment["condition"] = item.condition
        (output / "containment.json").write_text(json.dumps(containment, indent=2) + "\n")
        if item.experiment == "e-iso-8":
            recovery = summarize_recovery(output / "recovery.csv")
            (output / "recovery.json").write_text(json.dumps(recovery, indent=2) + "\n")

    loadgen = root / "target/release/wafer-loadgen"
    hdr = output / "latency.hdr"
    subscriber_metadata = output / "subscriber-metadata.json"
    if subscriber_metadata.is_file():
        subscriber = json.loads(subscriber_metadata.read_text())
        percentiles = {
            "total_count": subscriber.get("total_recorded", 0),
            "p50_ns": subscriber.get("latency_p50_ns", 0),
            "p95_ns": subscriber.get("latency_p95_ns", 0),
            "p99_ns": subscriber.get("latency_p99_ns", 0),
            "p999_ns": subscriber.get("latency_p999_ns", 0),
        }
        (output / "percentiles.json").write_text(json.dumps(percentiles, indent=2) + "\n")
    elif hdr.is_file():
        subprocess.run(
            [str(loadgen), "hdr-summary", "--hdr", str(hdr), "--output", str(output / "percentiles.json")],
            check=True,
        )
    if item.experiment == "e-perf-9":
        metadata = json.loads((output / "metadata.json").read_text())
        startup = {
            "cache_state": item.startup_mode,
            "wall_duration_ns": metadata["duration_ns"],
        }
        (output / "startup.json").write_text(json.dumps(startup, indent=2) + "\n")


def verify_result(root: Path, output: Path) -> None:
    subprocess.run(
        [
            sys.executable,
            str(root / "eval/scripts/verify-result-contract.py"),
            "--canonical",
            str(output),
        ],
        cwd=root,
        check=True,
    )


def summarise(root: Path, batch_id: str, experiments: set[str]) -> None:
    scripts = {
        "e-perf-4": "summarise-e-perf-4.sh",
        "e-perf-6": "summarise-e-perf-6-8.sh",
        "e-perf-8": "summarise-e-perf-6-8.sh",
        "e-perf-7": "summarise-e-perf-7.sh",
    }
    for experiment in EXPERIMENT_ORDER:
        script = scripts.get(experiment)
        if experiment not in experiments or script is None:
            continue
        result_root = root / "eval/results" / experiment / f"rpi5-{batch_id}"
        subprocess.run([str(root / "eval/scripts" / script), str(result_root)], check=True)


def print_plan(schedule: list[RunItem], seed: int, batch_id: str) -> None:
    print(f"batch_id={batch_id} seed={seed} host=rpi5")
    seen: set[tuple[str, str]] = set()
    for item in schedule:
        key = (item.experiment, item.condition)
        if key in seen:
            continue
        seen.add(key)
        suffix = f" shared_from={item.shared_from}" if item.shared_from else ""
        print(
            f"PLAN {item.experiment} condition={item.condition} runs="
            f"{sum(1 for candidate in schedule if candidate.experiment == item.experiment and candidate.condition == item.condition)} "
            f"warmup={item.warmup_secs}s measurement={item.measurement_secs}s "
            f"sut_cpus={item.runtime_cpus} support_cpus={item.support_cpus}{suffix}"
        )


def parse_experiments(raw: str) -> set[str]:
    if raw == "all":
        return set(CONDITIONS)
    values = {value.strip() for value in raw.split(",") if value.strip()}
    if not values:
        raise ValueError("--experiments must not be empty")
    return values


def main() -> int:
    parser = argparse.ArgumentParser(description="Run resumable canonical Pi 5 evaluations")
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[3])
    parser.add_argument("--experiments", default="all")
    parser.add_argument("--batch-id")
    parser.add_argument("--seed", type=int, default=1729)
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--execute", action="store_true")
    args = parser.parse_args()
    if args.dry_run == args.execute:
        parser.error("choose exactly one of --dry-run or --execute")

    root = args.root.resolve()
    try:
        experiments = parse_experiments(args.experiments)
        schedule = build_schedule(experiments, args.seed)
    except ValueError as error:
        parser.error(str(error))

    batch_id = args.batch_id or dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H-%M-%SZ")
    if not all(character.isalnum() or character in "._-" for character in batch_id):
        parser.error("--batch-id contains unsafe characters")
    print_plan(schedule, args.seed, batch_id)
    if args.dry_run:
        return 0

    ledger = root / "eval/results/canonical-batches" / f"rpi5-{batch_id}"
    ledger.mkdir(parents=True, exist_ok=True)
    (ledger / "schedule.json").write_text(
        json.dumps([item.__dict__ for item in schedule], indent=2) + "\n"
    )

    failures: list[str] = []
    completed = 0
    total = len(schedule)
    validation_items = [item for item in schedule if item.experiment == "e-val-1"]
    remaining_items = [item for item in schedule if item.experiment != "e-val-1"]
    write_progress(ledger, "batch-started", completed, total)
    for item in validation_items:
        write_progress(ledger, "item-started", completed, total, item.result_key, len(failures))
        if not run_item(root, batch_id, item):
            failures.append(item.result_key)
        completed += 1
        write_progress(ledger, "item-finished", completed, total, item.result_key, len(failures))

    if validation_items:
        validation_root = root / "eval/results/e-val-1" / f"rpi5-{batch_id}" / "delay-50ms"
        gate = evaluate_validation_gate(validation_root, expected_runs=30)
        (ledger / "e-val-1-gate.json").write_text(
            json.dumps(gate.__dict__, indent=2) + "\n"
        )
        if not gate.passed:
            print(f"[{utc_now()}] STOP E-Val-1 gate failed: {gate.failed_runs}", flush=True)
            write_progress(ledger, "batch-stopped", completed, total, failures=len(failures))
            return 1

    for item in remaining_items:
        write_progress(ledger, "item-started", completed, total, item.result_key, len(failures))
        if not run_item(root, batch_id, item):
            failures.append(item.result_key)
        completed += 1
        write_progress(ledger, "item-finished", completed, total, item.result_key, len(failures))
    summarise(root, batch_id, experiments)
    (ledger / "failures.json").write_text(json.dumps(failures, indent=2) + "\n")
    write_progress(ledger, "batch-finished", completed, total, failures=len(failures))
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
