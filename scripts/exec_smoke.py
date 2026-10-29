"""Test embedded exec with a fake engine/CLI; never exec into a real container.

Run: uv run --with pyte python scripts/exec_smoke.py
"""
import codecs
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import socketserver
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time

import pyte

IMAGE_ID = "sha256:" + "a" * 64


class Engine(socketserver.BaseRequestHandler):
    def handle(self):
        request = b""
        while b"\r\n\r\n" not in request and len(request) < 8192:
            chunk = self.request.recv(1024)
            if not chunk:
                return
            request += chunk
        path = request.split(b" ")[1].decode()
        if path.endswith("/version"):
            data = {"ApiVersion": "1.53", "MinAPIVersion": "1.24", "Version": "29.0"}
        elif path.endswith("/info"):
            data = {"ID": "tha-fixture", "ServerVersion": "29.0", "NCPU": 4, "MemTotal": 1073741824}
        elif "/stats?" in path:
            self.server.samples += 1
            data = {"cpu_stats": {"cpu_usage": {"total_usage": self.server.samples * 100},
                                  "system_cpu_usage": self.server.samples * 1000, "online_cpus": 4},
                    "memory_stats": {"usage": 4096, "limit": 8192, "stats": {"inactive_file": 1024}}}
        elif "/containers/json" in path:
            data = [{"Id": "fixture-id", "Names": ["/fixture-api"], "Image": "fixture", "ImageID": IMAGE_ID,
                     "State": "running", "Status": "Up", "Labels": {
                         "com.docker.compose.project": "fixture-project",
                         "com.docker.compose.service": "api"}}]
        elif "/images/json" in path:
            data = [{"Id": IMAGE_ID, "ParentId": "", "RepoTags": ["fixture:latest"],
                     "RepoDigests": [], "Created": 1, "Size": 1024, "SharedSize": -1,
                     "Labels": {}, "Containers": 1}]
        elif "/images/" in path:
            data = {"Id": IMAGE_ID, "Os": "linux", "Architecture": "amd64"}
        elif "/networks" in path:
            data = []
        elif "/volumes" in path:
            data = {"Volumes": [], "Warnings": []}
        else:
            data = {}
        body = json.dumps(data).encode()
        self.request.sendall(
            f"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {len(body)}\r\nConnection: close\r\n\r\n".encode() + body
        )


def main():
    root = Path(__file__).resolve().parents[1]
    with tempfile.TemporaryDirectory(prefix="tha-ex-") as temporary:
        directory = Path(temporary)
        endpoint = "unix://" + str(directory / "s")
        server = socketserver.UnixStreamServer(str(directory / "s"), Engine)
        server.samples = 0
        worker = threading.Thread(target=server.serve_forever, daemon=True)
        worker.start()
        binary_directory = directory / "bin"
        binary_directory.mkdir()
        marker = directory / "arguments.json"
        geometry = directory / "geometry.json"
        fake = binary_directory / "docker"
        fake.write_text(
            f"#!{sys.executable}\n"
            "import json, os, sys, tty\n"
            f"assert sys.argv[1:] == ['--host', {endpoint!r}, 'exec', '--interactive', '--tty', '--', 'fixture-id', 'sh'], sys.argv\n"
            "assert 'DOCKER_CONTEXT' not in os.environ\n"
            f"if os.path.exists({str(marker)!r}):\n"
            "    print('FAKE_SHELL_EXPECTED_FAILURE', flush=True)\n"
            "    sys.exit(23)\n"
            f"open({str(marker)!r}, 'w').write(json.dumps(sys.argv[1:]))\n"
            f"geometry = {str(geometry)!r}\n"
            + r'''
assert os.isatty(0) and os.isatty(1)
tty.setraw(0)
def out(text):
    os.write(1, (text + "\r\n").encode())
out("FAKE_SHELL_READY")
os.write(1, b"\x1b]52;c;c2VjcmV0\x07\x1b]2;secret-host-title\x07")
line = bytearray()
while True:
    data = os.read(0, 4096)
    if not data:
        break
    for byte in data:
        if byte in (10, 13):
            command = line.decode(errors="replace")
            line.clear()
            out("")
            if command == "exit":
                out("FAKE_SHELL_EXIT")
                sys.exit(0)
            elif command == "probe":
                out("PROBE-OK")
            elif command == "size":
                size = os.get_terminal_size(0)
                open(geometry, "w").write(json.dumps({"cols":size.columns, "rows":size.lines}))
                out("SIZE-OK")
            else:
                out("INPUT:" + command)
        elif byte == 3:
            line.clear()
            out("CTRL-C-RECEIVED")
        elif byte == 27:
            out("ESC-RECEIVED")
        elif byte == 29:
            out("BAD-DETACH-LEAK")
        else:
            line.append(byte)
            os.write(1, bytes([byte]))
'''
        )
        fake.chmod(0o700)
        config = directory / "config.toml"
        config.write_text(
            "log_wrap = false\ncontext_order = ['Mock']\n[[connections]]\nname = 'Mock'\nkind = 'docker'\n"
            f"endpoint = '{endpoint}'\n"
        )
        master, slave = pty.openpty()
        original = termios.tcgetattr(slave)
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 35, 120, 0, 0))
        screen = pyte.Screen(120, 35)
        stream = pyte.Stream(screen)
        decoder = codecs.getincrementaldecoder("utf-8")("replace")
        raw_output = bytearray()
        environment = {**os.environ, "HOME": temporary, "DOCKER_CONFIG": temporary,
                       "XDG_CONFIG_HOME": temporary, "XDG_RUNTIME_DIR": temporary,
                       "PATH": str(binary_directory) + os.pathsep + os.environ["PATH"],
                       "DOCKER_CONTEXT": "must-not-reach-exec", "TERM": "xterm-256color"}
        environment.pop("DOCKER_HOST", None)
        process = subprocess.Popen(
            [str(root / "target/debug/tha"), "--overview", "--config", str(config)],
            stdin=slave, stdout=slave, stderr=slave, env=environment, start_new_session=True,
        )

        def drain(seconds=0.35):
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                if select.select([master], [], [], 0.03)[0]:
                    data = os.read(master, 65536)
                    raw_output.extend(data)
                    if b"\x1b[6n" in data:
                        os.write(master, b"\x1b[1;1R")
                    stream.feed(decoder.decode(data))

        def expect(text):
            assert text in "\n".join(screen.display), f"Missing {text!r}:\n" + "\n".join(screen.display)

        def keys(value, seconds=0.35):
            os.write(master, value)
            drain(seconds)

        try:
            drain(1.8)
            expect("fixture-project")
            expect("40.0%")
            expect("3.0 KiB")
            expect("linux/amd64")
            keys(b"j\r")
            expect("All runtimes › Mock › fixture-project")
            keys(b"jlj4\r", 1.0)
            expect("FAKE_SHELL_READY")
            expect("Explorer")
            expect("4 Exec")
            expect("SHELL INPUT")
            samples_before = server.samples
            assert marker.exists(), "Exec did not reach the fake runtime CLI"
            assert not termios.tcgetattr(slave)[3] & termios.ICANON, "Tha unexpectedly handed off the host terminal"
            keys(b"p\r")
            expect("INPUT:p")
            expect("▸ Performance")
            keys(b"\x03")
            expect("CTRL-C-RECEIVED")
            expect("Explorer")
            keys(b"\x1b")
            expect("ESC-RECEIVED")
            assert server.samples > samples_before, "Metrics stopped while the shell owned input"
            keys(b"\x1d")
            keys(b"p")
            expect("scale")
            assert "BAD-DETACH-LEAK" not in "\n".join(screen.display)
            keys(b"4\r")
            keys(b"\x1b[200~probe\x1b[201~\r")
            expect("PROBE-OK")
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 25, 80, 0, 0))
            screen.resize(25, 80)
            os.kill(process.pid, signal.SIGWINCH)
            drain()
            keys(b"size\r")
            expect("SIZE-OK")
            dimensions = json.loads(geometry.read_text())
            assert dimensions["cols"] < 80 and dimensions["rows"] < 25, dimensions
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 35, 120, 0, 0))
            screen.resize(35, 120)
            os.kill(process.pid, signal.SIGWINCH)
            drain()
            keys(b"exit\r", 1.0)
            expect("Exited 0")
            expect("All runtimes › Mock › fixture-project")
            keys(b"e", 1.0)
            expect("Exited 23")
            expect("FAKE_SHELL_EXPECTED_FAILURE")
            expect("All runtimes › Mock › fixture-project")
            assert b"]52;" not in raw_output and b"secret-host-title" not in raw_output, "Child controls escaped to the host terminal"
            keys(b"p")
            keys(b"2")
            expect("Wrap: off")  # Config sets the initial state only.
            keys(b"w")
            expect("Wrap: on")
            keys(b"q")
            keys(b"q")
            keys(b"q")
            assert process.wait(timeout=3) == 0
            assert termios.tcgetattr(slave) == original
            print("PASS: embedded PTY, 4/Enter, fixed endpoint, input routing, Ctrl-] navigation, paste, resize, OSC containment, platform, metrics, exit/error state, wrap toggle, terminal restoration")
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            os.close(master)
            os.close(slave)
            server.shutdown()
            server.server_close()
            worker.join()


if __name__ == "__main__":
    main()
