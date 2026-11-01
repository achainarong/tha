"""Read-only terminal regression checks for the staged feature batches.

Run: uv run --with pyte python scripts/features_smoke.py
"""
import codecs
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import termios
import tempfile
import time

import pyte


def main():
    root = Path(__file__).resolve().parents[1]
    temporary = tempfile.TemporaryDirectory(prefix="tha-features-")
    directory = Path(temporary.name)
    master, slave = pty.openpty()
    original = termios.tcgetattr(slave)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 35, 140, 0, 0))
    screen = pyte.Screen(140, 35)
    stream = pyte.Stream(screen)
    decoder = codecs.getincrementaldecoder("utf-8")("replace")
    process = subprocess.Popen([str(root / "target/debug/tha"), "--demo"], stdin=slave, stdout=slave,
                               stderr=slave, env={**os.environ, "TERM": "xterm-256color", "HOME": str(directory), "XDG_CONFIG_HOME": str(directory)}, start_new_session=True)

    def drain(seconds=0.3):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if select.select([master], [], [], 0.03)[0]:
                data = os.read(master, 65536)
                if b"\x1b[6n" in data:
                    os.write(master, b"\x1b[1;1R")
                stream.feed(decoder.decode(data))

    def expect(text):
        for _ in range(12):
            if text in "\n".join(screen.display):
                return
            drain(0.2)
        raise AssertionError(f"Missing {text!r}:\n" + "\n".join(screen.display))

    def keys(value):
        os.write(master, value)
        drain()

    try:
        expect("Tha")
        keys(b"/shop-api-1\rG7")
        expect("7 Layers")
        expect("Filesystem layers (2)")
        expect("Build history (2)")
        keys(b"G\r")
        expect("Explicit metadata-only step")
        expect("ENV APP_MODE=production")
        expect("may contain secrets")
        keys(b"\x1b")
        expect("Filesystem layers (2)")
        keys(b"\t")
        keys(b"a")
        expect("Start · disabled")
        expect("Demo mode")
        keys(b"\r")
        expect("Start · disabled")  # A disabled action must not dismiss or mutate.
        keys(b"\x1b")
        keys(b":file-explorer\r")
        expect("File explorer")
        keys(b"/hostname\r")
        expect("Find in /etc: hostname")
        expect("Sample contents of /etc/hostname")
        keys(b"e")
        expect("Demo mode: files are not edited")
        keys(b":download\r")
        expect("Size: 12 B")
        expect("hostname")
        keys(b"\x1b")
        keys(b"\x06")  # Ctrl-f searches the entire source filesystem, not just /etc.
        expect("Find in /: /hostname")
        keys(b"\x15mydb.dump\r")
        expect("Find in /: mydb.dump")
        expect("is big (2.4 GB)")
        keys(b"d")
        expect("2.4 GB")
        keys(b"\r")
        expect("Demo mode: files are not downloaded")
        keys(b"\x1b")
        keys(b"\t")
        expect("No downloads in this session yet.")
        keys(b"\x1b")
        keys(b":logs\r")
        expect("Wrap: on")
        keys(b"\x1b[13~")  # F3
        expect("Filter retained logs · Text")
        keys(b"retry UPSTREAM\r")  # Every word, in any order, ignoring case.
        expect("Filter: retry UPSTREAM")
        expect('"level":"warn"')
        keys(b"\x1b[13~")
        keys(b"\t")  # Tab reaches the JSON filter, which combines with the text.
        expect("Filter retained logs · Advanced JSON")
        keys(b"\x15")  # Ctrl-u clears the initial {}.
        keys(b'\x1b[200~{"min_level":"warn"}\x1b[201~\r')
        expect("Filter: retry UPSTREAM + advanced")
        expect('"level":"warn"')
        destination = directory / "filtered.jsonl"
        keys(b":export-logs\r")
        expect("Export retained logs")
        expect("SECRETS")
        keys(str(destination).encode() + b"\r")
        expect("Exported")
        original_export = destination.read_bytes()
        records = [json.loads(line) for line in original_export.splitlines()]
        assert records and all(record["level"] == "warn" for record in records)
        assert destination.stat().st_mode & 0o777 == 0o600
        keys(b":export-logs\r")
        keys(str(destination).encode() + b"\r")
        expect("Cannot create export")
        assert destination.read_bytes() == original_export
        keys(b":clear-log-filter\r")
        expect("Filter: off")
        keys(b"\x1b[19~")  # F8 -> Images overview
        expect("Images overview")
        expect("Sort: Usage")
        expect("Status")
        keys(b"/redis\r")
        expect("1 / 3 shown")
        keys(b"\r")
        expect("Details")
        expect("redis:7")
        keys(b"\x1b")
        expect("Images overview")
        keys(b"/\x15\r")  # clear applied query
        expect("3 / 3 shown")
        keys(b"fs")
        expect("Filter: Running")
        expect("Sort: Name")
        keys(b"\x1b[19~")  # F8 -> Volumes overview
        expect("Volumes overview")
        expect("shop_data")
        expect("old_cache")
        keys(b"G ")
        expect("1 marked")
        keys(b" ")
        expect("0 marked")
        keys(b"u")
        expect("1 marked")
        keys(b"/old_cache\r")
        expect("0 marked")
        expect("1 / 2 shown")
        keys(b"/\x15\r")
        expect("2 / 2 shown")
        keys(b"uc")
        expect("0 marked")
        keys(b"fffff\r")
        expect("Bind mounts")
        expect("/home/demo/shop")
        keys(b"\x1b")
        keys(b"\x1b[18~")  # F7 -> Images, retaining its filter/sort
        expect("Filter: Running")
        expect("Sort: Name")
        keys(b"\x1b[18~")  # F7 -> Containers
        keys(b"7")
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 15, 50, 0, 0))
        screen.resize(15, 50)
        os.kill(process.pid, signal.SIGWINCH)
        drain()
        expect("7 Layers")
        keys(b"\x03")
        process.wait(timeout=5)
        assert process.returncode == 0
        assert termios.tcgetattr(slave) == original
        print("PASS: selectable inventory tables, search/clear, filters/sort, image and bind-mount details, F7/F8 overview cycling/state retention, Layers/details, file explorer/preview/download confirmation, log filtering/private export, resize, terminal restoration; demo only")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=5)
        os.close(master)
        os.close(slave)
        temporary.cleanup()


if __name__ == "__main__":
    main()
