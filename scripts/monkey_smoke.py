#!/usr/bin/env python3
"""Random keys, mouse events, pastes and resizes against `tha --demo`, looking for panics and hangs.

Demo mode contacts nothing, and each run also gets an empty home folder and no PATH. Uses bubblewrap
for network and filesystem isolation where it works. Needs a built binary: make build.
"""
import argparse
import collections
import fcntl
import os
import pty
import random
import re
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
KEYS = [*"abcdefghijklmnoprstuvwxyzABCDEFGHIJKLMNOPRSTUVWXYZ0123456789", *":/?.,-_=+[]{}!@#$%^&*()<>|~'\" ",
        "\r", "\r", "\t", "\x1b", "\x7f", "\x1b[A", "\x1b[B", "\x1b[C", "\x1b[D", "\x1b[A", "\x1b[B", "\x1b[Z",
        "\x1b[5~", "\x1b[6~", "\x1b[H", "\x1b[F", "\x1b[1;5C", "\x1b[1;5D", "\x1b[3~", "\x1b[1;2A", "\x1b[1;2B",
        "\x04", "\x05", "\x06", "\x0b", "\x0c", "\x0e", "\x10", "\x12", "\x15", "\x17", "\x19"]
PASTES = ["日本語のコンテナ名", "ท่าข้อความ", "🦀🦀🦀👨‍👩‍👧‍👦", "é" * 40, "x" * 5000, "\x1b[31mred",
          "a\nb\r\nc", "../../etc/passwd", "(((", "[a-"]
SIZES = [(1, 1), (1, 80), (2, 5), (3, 10), (4, 30), (5, 20), (8, 300), (10, 40), (12, 60), (24, 80), (40, 200),
         (200, 8), (60, 20), (2, 400)]
HANG_SECONDS = 15
# `q` and Ctrl-C are left out, so a run stays in the UI. Far more restarts than this means the
# program is not starting at all, and a run with no panics would prove nothing.
MAX_STARTS_PER_RUN = 20


def commands():
    source = (ROOT / "src/shortcuts.rs").read_text()
    words = set(re.findall(r'=> "([a-z][a-z-]+)"', source)) | set(re.findall(r'"([a-z][a-z]+-[a-z-]+)"', source))
    return sorted(words) + ["servers", "settings", "file-explorer", "file-explorer /", "history", "help", "logs"]


def sandboxed(work, program):
    """The command for one run, isolated by bubblewrap when it is usable here."""
    environment = {"HOME": f"{work}/home", "PATH": "/nonexistent", "XDG_RUNTIME_DIR": f"{work}/run",
                   "XDG_STATE_HOME": f"{work}/home/state", "TERM": "xterm-256color", "LANG": "C.UTF-8",
                   "RUST_BACKTRACE": "0"}
    shell = f'exec "{program}" --demo --demo-servers 2>>"{work}/stderr"'
    bubblewrap = shutil.which("bwrap")
    if bubblewrap and subprocess.run([bubblewrap, "--ro-bind", "/", "/", "--unshare-net", "/bin/true"],
                                     capture_output=True).returncode == 0:
        inner = {key: value.replace(work, "/run/w") for key, value in environment.items()}
        arguments = [bubblewrap, "--ro-bind", "/", "/", "--dev-bind", "/dev", "/dev", "--proc", "/proc", "--tmpfs", "/home",
                     "--tmpfs", "/tmp", "--tmpfs", "/run", "--bind", work, "/run/w", "--unshare-net", "--unshare-pid",
                     "--die-with-parent", "--chdir", "/run/w/cwd", "--clearenv"]
        for key, value in inner.items():
            arguments += ["--setenv", key, value]
        return arguments + ["/bin/sh", "-c", shell.replace(work, "/run/w")], None, f"{work}/cwd"
    return ["/bin/sh", "-c", shell], environment, f"{work}/cwd"


def resize(descriptor, rows, columns):
    fcntl.ioctl(descriptor, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))


def run(job):
    seed, rows, columns, seconds, binary, words = job
    rng = random.Random(seed)
    result = {"seed": seed, "size": (rows, columns), "starts": 0, "inputs": 0, "hang": False}
    with tempfile.TemporaryDirectory(prefix="tha-m", dir=os.environ.get("THA_MONKEY_DIR")) as work:
        shutil.copy(binary, f"{work}/tha")
        for folder in ("home", "cwd", "run"):
            os.makedirs(f"{work}/{folder}", mode=0o700)
        program, environment, directory = sandboxed(work, f"{work}/tha")
        start = time.time()
        while time.time() - start < seconds and not result["hang"]:
            result["starts"] += 1
            if result["starts"] > MAX_STARTS_PER_RUN:
                break
            pid, descriptor = pty.fork()
            if pid == 0:
                os.chdir(directory)
                if environment is None:
                    os.execv(program[0], program)
                os.execve(program[0], program, environment)
            resize(descriptor, rows, columns)
            last_output, alive = time.time(), True
            while alive and time.time() - start < seconds:
                if select.select([descriptor], [], [], 0.003)[0]:
                    try:
                        alive = bool(os.read(descriptor, 65536))
                    except OSError:
                        alive = False
                    last_output = time.time()
                    continue
                if time.time() - last_output > HANG_SECONDS:
                    result["hang"] = True
                    break
                roll = rng.random()
                if roll < 0.03:
                    resize(descriptor, *rng.choice(SIZES))
                    continue
                if roll < 0.08:
                    text = f":{rng.choice(words)}\r"
                elif roll < 0.10:
                    text = f"\x1b[200~{rng.choice(PASTES)}\x1b[201~"
                elif roll < 0.15:
                    text = (f"\x1b[<{rng.choice([0, 1, 2, 64, 65, 32])};{rng.randint(1, columns + 2)};"
                            f"{rng.randint(1, rows + 2)}{rng.choice('Mm')}")
                else:
                    text = rng.choice(KEYS)
                result["inputs"] += 1
                try:
                    os.write(descriptor, text.encode())
                except OSError:
                    alive = False
            try:
                os.kill(pid, signal.SIGKILL)
            except OSError:
                pass
            os.waitpid(pid, 0)
            os.close(descriptor)
        stderr = Path(f"{work}/stderr")
        text = stderr.read_text(errors="replace") if stderr.exists() else ""
        crash_log = Path(f"{work}/home/state/tha/crash.log")
        text += crash_log.read_text(errors="replace") if crash_log.exists() else ""
        result["panics"] = re.findall(r"panicked at ([^\n]+)\n([^\n]*)", text)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default=str(ROOT / "target/debug/tha"))
    parser.add_argument("--seconds", type=int, default=6, help="length of each run")
    parser.add_argument("--runs", type=int, default=len(SIZES))
    parser.add_argument("--jobs", type=int, default=4)
    options = parser.parse_args()
    words = commands()
    jobs = [(seed, *SIZES[seed % len(SIZES)], options.seconds, options.binary, words) for seed in range(options.runs)]
    with ThreadPoolExecutor(options.jobs) as pool:
        results = list(pool.map(run, jobs))
    inputs, starts = sum(r["inputs"] for r in results), sum(r["starts"] for r in results)
    print(f"{len(results)} runs, {inputs} inputs, {starts} starts, {len(words)} commands")
    failed = False
    panics = collections.Counter((where, message[:160]) for r in results for where, message in r["panics"])
    for (where, message), count in panics.most_common():
        failed = True
        print(f"PANIC {count}x at {where}\n  {message}")
    for result in results:
        if result["hang"]:
            failed = True
            print(f"HANG with seed {result['seed']} at {result['size']}")
        if result["starts"] > MAX_STARTS_PER_RUN or result["inputs"] == 0:
            failed = True
            print(f"INVALID run, the program did not stay up: seed {result['seed']}, {result['starts']} starts")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
