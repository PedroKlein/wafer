#!/usr/bin/env python3
import os
import signal
import subprocess
import sys

host, message, stream, stream_v2 = sys.argv[1:]


def run(mode, *artifacts):
    process = subprocess.Popen(
        [host, mode, *artifacts],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    try:
        stdout, stderr = process.communicate(timeout=60)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        stdout, stderr = process.communicate()
        raise AssertionError(f"{mode} timed out\nstdout:\n{stdout}\nstderr:\n{stderr}")
    assert process.returncode == 0, (
        f"{mode} exited {process.returncode}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    )
    return stdout


conformance = run("conformance", message, stream)
assert "message_outputs=8" in conformance
assert "stream_outputs=8" in conformance
assert "full_field_equality=true" in conformance
assert "ordered=true" in conformance

lifecycle = run("lifecycle", message, stream, stream_v2)
for expected in [
    "guest_error=dead-lettered:processing-failed",
    "trap=dead-lettered:session-trap",
    "trap_store_replaced=true",
    "early_close=dead-lettered:protocol-failure",
    "backpressure_stalled=true",
    "backpressure_resumed=true",
    "shutdown=forwarded",
    "shutdown_call_completed=true",
    "cancel=dead-lettered:cancelled",
    "cancel_store_replaced=true",
    "hot_swap_quiescence=drained",
    "hot_swap_interleaved=false",
    "hot_swap_store_replaced=true",
    "copy_accounting_reconciled=true",
]:
    assert expected in lifecycle, f"missing {expected!r} in:\n{lifecycle}"

print("conformance=passed")
print("lifecycle=passed")
