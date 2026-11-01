"""Start an actual release package in an isolated pseudo-terminal (requires pyte)."""
import argparse
import codecs
import errno
import fcntl
import os
from pathlib import Path
import pty
import select
import shutil
import struct
import subprocess
import tarfile
import tempfile
import termios
import time

from release_smoke import check_archive


def check_startup(archive, version, timeout=15):
    check_archive(archive)
    with tempfile.TemporaryDirectory(prefix="tha-startup-") as temporary:
        directory = Path(temporary)
        binary = directory / "tha"
        with tarfile.open(archive, "r:gz") as package:
            with package.extractfile("tha") as source, binary.open("wb") as destination:
                shutil.copyfileobj(source, destination)
        binary.chmod(0o700)
        home = directory / "home"
        home.mkdir()
        env = {"PATH": os.defpath, "HOME": str(home), "TERM": "xterm-256color",
               "XDG_CONFIG_HOME": str(home / "config"), "XDG_DATA_HOME": str(home / "data"),
               "XDG_STATE_HOME": str(home / "state"), "XDG_CACHE_HOME": str(home / "cache"),
               "TMPDIR": temporary, "LANG": "C.UTF-8"}
        result = subprocess.run([str(binary), "--version"], env=env, cwd=directory,
                                capture_output=True, text=True, check=True, timeout=timeout)
        if result.stdout.strip() != f"tha {version}":
            raise AssertionError(f"Unexpected package version: {result.stdout!r}; expected tha {version}")

        import pyte

        master, slave = pty.openpty()
        process = None
        screen = pyte.Screen(120, 35)
        stream = pyte.Stream(screen)
        decoder = codecs.getincrementaldecoder("utf-8")("replace")
        try:
            original = termios.tcgetattr(slave)
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 35, 120, 0, 0))
            process = subprocess.Popen([str(binary), "--demo"], env=env, cwd=directory,
                                       stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
            deadline = time.monotonic() + timeout
            ready = False
            while time.monotonic() < deadline:
                if select.select([master], [], [], 0.05)[0]:
                    try:
                        data = os.read(master, 65536)
                    except OSError as error:
                        if error.errno != errno.EIO:
                            raise
                        break
                    if not data:
                        break
                    if b"\x1b[6n" in data:
                        os.write(master, b"\x1b[1;1R")
                    stream.feed(decoder.decode(data))
                display = "\n".join(screen.display)
                if not ready and all(text in display for text in ("Tha", "Explorer", "shop-api-1")):
                    ready = True
                    os.write(master, b"q")
                if process.poll() is not None:
                    break
            if not ready:
                raise AssertionError("Release did not render the demo UI:\n" + "\n".join(screen.display))
            if process.poll() is None:
                raise AssertionError("Release did not exit after q:\n" + "\n".join(screen.display))
            if process.returncode != 0:
                raise AssertionError(f"Release exited with {process.returncode}:\n" + "\n".join(screen.display))
            if termios.tcgetattr(slave) != original:
                raise AssertionError("Release did not restore terminal settings")
            print(f"PASS: {archive.name}: version, demo UI, clean quit, terminal restoration")
        finally:
            if process is not None and process.poll() is None:
                process.kill()
                process.wait()
            os.close(master)
            os.close(slave)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--version", required=True)
    options = parser.parse_args()
    check_startup(options.archive, options.version)
