"""Hermetic Podman discovery/refresh smoke; never starts a real VM or container.

Run: uv run --with pyte python scripts/podman_smoke.py
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
import sys
import tempfile
import termios
import threading
import time

import pyte
sys.dont_write_bytecode = True
from exec_smoke import Engine


def main():
    root = Path(__file__).resolve().parents[1]
    with tempfile.TemporaryDirectory(prefix="tha-pod-") as temporary:
        directory = Path(temporary)
        servers = []
        for name in ["one.sock", "two.sock"]:
            server = socketserver.UnixStreamServer(str(directory / name), Engine)
            server.samples = 0
            threading.Thread(target=server.serve_forever, daemon=True).start()
            servers.append(server)
        state = directory / "state.json"
        calls = directory / "calls.jsonl"

        def update(running=True, socket="one.sock", malformed=False):
            staged = directory / "state.new"
            staged.write_text(json.dumps({"running": running, "socket": str(directory / socket), "malformed": malformed}))
            staged.replace(state)

        update()
        binary_directory = directory / "bin"
        binary_directory.mkdir()
        fake = binary_directory / "podman"
        fake.write_text(
            f"#!{sys.executable}\n"
            "import json, sys\n"
            f"state = json.load(open({str(state)!r}))\n"
            f"with open({str(calls)!r}, 'a') as log: log.write(json.dumps(sys.argv[1:]) + '\\n')\n"
            "if sys.argv[1:] == ['machine', 'list', '--format', 'json']:\n"
            "    print('{broken' if state['malformed'] else json.dumps([{'Name':'fixture', 'Running':state['running']}]))\n"
            "elif sys.argv[1:] == ['machine', 'inspect', 'fixture']:\n"
            "    assert state['running'], 'stopped machines must not be inspected'\n"
            "    print(json.dumps([{'ConnectionInfo':{'PodmanSocket':{'Path':state['socket']}}}]))\n"
            "else:\n"
            "    raise SystemExit('Unexpected/mutating Podman invocation: ' + repr(sys.argv))\n"
        )
        fake.chmod(0o700)
        config = directory / "config.toml"
        config.write_text("context_order = ['Podman / fixture']\nrefresh_seconds = 1\n")
        environment = {**os.environ, "HOME": temporary, "DOCKER_CONFIG": temporary,
                       "XDG_CONFIG_HOME": temporary, "XDG_RUNTIME_DIR": temporary, "XDG_STATE_HOME": temporary,
                       "PATH": str(binary_directory) + os.pathsep + os.environ["PATH"],
                       "TERM": "xterm-256color"}
        environment.pop("DOCKER_HOST", None)
        master, slave = pty.openpty()
        original = termios.tcgetattr(slave)
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 45, 160, 0, 0))
        screen = pyte.Screen(160, 45)
        stream = pyte.Stream(screen)
        decoder = codecs.getincrementaldecoder("utf-8")("replace")
        process = subprocess.Popen([str(root / "target/debug/tha"), "--overview", "--config", str(config)],
                                   stdin=slave, stdout=slave, stderr=slave, env=environment, start_new_session=True)

        def drain(seconds=0.4):
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                if select.select([master], [], [], 0.03)[0]:
                    data = os.read(master, 65536)
                    if b"\x1b[6n" in data:
                        os.write(master, b"\x1b[1;1R")
                    stream.feed(decoder.decode(data))

        def expect(text):
            for _ in range(16):
                if text in "\n".join(screen.display):
                    return
                drain(0.25)
            raise AssertionError(f"Missing {text!r}:\n" + "\n".join(screen.display))

        def keys(value):
            os.write(master, value)
            drain()

        try:
            expect("Podman / fixture")
            keys(b"/Podman / fixture\rgg")
            expect("fixture-project")
            expect("Images (1)")
            expect("fixture:latest")
            expect("3.0 KiB")
            update(running=False)
            keys(b":refresh\r")
            expect("(offline)")
            expect("machine is stopped")
            old_samples = servers[0].samples
            update(socket="two.sock")
            keys(b":refresh\r")
            expect("two.sock")
            expect("Images (1)")
            drain(1.5)
            assert servers[1].samples > 0, "Metrics did not move to the refreshed socket"
            assert servers[0].samples == old_samples, "Old socket continued to be sampled"
            update(socket="two.sock", malformed=True)
            keys(b":refresh\r")
            expect("invalid JSON")
            assert process.poll() is None, "Invalid discovery crashed Tha"
            update(socket="two.sock")
            keys(b":refresh\r")
            expect("Images (1)")
            keys(b"\x03")
            process.wait(timeout=5)
            assert process.returncode == 0
            assert termios.tcgetattr(slave) == original, "Terminal mode was not restored"
            commands = [json.loads(line) for line in calls.read_text().splitlines()]
            assert all(command in [["machine", "list", "--format", "json"], ["machine", "inspect", "fixture"]] for command in commands)
            print("PASS: native Podman discovery, images, grouping, metrics, stopped machine, socket refresh, invalid JSON/recovery, read-only commands, terminal restoration")
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)
            os.close(master)
            os.close(slave)
            for server in servers:
                server.shutdown()
                server.server_close()


if __name__ == "__main__":
    main()
