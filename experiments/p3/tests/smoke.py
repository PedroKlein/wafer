#!/usr/bin/env python3
import os
import signal
import socket
import subprocess
import sys
import threading

host, message, stream, http = sys.argv[1:]


def run(mode, artifact, env=None):
    process = subprocess.Popen(
        [host, mode, artifact],
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    try:
        stdout, stderr = process.communicate(timeout=15)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        stdout, stderr = process.communicate()
        raise SystemExit(f"{mode} timed out: {stdout}{stderr}")
    if process.returncode != 0:
        raise SystemExit(f"{mode} failed: {stdout}{stderr}")
    return stdout.strip()


assert run("message", message) == "message-ok"
assert run("stream", stream) == "stream-ok"

listener = socket.socket()
listener.bind(("127.0.0.1", 0))
listener.listen()
listener.settimeout(0.5)
port = listener.getsockname()[1]
env = os.environ.copy()
env["HTTP_SERVER"] = f"127.0.0.1:{port}"
assert run("http-deny", http, env) == "http-denied"
try:
    listener.accept()
    raise AssertionError("denied request reached loopback server")
except TimeoutError:
    pass
listener.close()

listener = socket.socket()
listener.bind(("127.0.0.1", 0))
listener.listen()
port = listener.getsockname()[1]
requests = []


def serve_once():
    connection, _ = listener.accept()
    requests.append(connection.recv(4096))
    connection.sendall(
        b"HTTP/1.1 204 No Content\r\n"
        b"Content-Length: 0\r\n"
        b"Connection: close\r\n\r\n"
    )
    connection.close()
    listener.close()


server = threading.Thread(target=serve_once)
server.start()
env["HTTP_SERVER"] = f"127.0.0.1:{port}"
assert run("http-allow", http, env) == "http-allowed"
server.join(5)
assert not server.is_alive()
assert len(requests) == 1
assert requests[0].startswith(b"GET /smoke HTTP/1.1\r\n")
print("message_count=1")
print("stream_count=1")
print("denied_server_count=0")
print("allowed_server_count=1")
print("http_suspended_resumed=true")
