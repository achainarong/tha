"""Read-only TUI smoke test. Run: uv run --with pyte python scripts/smoke.py."""
import codecs
import fcntl
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import termios
import time

import pyte


def main():
    root = Path(__file__).resolve().parents[1]
    master, slave = pty.openpty()
    original = termios.tcgetattr(slave)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 35, 120, 0, 0))
    screen = pyte.Screen(120, 35)
    stream = pyte.Stream(screen)
    decoder = codecs.getincrementaldecoder("utf-8")("replace")

    process = subprocess.Popen(
        [str(root / "target/debug/tha"), "--demo"],
        stdin=slave, stdout=slave, stderr=slave,
        env={**os.environ, "TERM": "xterm-256color"},
        start_new_session=True,
    )

    def drain(seconds=0.3):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if select.select([master], [], [], 0.03)[0]:
                try:
                    data = os.read(master, 65536)
                except OSError:
                    break
                if not data:
                    break
                # Answer cursor-position queries if a terminal backend requests one.
                if b"\x1b[6n" in data:
                    os.write(master, b"\x1b[1;1R")
                stream.feed(decoder.decode(data))

    def expect(text):
        assert text in "\n".join(screen.display), f"Missing {text!r}:\n" + "\n".join(screen.display)

    def keys(value, seconds=0.3):
        os.write(master, value)
        drain(seconds)

    try:
        drain(0.7)
        expect("Tha")
        expect("Explorer")
        expect("shop-api-1")
        expect("shop [3/3 running]")
        expect("Services (2)")
        expect("api [2/2 running]")
        keys(b" ")
        assert "Services (2)" not in "\n".join(screen.display), "Space did not collapse the project"
        keys(b" ")
        expect("Services (2)")
        keys(b"\r")
        expect("All runtimes › OrbStack › shop")
        assert "Colima" not in "\n".join(screen.display), "Workspace leaked another engine"
        keys(b"\x7f")
        expect("Colima")
        keys(b"2\t", 1.0)
        expect("Following")
        expect("INFO request")  # The RFC3339 prefix can wrap the remainder onto another row.
        expect("Wrap: on")
        keys(b"w")
        expect("Wrap: off")
        keys(b"w")
        expect("Wrap: on")
        keys(b"/upstream\r")
        expect("matches /upstream")
        expect("INFO request")  # Search keeps surrounding entries, even when wrapped.
        assert any(cell.reverse and cell.bold and cell.underscore for row in screen.buffer.values() for cell in row.values()), "No current-match highlight"
        keys(b"n")
        expect("matches /upstream")
        keys(b"\x1b")
        keys(b":networks\r")
        expect("Network membership")
        expect("shop_default")
        keys(b"p\t\t")  # Details -> Explorer -> Performance.
        expect("Performance \u00b7 Connected runtimes")
        keys(b"G")
        performance_start = next(index for index, row in enumerate(screen.display) if "\u25be Performance" in row[:60])
        performance_text = "\n".join(row[:60] for row in screen.display[performance_start:-2])
        assert "Podman" not in performance_text, "Offline runtime leaked into Performance"
        assert "Unavailable" not in performance_text
        keys(b"gg\r")
        right = "\n".join(row[60:] for row in screen.display)
        assert "Colima / development" in right, "Enter did not inspect the first connected runtime"
        keys(b"pq/shop-api-1\rG")
        keys(b"P")
        expect("Performance · Selected container")
        expect("Container · shop-api-1")
        keys(b"z")
        expect("latest")
        border = next(row for row in screen.display if "· Selected container" in row)
        assert border[-1] == "┓", "Expanded Performance did not span terminal width"
        keys(b"\x1b")
        border = next(row for row in screen.display if "· Selected container" in row)
        assert border[59] == "┓", "Esc did not restore the compact panel"
        keys(b"p:networks\r")
        keys(b"?")
        expect("Shortcuts")
        keys(b"jjj")
        expect("Toggle wrapping on/off immediately")
        keys(b"G")
        expect("Exec and lifecycle actions can change workloads")
        keys(b"\x1b")
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 20, 60, 0, 0))
        screen.resize(20, 60)
        os.kill(process.pid, signal.SIGWINCH)
        drain()
        expect("Network membership")
        keys(b"q")
        expect("Explorer")
        keys(b"q")
        assert process.wait(timeout=3) == 0
        assert termios.tcgetattr(slave) == original, "Terminal settings were not restored"
        print("PASS: tree, Space, counts, Compose workspace/back, wrap/search, connected-only Performance navigation/Enter, palette, cheatsheet, resize, quit, terminal restoration")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        os.close(master)
        os.close(slave)


if __name__ == "__main__":
    main()
