"""Lifecycle shortcuts through a PTY and isolated mock API; no real mutations.

Run: uv run --with pyte python scripts/actions_smoke.py
"""
import codecs
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import socketserver
import struct
import subprocess
import tempfile
import termios
import threading
import time
from urllib.parse import parse_qs, urlsplit

import pyte


class Engine(socketserver.BaseRequestHandler):
    def handle(self):
        request = b""
        while b"\r\n\r\n" not in request and len(request) < 8192:
            data = self.request.recv(1024)
            if not data:
                return
            request += data
        method, path, _ = request.split(b"\r\n")[0].decode().split()
        url = urlsplit(path)
        code = 200
        with self.server.lock:
            states = self.server.states
            if url.path.endswith("/version"):
                data = {"ApiVersion": "1.44", "MinAPIVersion": "1.24", "Version": "fixture"}
            elif url.path.endswith("/info"):
                data = {"ID": "lifecycle-fixture", "NCPU": 2, "MemTotal": 1024 * 1024 * 1024}
            elif url.path.endswith("/containers/json"):
                data = [{"Id": key, "Names": ["/fixture-" + key], "Image": "fixture:latest", "State": state,
                         "Status": "Up" if state == "running" else "Exited (0)", "Labels": {
                             "com.docker.compose.project": "fixture-project", "com.docker.compose.service": "api"}}
                        for key, state in states.items()]
            elif "/containers/" in url.path and url.path.endswith("/json"):
                key = url.path.split("/containers/")[1].split("/")[0]
                if key not in states:
                    code, data = 404, {"message": "gone"}
                else:
                    data = {"Id": key, "Name": "/fixture-" + key, "State": {"Status": states[key], "Running": states[key] == "running"},
                            "Config": {"Labels": {"com.docker.compose.project": "fixture-project", "com.docker.compose.service": "api"}}}
            elif method in ["POST", "DELETE"]:
                assert "/containers/" in url.path, "Unexpected mutation: " + path
                target = url.path.split("/containers/")[1]
                key, _, operation = target.partition("/")
                assert key in states
                if method == "DELETE":
                    assert not operation
                    assert states[key] == "exited"
                    assert parse_qs(url.query) == {"v": ["false"], "force": ["false"], "link": ["false"]}
                    del states[key]
                else:
                    assert operation in ["start", "stop", "restart"]
                    states[key] = "exited" if operation == "stop" else "running"
                self.server.writes.append((method, target, url.query))
                code, data = 204, None
            elif url.path.endswith("/volumes"):
                data = {"Volumes": [], "Warnings": []}
            elif "/stats" in url.path:
                data = {"cpu_stats": {"cpu_usage": {"total_usage": 100}, "system_cpu_usage": 1000, "online_cpus": 2},
                        "memory_stats": {"usage": 1024, "limit": 4096}}
            elif url.path.endswith("/networks") or url.path.endswith("/images/json"):
                data = []
            else:
                code, data = 404, {"message": "unsupported fixture endpoint"}
        body = b"" if code == 204 else json.dumps(data).encode()
        self.request.sendall(f"HTTP/1.1 {code} fixture\r\nContent-Type: application/json\r\nContent-Length: {len(body)}\r\nConnection: close\r\n\r\n".encode() + body)


def main():
    root = Path(__file__).resolve().parents[1]
    with tempfile.TemporaryDirectory(prefix="tha-act-") as temporary:
        directory = Path(temporary)
        socket = directory / "api.sock"
        server = socketserver.UnixStreamServer(str(socket), Engine)
        server.lock = threading.Lock()
        server.states = {"a": "running", "b": "running"}
        server.writes = []
        threading.Thread(target=server.serve_forever, daemon=True).start()
        config = directory / "config.toml"
        config.write_text("context_order = ['Mock lifecycle']\nrefresh_seconds = 1\n[[connections]]\nname = 'Mock lifecycle'\nkind = 'docker'\n" + f"endpoint = 'unix://{socket}'\n")
        environment = {**os.environ, "HOME": temporary, "DOCKER_CONFIG": temporary,
                       "XDG_CONFIG_HOME": temporary, "XDG_RUNTIME_DIR": temporary, "TERM": "xterm-256color"}
        environment.pop("DOCKER_HOST", None)
        master, slave = pty.openpty()
        original = termios.tcgetattr(slave)
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 35, 120, 0, 0))
        screen = pyte.Screen(120, 35)
        stream = pyte.Stream(screen)
        decoder = codecs.getincrementaldecoder("utf-8")("replace")
        process = subprocess.Popen([str(root / "target/debug/tha"), "--overview", "--config", str(config)],
                                   stdin=slave, stdout=slave, stderr=slave, env=environment, start_new_session=True)

        def drain(seconds=0.3):
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                if select.select([master], [], [], 0.03)[0]:
                    data = os.read(master, 65536)
                    if b"\x1b[6n" in data:
                        os.write(master, b"\x1b[1;1R")
                    stream.feed(decoder.decode(data))

        def expect(text):
            for _ in range(20):
                if text in "\n".join(screen.display):
                    return
                drain(0.2)
            raise AssertionError(f"Missing {text!r}:\n" + "\n".join(screen.display))

        def keys(value):
            os.write(master, value)
            drain()

        def confirmed(key, action):
            keys(key)
            expect("Confirm " + action)
            expect("Runtime: Mock lifecycle")  # Never confirm an action on any host runtime.
            expect("2 target(s)")
            keys(b"y")
            expect(action + " \u00b7 Results")
            expect("Succeeded")
            keys(b"\r")

        try:
            expect("fixture-project")
            keys(b"/Mock lifecycle\rggj")
            keys(b"s")
            expect("Confirm Stop")
            expect("Runtime: Mock lifecycle")
            assert not server.writes
            keys(b"\r")  # Enter cancels by default.
            assert not server.writes
            confirmed(b"s", "Stop")
            expect("fixture-project [0/2 running]")
            confirmed(b"S", "Start")
            expect("fixture-project [2/2 running]")
            confirmed(b"r", "Restart")
            keys(b"d")
            expect("Stop all selected containers before deleting")
            assert len(server.writes) == 6
            confirmed(b"s", "Stop")
            expect("fixture-project [0/2 running]")
            keys(b"d")
            expect("Confirm Delete")
            expect("Runtime: Mock lifecycle")
            expect("ALL volumes and images stay")
            keys(b"\r")
            expect("Name does not match")
            assert len(server.writes) == 8
            keys(b"\x1b[200~fixture-project\x1b[201~\r")
            expect("Delete \u00b7 Results")
            expect("Succeeded")
            assert len(server.writes) == 10
            assert not server.states
            keys(b"\r")
            keys(b"\x03")
            process.wait(timeout=5)
            assert process.returncode == 0
            assert termios.tcgetattr(slave) == original
            print("PASS: S/s/r/d, frozen Compose targets, cancel by default, typed deletion, API results, automatic refresh, volume-preserving flags, terminal restoration; mock mutations only")
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)
            os.close(master)
            os.close(slave)
            server.shutdown()
            server.server_close()


if __name__ == "__main__":
    main()
