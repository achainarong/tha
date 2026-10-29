"""Offline PTY measurements, not timing-sensitive CI assertions.

Run: uv run --with pyte python scripts/perf_smoke.py --binary target/release/tha
Measures demo-mode idle CPU and navigation-to-visible-selection latency. No runtime
connections, user configuration, mutations, or credentials are used.
"""
import argparse
import codecs
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import statistics
import struct
import subprocess
import tempfile
import termios
import time

import pyte


def cpu_seconds(pid):
    value = subprocess.check_output(["ps", "-p", str(pid), "-o", "time="], text=True).strip()
    days, separator, clock = value.partition("-")
    if not separator:
        clock, days = value, "0"
    result = float(days) * 86400
    for index, part in enumerate(reversed(clock.split(":"))):
        result += float(part) * 60 ** index
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/tha"))
    parser.add_argument("--idle-seconds", type=float, default=8)
    parser.add_argument("--samples", type=int, default=40)
    args = parser.parse_args()
    if args.idle_seconds <= 0 or args.samples < 2:
        parser.error("Use positive idle seconds and at least two samples")
    binary = args.binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="tha-perf-") as temporary:
        master, slave = pty.openpty()
        original = termios.tcgetattr(slave)
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 35, 120, 0, 0))
        screen = pyte.Screen(120, 35)
        stream = pyte.Stream(screen)
        decoder = codecs.getincrementaldecoder("utf-8")("replace")
        environment = {**os.environ, "HOME": temporary, "DOCKER_CONFIG": temporary,
                       "XDG_CONFIG_HOME": temporary, "XDG_STATE_HOME": temporary,
                       "XDG_RUNTIME_DIR": temporary, "TERM": "xterm-256color"}
        environment.pop("DOCKER_HOST", None)
        process = subprocess.Popen([str(binary), "--demo"], stdin=slave, stdout=slave,
                                   stderr=slave, env=environment, start_new_session=True)

        def pump(wait=0.002):
            if select.select([master], [], [], wait)[0]:
                data = os.read(master, 65536)
                if b"\x1b[6n" in data:
                    os.write(master, b"\x1b[1;1R")
                stream.feed(decoder.decode(data))

        def drain(seconds):
            until = time.monotonic() + seconds
            while time.monotonic() < until:
                pump(min(0.02, max(0, until - time.monotonic())))

        def selection():
            return tuple((y, "".join(screen.buffer[y][x].data for x in range(60)
                                     if screen.buffer[y][x].reverse))
                         for y in range(2, 33)
                         if any(screen.buffer[y][x].reverse for x in range(60)))

        try:
            drain(1)
            assert "Demo" in "\n".join(screen.display), "Expected isolated demo mode"
            before_cpu, before_wall = cpu_seconds(process.pid), time.monotonic()
            drain(args.idle_seconds)
            wall = time.monotonic() - before_wall
            cpu = max(0, cpu_seconds(process.pid) - before_cpu)
            timings = []
            for index in range(args.samples):
                before = selection()
                started = time.monotonic()
                os.write(master, b"j" if index % 2 == 0 else b"k")
                while selection() == before:
                    assert time.monotonic() - started < 3, "Navigation did not become visible"
                    pump()
                timings.append((time.monotonic() - started) * 1000)
                drain(0.03)
            os.write(master, b"\x03")
            process.wait(timeout=5)
            assert process.returncode == 0
            assert termios.tcgetattr(slave) == original, "Terminal settings were not restored"
            ordered = sorted(timings)
            print(json.dumps({"binary": str(binary), "idle_seconds": round(wall, 2),
                              "idle_cpu_seconds": round(cpu, 4), "idle_cpu_percent_one_core": round(cpu / wall * 100, 2),
                              "navigation_samples": len(timings), "navigation_p50_ms": round(statistics.median(timings), 2),
                              "navigation_p95_ms": round(ordered[min(len(ordered) - 1, int(len(ordered) * 0.95))], 2)}, indent=2))
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)
            os.close(master)
            os.close(slave)


if __name__ == "__main__":
    main()
