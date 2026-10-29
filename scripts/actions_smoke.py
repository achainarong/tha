"""Lifecycle shortcuts through a PTY and isolated mock API; no real mutations.

Run: uv run --with pyte python scripts/actions_smoke.py
"""
import codecs
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import socketserver
import struct
import subprocess
import sys
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
            if self.server.offline.exists():
                code, data = 503, {"message": "fixture runtime is stopped"}
            elif url.path.endswith("/version"):
                data = {"ApiVersion": "1.44", "MinAPIVersion": "1.24", "Version": "fixture"}
            elif url.path.endswith("/info"):
                data = {"ID": "lifecycle-fixture", "NCPU": 2, "MemTotal": 1024 * 1024 * 1024}
            elif url.path.endswith("/containers/json"):
                data = [{"Id": key, "Names": ["/fixture-" + key], "Image": "fixture:latest", "ImageID": "sha256:"+"a"*64, "State": state,
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
            elif method == "DELETE" and "/images/" in url.path:
                assert url.path.endswith("sha256:"+"c"*64)
                assert parse_qs(url.query)=={"force":["false"],"noprune":["true"]}
                self.server.unused_image=False
                self.server.resource_writes.append(path)
                data=[]
            elif method == "DELETE" and "/volumes/" in url.path:
                assert url.path.endswith("/cache_unused")
                assert parse_qs(url.query)=={"force":["false"]}
                self.server.unused_volume=False
                self.server.resource_writes.append(path)
                code,data=204,None
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
                data = {"Volumes": [{"Name":"cache_unused","Driver":"local","Mountpoint":"/data","Labels":{"com.docker.compose.project":"fixture-project"},"Scope":"local","Options":{}}] if self.server.unused_volume else [], "Warnings": []}
            elif "/stats" in url.path:
                data = {"cpu_stats": {"cpu_usage": {"total_usage": 100}, "system_cpu_usage": 1000, "online_cpus": 2},
                        "memory_stats": {"usage": 1024, "limit": 4096}}
            elif url.path.endswith("/images/json"):
                data=[{"Id":"sha256:"+"c"*64,"ParentId":"","RepoTags":[],"RepoDigests":[],"Created":1,"Size":1024,"SharedSize":-1,"Labels":{},"Containers":0}] if self.server.unused_image else []
            elif url.path.endswith("/networks"):
                data = [{"Name":"fixture-project_default","Id":"retained-network","Driver":"bridge","Scope":"local","Labels":{"com.docker.compose.project":"fixture-project"}}]
            else:
                code, data = 404, {"message": "unsupported fixture endpoint"}
        body = b"" if code == 204 else json.dumps(data).encode()
        self.request.sendall(f"HTTP/1.1 {code} fixture\r\nContent-Type: application/json\r\nContent-Length: {len(body)}\r\nConnection: close\r\n\r\n".encode() + body)


def main():
    root = Path(__file__).resolve().parents[1]
    with tempfile.TemporaryDirectory(prefix="tha-act-") as temporary:
        directory = Path(temporary)
        socket = directory / ".colima/fixture/docker.sock"
        socket.parent.mkdir(parents=True)
        binaries = directory / "bin"
        binaries.mkdir()
        offline = binaries / "runtime.offline"
        runtime_log = binaries / "runtime.log"
        fake_colima = binaries / "colima"
        fake_colima.write_text(f"#!{sys.executable}\n" + '''import json, sys, time
from pathlib import Path
root = Path(__file__).parent
if sys.argv[1:] == ["list", "--json"]:
    print(json.dumps({"name": "fixture"}))
    sys.exit(0)
assert sys.argv[1:3] == ["--profile", "fixture"], sys.argv
assert len(sys.argv) == 4 and sys.argv[3] in ["start", "stop", "restart"], sys.argv
with (root / "runtime.log").open("a") as log:
    log.write(sys.argv[3] + "\\n")
if sys.argv[3] == "stop":
    if (root / "hold-stop").exists():
        (root / "stop-started").touch()
        while not (root / "release-stop").exists():
            time.sleep(0.025)
    (root / "runtime.offline").touch()
else:
    (root / "runtime.offline").unlink(missing_ok=True)
''')
        fake_colima.chmod(0o700)
        # Keep optional machine discovery isolated too.
        fake_podman = binaries / "podman"
        fake_podman.write_text(f"#!{sys.executable}\nprint('[]')\n")
        fake_podman.chmod(0o700)
        server = socketserver.UnixStreamServer(str(socket), Engine)
        server.lock = threading.Lock()
        server.states = {"a": "running", "b": "running"}
        server.writes = []
        server.resource_writes=[]
        server.unused_image=True
        server.unused_volume=True
        server.offline = offline
        threading.Thread(target=server.serve_forever, daemon=True).start()
        config = directory / "config.toml"
        config.write_text("context_order = ['Mock lifecycle']\nrefresh_seconds = 1\n[[connections]]\nname = 'Mock lifecycle'\nkind = 'docker'\n" + f"endpoint = 'unix://{socket}'\n")
        environment = {**os.environ, "HOME": temporary, "DOCKER_CONFIG": temporary,
                       "PATH": str(binaries) + os.pathsep + os.environ.get("PATH", ""),
                       "XDG_CONFIG_HOME": temporary, "XDG_RUNTIME_DIR": temporary, "XDG_STATE_HOME": temporary, "TERM": "xterm-256color"}
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

        def confirmed(key, action, confirm=b"y"):
            keys(key)
            expect(action + " fixture-project?")
            expect("Runtime: Mock lifecycle")  # Never confirm an action on any host runtime.
            expect("2 target(s)")
            keys(confirm)
            expect(":action-results")
            assert action + " fixture-project?" not in "\n".join(screen.display)
            assert "Please wait" not in "\n".join(screen.display)
            keys(b":action-results\r")
            expect(action + " \u00b7 Results")
            expect("Succeeded")
            keys(b"\r")

        try:
            expect("fixture-project")
            keys(b"/Mock lifecycle\rggj")
            keys(b"s")
            expect("Stop fixture-project?")
            expect("Runtime: Mock lifecycle")
            assert not server.writes
            keys(b"n")
            assert "Stop fixture-project?" not in "\n".join(screen.display)
            assert not server.writes
            keys(b"s")
            expect("Stop fixture-project?")
            keys(b"\x1b")
            assert not server.writes
            confirmed(b"s", "Stop", confirm=b"\r")
            expect("fixture-project [0/2 running]")
            confirmed(b"S", "Start")
            expect("fixture-project [2/2 running]")
            confirmed(b"r", "Restart", confirm=b"\r")
            keys(b"d")
            expect("Stop all selected containers before deleting")
            assert len(server.writes) == 6
            confirmed(b"s", "Stop")
            expect("fixture-project [0/2 running]")
            keys(b"d")
            expect("Delete fixture-project?")
            expect("Runtime: Mock lifecycle")
            expect("all volumes/images stay")
            keys(b"\r")
            expect("Name does not match")
            assert len(server.writes) == 8
            keys(b"\x1b[200~fixture-project\x1b[201~\r")
            expect("Delete \u00b7 finished")
            keys(b":action-results\r")
            expect("Delete \u00b7 Results")
            expect("Succeeded")
            assert len(server.writes) == 10
            assert not server.states
            keys(b"\r")

            # Runtime rows must invoke the provider, never container mutations.
            keys(b"ggs")
            expect("Stop Colima profile fixture?")
            expect("all workloads")
            expect("Runtime: Mock lifecycle")
            keys(b"n")
            assert not runtime_log.exists()
            keys(b"s")
            expect("Stop Colima profile fixture?")
            (binaries / "hold-stop").touch()
            keys(b"\r")
            expect("Stopping…")
            first_spinner = set("\n".join(screen.display)) & set("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
            assert first_spinner, "No visible pending-action spinner"
            drain(0.25)
            next_spinner = set("\n".join(screen.display)) & set("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
            assert next_spinner and next_spinner != first_spinner, "Pending-action spinner did not animate"
            drain(1)
            assert re.search(r"Stopping… [1-9][0-9]*s", "\n".join(screen.display)), "No elapsed-time feedback"
            keys(b"?")
            expect("Shortcuts")  # Navigation must remain usable during a slow provider command.
            keys(b"\x1b")
            expect("Stopping…")
            assert not offline.exists(), "UI must not pretend the still-running runtime is stopped"
            (binaries / "release-stop").touch()
            expect("Stop \u00b7 finished")
            expect("(offline)")
            assert "Stop Colima profile fixture?" not in "\n".join(screen.display)
            assert runtime_log.read_text() == "stop\n"
            assert len(server.writes) == 10
            keys(b"\x03")
            process.wait(timeout=5)
            assert process.returncode == 0
            assert termios.tcgetattr(slave) == original

            # Relaunch while offline, then explicitly start and restart the same runtime.
            drain(0.1)
            screen = pyte.Screen(120, 35)
            stream = pyte.Stream(screen)
            decoder = codecs.getincrementaldecoder("utf-8")("replace")
            process = subprocess.Popen([str(root / "target/debug/tha"), "--overview", "--config", str(config)],
                                       stdin=slave, stdout=slave, stderr=slave, env=environment, start_new_session=True)
            drain(1)
            expect("(offline)")
            keys(b"/Mock lifecycle\rggS")
            expect("Start Colima profile fixture?")
            keys(b"\r")
            expect("Start \u00b7 finished")
            for _ in range(20):
                if "(offline)" not in "\n".join(screen.display):
                    break
                drain(0.2)
            assert "Mock lifecycle (offline)" not in "\n".join(screen.display)
            keys(b"r")
            expect("Restart Colima profile fixture?")
            keys(b"y")
            expect("Restart \u00b7 finished")
            keys(b":action-results\r")
            expect("Restart \u00b7 Results")
            expect("Succeeded")
            keys(b"\r")
            assert runtime_log.read_text() == "stop\nstart\nrestart\n"
            assert len(server.writes) == 10
            with server.lock:
                server.states.clear()  # external Compose removal; retain its labelled resources
            keys(b":refresh\r")
            expect("Shared Networks")
            assert "fixture-project [0/0 running]" not in "\n".join(screen.display),"Retained resources recreated a deleted Compose stack"
            keys(b"\x1b[1;2C")  # Shift+Right -> Images
            expect("Images overview")
            expect("unreferenced")
            keys(b"\x1b[3~")  # Delete
            expect("Confirm resource deletion")
            keys(b"\r")
            assert not server.resource_writes,"Empty confirmation must not delete"
            keys(("sha256:"+"c"*64).encode()+b"\r")
            expect("No resources reported")
            assert len(server.resource_writes)==1
            keys(b"\x1b[1;2C")  # Shift+Right -> Volumes
            expect("cache_unused")
            keys(b"\x1b[3~")
            expect("permanently destroys")
            keys(b"cache_unused\r")
            expect("No resources reported")
            assert len(server.resource_writes)==2
            keys(b"\x03")
            process.wait(timeout=5)
            assert process.returncode == 0
            assert termios.tcgetattr(slave) == original
            print("PASS: container safeguards, runtime lifecycle, Shift-arrow overviews, exact-confirmation unused image/volume deletion and refresh, no force/pruning, background results, terminal restoration; mock mutations only")
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
