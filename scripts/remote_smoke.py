#!/usr/bin/env python3
"""Exercise remote servers for real: a disposable SSH server, `tha-agent setup`, and the client.

`--try` stops after setup and keeps the server running, to try the Servers modal by hand.

Needs Docker and OpenSSH. Only touches containers it creates, and removes them afterwards.
Host keys go in a private known_hosts through a PATH shim, never in ~/.ssh/known_hosts.
"""
import fcntl
import os
import pty
import select
import shlex
import socket
import stat
import struct
import subprocess
import sys
import tempfile
import termios
import time
from pathlib import Path

SERVER = "tha-remote-smoke-server"
TARGET = "tha-remote-smoke-target"
ROOT = Path(__file__).resolve().parent.parent
PORT = "22922"


def run(*command, check=True, **options):
    return subprocess.run(command, check=check, text=True, capture_output=True, **options)


def inside(script, check=True):
    return run("docker", "exec", SERVER, "sh", "-c", script, check=check)


def cleanup():
    run("docker", "rm", "-f", SERVER, TARGET, check=False)


def refused(result, what):
    if result.returncode == 0:
        sys.exit(f"FAILED: the restricted key could {what}")
    print(f"ok: the restricted key cannot {what}")


def forward_is_blocked(ssh):
    """A forward only opens when used, so try it: Docker must never answer through it."""
    with tempfile.TemporaryDirectory(prefix="f", dir="/tmp") as short:
        path = f"{short}/s"
        process = subprocess.Popen(
            [*ssh, "-N", "-L", f"{path}:/var/run/docker.sock", "127.0.0.1"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        try:
            for _ in range(40):
                if os.path.exists(path) or process.poll() is not None:
                    break
                time.sleep(0.25)
            answer = b""
            if os.path.exists(path):
                with socket.socket(socket.AF_UNIX) as client:
                    client.settimeout(5)
                    client.connect(path)
                    client.sendall(b"GET /_ping HTTP/1.1\r\nHost: x\r\n\r\n")
                    try:
                        answer = client.recv(256)
                    except (TimeoutError, ConnectionError):
                        pass
            if b"HTTP/" in answer:
                sys.exit(f"FAILED: Docker answered through a forwarded socket: {answer!r}")
        finally:
            process.terminate()
            process.wait()
    print("ok: the restricted key cannot reach Docker through a forwarded socket")


def masters():
    found = run("pgrep", "-f", f"ssh .*-p {PORT} .*-M -N", check=False)
    return found.stdout.split()


def run_tui(scratch, key, environment, connect, seconds):
    """The real binary on a real terminal, with its own state so nothing of yours changes."""
    config = scratch / f"config-{connect}.toml"
    config.write_text(
        "discover_connections = false\nsave_preferences = false\n[[remotes]]\n"
        f'name = "smoke"\nhost = "127.0.0.1"\nuser = "tha"\nport = {PORT}\n'
        f'identity_file = "{key}"\nconnect = "{connect}"\n'
    )
    environment = dict(
        environment,
        XDG_CONFIG_HOME=str(scratch),
        XDG_STATE_HOME=str(scratch),
        TERM="xterm-256color",
    )
    pid, descriptor = pty.fork()
    if pid == 0:
        os.chdir("/")
        os.execve(
            str(ROOT / "target/debug/tha"),
            ["tha", "--standalone", "--overview", "--config", str(config)],
            environment,
        )
    fcntl.ioctl(descriptor, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 140, 0, 0))
    output = b""
    during = []
    deadline = time.time() + seconds
    while time.time() < deadline:
        ready, _, _ = select.select([descriptor], [], [], 0.2)
        if ready:
            try:
                output += os.read(descriptor, 65536)
            except OSError:
                break
        during = during or masters()
    os.write(descriptor, b"\x03")
    time.sleep(1.0)
    try:
        os.waitpid(pid, os.WNOHANG)
    except ChildProcessError:
        pass
    try:
        os.close(descriptor)
    except OSError:
        pass
    return output.decode("utf-8", "replace"), during


def process_exists(path):
    try:
        os.kill(int(path.name.rsplit("-", 1)[1]), 0)
    except (ProcessLookupError, ValueError):
        return False
    except PermissionError:
        pass
    return True


def tui_checks(scratch, key, environment):
    output, during = run_tui(scratch, key, environment, "manual", 3)
    if during:
        sys.exit("FAILED: a manual server opened SSH at startup")
    if "not connected" not in output:
        sys.exit(f"FAILED: the manual server is not shown as not connected: {output[-500:]!r}")
    print("ok: a manual server contacts nothing at startup")
    output, during = run_tui(scratch, key, environment, "startup", 8)
    if not during:
        sys.exit("FAILED: a startup server did not open SSH")
    if "docker" not in output:
        sys.exit(f"FAILED: the connected engine is not shown: {output[-500:]!r}")
    print("ok: a startup server connects and shows its engine in the UI")
    time.sleep(1)
    if masters():
        sys.exit("FAILED: quitting left an SSH session open")
    print("ok: quitting closes the SSH session")
    runtime = Path(os.environ.get("XDG_RUNTIME_DIR", tempfile.gettempdir()))
    if any(not path.name.endswith(str(os.getpid())) and not process_exists(path) for path in runtime.glob("tha-remote-*")):
        sys.exit("FAILED: quitting left a session directory behind")
    print("ok: quitting removes its session directory")


def try_it(scratch, key):
    print(
        f"""
Server ready. In another terminal: ./target/debug/tha
Then press . (or type :servers), press a, and enter:

  Name     try
  Host     127.0.0.1
  User     tha
  Port     {PORT}
  SSH key  {key}

Use Test connection and answer yes to the host key. OpenSSH records it in your
~/.ssh/known_hosts as [127.0.0.1]:{PORT}. Afterwards remove it with:
  ssh-keygen -R '[127.0.0.1]:{PORT}'
and remove the "try" server in the Servers modal.

Container to practise on: {TARGET}. Compare what is allowed with what is refused.
"""
    )
    input("Press Enter here to remove the disposable server…")


def main():
    subprocess.run(["cargo", "build"], cwd=ROOT, check=True)
    cleanup()
    with tempfile.TemporaryDirectory(prefix="tha-smoke-") as directory:
        scratch = Path(directory)
        key, wrong = scratch / "key", scratch / "wrong"
        for path in (key, wrong):
            run("ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(path))
        known = scratch / "known_hosts"
        run("docker", "run", "-d", "--name", TARGET, "nginx:latest")
        run(
            "docker", "run", "-d", "--name", SERVER,
            "-p", f"127.0.0.1:{PORT}:22",
            "-v", "/var/run/docker.sock:/var/run/docker.sock",
            "archlinux:latest", "sleep", "3600",
        )
        print("Preparing the disposable server…")
        inside("pacman -Sy --noconfirm --needed openssh shadow >/dev/null 2>&1 && ssh-keygen -A")
        # The socket's group ID has no sensible name in a fresh image; this only edits the
        # container's own /etc/group, and setup refuses privileged group names.
        inside(
            "group=$(stat -c %G /var/run/docker.sock); "
            '[ "$group" = docker ] || groupmod -n docker "$group"'
        )
        run("docker", "cp", str(ROOT / "target/debug/tha-agent"), f"{SERVER}:/usr/local/bin/tha-agent")
        inside("chown root:root /usr/local/bin/tha-agent && chmod 755 /usr/local/bin/tha-agent && /usr/sbin/sshd")
        # This disposable container has no systemd PID 1. Exercise the actual persistent
        # daemon/relay here; unit generation is checked separately in Rust tests.
        inside("mkdir -p /run/tha-agent /var/lib/tha-agent /etc/systemd/system")
        inside("cat > /usr/local/bin/systemctl <<'SH'\n#!/bin/sh\n"
               "case $1 in\nrestart)\n"
               "if [ -f /run/tha-agent-test.pid ]; then kill \"$(cat /run/tha-agent-test.pid)\" 2>/dev/null || true; sleep 3; fi\n"
               "HOME=/var/lib/tha-agent XDG_DATA_HOME=/var/lib/tha-agent nohup /usr/local/bin/tha-agent daemon >/var/log/tha-agent-test.log 2>&1 &\n"
               "echo $! >/run/tha-agent-test.pid\nsleep 1\n;;\nesac\nSH\n"
               "chmod 755 /usr/local/bin/systemctl")

        public = (scratch / "key.pub").read_text().strip()
        setup = f"tha-agent setup --user tha --key {shlex.quote(public)}"
        print(inside(setup).stdout.strip())
        inside(setup)
        lines = inside("grep -c . /home/tha/.ssh/authorized_keys").stdout.strip()
        if lines != "1":
            sys.exit(f"FAILED: rerunning setup left {lines} keys")
        print("ok: setup is safe to rerun")

        for _ in range(20):
            scan = run("ssh-keyscan", "-p", PORT, "127.0.0.1", check=False)
            if scan.stdout:
                known.write_text(scan.stdout)
                break
            time.sleep(0.5)
        else:
            sys.exit("FAILED: sshd did not start")

        if "--try" in sys.argv:
            try_it(scratch, key)
            return

        shim = scratch / "bin"
        shim.mkdir()
        wrapper = shim / "ssh"
        wrapper.write_text(f'#!/bin/sh\nexec /usr/bin/ssh -o UserKnownHostsFile={known} "$@"\n')
        wrapper.chmod(wrapper.stat().st_mode | stat.S_IEXEC)

        ssh = [
            str(wrapper), "-i", str(key), "-o", "IdentitiesOnly=yes", "-o", "BatchMode=yes",
            "-p", PORT, "-l", "tha",
        ]
        refused(run(*ssh, "127.0.0.1", "sh -c id", check=False), "run a shell command")
        refused(run(*ssh, "127.0.0.1", check=False, stdin=subprocess.DEVNULL), "open a shell")
        forward_is_blocked(ssh)

        environment_for_tui = dict(os.environ, PATH=f"{shim}:{os.environ['PATH']}")
        tui_checks(scratch, key, environment_for_tui)

        environment = dict(
            os.environ,
            PATH=f"{shim}:{os.environ['PATH']}",
            THA_SMOKE_PORT=PORT,
            THA_SMOKE_KEY=str(key),
            THA_SMOKE_WRONG_KEY=str(wrong),
            THA_SMOKE_TARGET=TARGET,
            THA_SMOKE_SERVER=SERVER,
        )
        root_key = scratch / "root_key"
        run("ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(root_key))
        root_public = (scratch / "root_key.pub").read_text().strip()
        inside(
            "mkdir -p /root/.ssh && chmod 700 /root/.ssh && "
            f"echo {shlex.quote(root_public)} >> /root/.ssh/authorized_keys && "
            "chmod 600 /root/.ssh/authorized_keys && tha-agent setup --user root"
        )
        if inside("grep -c . /root/.ssh/authorized_keys").stdout.strip() != "1":
            sys.exit("FAILED: root setup changed root's keys")
        print("Checking a root login…")
        subprocess.run(
            ["cargo", "test", "--test", "remote_smoke", "the_whole_path", "--", "--ignored", "--nocapture"],
            cwd=ROOT,
            env=dict(environment, THA_SMOKE_USER="root", THA_SMOKE_KEY=str(root_key)),
            check=True,
        )
        subprocess.run(
            ["cargo", "test", "--test", "remote_smoke", "--", "--ignored", "--test-threads=1", "--nocapture"],
            cwd=ROOT, env=environment, check=True,
        )
    print("Remote smoke test passed.")


if __name__ == "__main__":
    try:
        main()
    finally:
        cleanup()
