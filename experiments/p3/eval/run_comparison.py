#!/usr/bin/env python3
import argparse
import hashlib
import json
import os
import signal
import subprocess
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
P3 = REPO / "experiments/p3"


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def wit_tree_hash():
    digest = hashlib.sha256()
    for path in sorted((P3 / "wit/wafer-pipeline-0.2.0").glob("*.wit")):
        digest.update(path.name.encode())
        digest.update(b"\0")
        digest.update(path.read_bytes())
    return digest.hexdigest()


def command_output(*command):
    return subprocess.check_output(command, cwd=REPO, text=True).strip()


def run_leaf(command, timeout, stdout_path):
    process = subprocess.Popen(
        command,
        cwd=REPO,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    try:
        stdout, stderr = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        stdout, stderr = process.communicate()
        process.returncode = 124
    stdout_path.write_text(stdout + ("\n--- stderr ---\n" + stderr if stderr else ""))
    if process.returncode != 0:
        raise RuntimeError(f"leaf exited {process.returncode}: {' '.join(command)}")
    lines = [line for line in stdout.splitlines() if line.strip()]
    if not lines:
        raise RuntimeError(f"leaf produced no JSON: {' '.join(command)}")
    return json.loads(lines[-1])


def next_attempt(slot):
    slot.mkdir(parents=True, exist_ok=True)
    existing = [int(path.name.split("-")[1]) for path in slot.glob("attempt-*")]
    number = max(existing, default=0) + 1
    path = slot / f"attempt-{number:03}"
    path.mkdir()
    return path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", required=True, type=Path)
    parser.add_argument("--p2", required=True, type=Path)
    parser.add_argument("--p3-message", required=True, type=Path)
    parser.add_argument("--p3-stream", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--execute", action="store_true")
    args = parser.parse_args()

    contract = json.loads((P3 / "experiment-contract.json").read_text())
    schedule = json.loads((P3 / "dry-run-schedule.json").read_text())
    print(f"triplets={schedule['triplet_count']} runs={schedule['run_count']}")
    if not args.execute:
        for row in schedule["triplets"]:
            print(f"{row['triplet_index_global']:02} {row['condition']} {' '.join(row['arm_order'])}")
        return

    git_sha = command_output("git", "rev-parse", "HEAD")
    if command_output("git", "status", "--porcelain"):
        raise SystemExit("comparison requires a clean source tree")
    artifacts = {"p2": args.p2, "p3-message": args.p3_message, "p3-stream": args.p3_stream}
    for path in [args.host, *artifacts.values()]:
        if not path.is_file():
            raise SystemExit(f"missing artifact: {path}")

    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    batch = {
        "schema": "wafer-p3-poc-batch-v1",
        "source": {"git_sha": git_sha, "git_dirty": False},
        "contract_sha256": sha256(P3 / "experiment-contract.json"),
        "schedule_sha256": sha256(P3 / "dry-run-schedule.json"),
        "wit_tree_sha256": wit_tree_hash(),
        "host_sha256": sha256(args.host),
        "components": {arm: sha256(path) for arm, path in artifacts.items()},
        "toolchain": {
            "rustc": command_output("rustc", "+1.98.1", "--version"),
            "cargo": command_output("cargo", "+1.98.1", "--version"),
            "wasm_tools": "wasm-tools 1.259.0",
            "wasmtime_revision": "11eac9de8693e1a69f86815d1a58918edd5c1785",
            "production_wasmtime_revision": "e9f1ea232fd245aea338ab3eb7d73487ae75cab1"
        },
        "triplets": schedule["triplets"],
    }
    (output / "batch.json").write_text(json.dumps(batch, indent=2) + "\n")

    controls = contract["controlled_factors"]
    admitted = []
    for row in schedule["triplets"]:
        for position, arm in enumerate(row["arm_order"], 1):
            slot = output / "raw" / row["condition"] / f"triplet-{row['replicate']:02}" / f"{position}-{arm}"
            attempt = next_attempt(slot)
            stdout_path = attempt / "stdout.log"
            command = [
                str(args.host), "bench", arm, str(artifacts[arm]), str(row["payload_bytes"]),
                str(row["depth"]), str(controls["warmup_messages"]), str(controls["measured_messages"]),
            ]
            try:
                result = run_leaf(command, controls["outer_timeout_seconds"], stdout_path)
                leaf = {
                    "schema": "wafer-p3-poc-raw-leaf-v1",
                    "source": {"git_sha": git_sha, "git_dirty": False, "evidence_class": "experimental-diagnostic"},
                    "arm": arm,
                    "condition": row["condition"],
                    "triplet_index": row["replicate"],
                    "position": position,
                    "controlled_factors": {
                        "payload_bytes": row["payload_bytes"],
                        "depth": row["depth"],
                        "queue_capacity_elements": controls["queue_capacity_elements"],
                        "stream_session_capacity_elements": controls["stream_session_capacity_elements"],
                        "worker_threads": controls["worker_threads"],
                        "warmup_messages": controls["warmup_messages"],
                        "measured_messages": controls["measured_messages"],
                        "release": controls["release"],
                        "locked": controls["locked"],
                        "metering_mode": controls["metering_mode"],
                        "outer_timeout_seconds": controls["outer_timeout_seconds"]
                    },
                    "hashes": {
                        "executable": batch["host_sha256"],
                        "component": batch["components"][arm],
                        "wit_tree": batch["wit_tree_sha256"],
                        "experiment_contract": batch["contract_sha256"]
                    },
                    "toolchain": batch["toolchain"],
                    **result,
                    "status": "passed"
                }
                (attempt / "result.json").write_text(json.dumps(leaf, indent=2) + "\n")
                admitted.append(attempt / "result.json")
                print(f"PASS {row['condition']} triplet={row['replicate']} position={position} arm={arm}", flush=True)
            except Exception as error:
                (attempt / "failure.json").write_text(json.dumps({
                    "schema": "wafer-p3-poc-failed-attempt-v1",
                    "source_sha": git_sha,
                    "command": command,
                    "failure": str(error),
                }, indent=2) + "\n")
                print(f"FAIL {row['condition']} triplet={row['replicate']} position={position} arm={arm}: {error}", flush=True)
                raise

    index = "".join(f"{sha256(path)}  {path.relative_to(output)}\n" for path in admitted)
    (output / "admitted-leaves.sha256").write_text(index)
    print(f"admitted_leaves={len(admitted)}")


if __name__ == "__main__":
    main()
