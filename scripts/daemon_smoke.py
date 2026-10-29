"""Isolated daemon + SQLite + History PTY checks; never connects to real runtimes.

Run: uv run --with pyte python scripts/daemon_smoke.py
Benchmark collector: uv run --with pyte python scripts/daemon_smoke.py --benchmark --containers 100 --binary target/release/tha
"""
import argparse
import codecs
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import socket as unix_socket
import socketserver
import sqlite3
import struct
import subprocess
import tempfile
import termios
import threading
import time
from urllib.parse import urlsplit

import pyte


class Engine(socketserver.BaseRequestHandler):
    def handle(self):
        request = b""
        while b"\r\n\r\n" not in request and len(request) < 65536:
            data = self.request.recv(65536)
            if not data:
                return
            request += data
        method, raw_path, _ = request.split(b"\r\n")[0].decode().split()
        path = urlsplit(raw_path).path
        assert method == "GET", "Collector attempted a mutation"
        if path.endswith("/version"):
            data = {"ApiVersion": "1.44", "MinAPIVersion": "1.24", "Version": "fixture"}
        elif path.endswith("/info"):
            data = {"ID": "history-fixture", "NCPU": 2, "MemTotal": 1024**3}
        elif path.endswith("/containers/json"):
            data = [{"Id": f"container-{i}", "Names": [f"/history-container-{i}"], "State": "running", "Image": "fixture:local", "Labels": {"com.docker.compose.project": "fixture", "com.docker.compose.service": "api"}} for i in range(self.server.containers)]
        elif path.endswith("/stats"):
            key = path.split("/containers/")[1].split("/")[0]
            with self.server.guard:
                count = self.server.calls.get(key, 0) + 1
                self.server.calls[key] = count
            data = {"cpu_stats": {"cpu_usage": {"total_usage": count*1000}, "system_cpu_usage": count*10000, "online_cpus": 2}, "memory_stats": {"usage": 1024**2, "limit": 1024**3}, "networks": {"eth0": {"rx_bytes": count*1000, "tx_bytes": count*2000}}, "blkio_stats": {"io_service_bytes_recursive": [{"op": "Read", "value": count*3000}, {"op": "Write", "value": count*4000}]}}
        elif path.endswith("/volumes"):
            data = {"Volumes": [], "Warnings": []}
        elif path.endswith("/networks") or path.endswith("/images/json"):
            data = []
        else:
            data = {}
        body = json.dumps(data).encode()
        try:
            self.request.sendall(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: " + str(len(body)).encode() + b"\r\nConnection: close\r\n\r\n" + body)
        except BrokenPipeError:
            pass


class Server(socketserver.ThreadingUnixStreamServer):
    # The default backlog of five is below the collector's eight parallel
    # requests and would benchmark fixture connection failures, not ingestion.
    request_queue_size = 1024
    daemon_threads = True


def wait(predicate, label, seconds=15):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        try:
            if predicate():
                return
        except (sqlite3.Error, OSError, ValueError):
            pass
        time.sleep(.1)
    raise AssertionError(label)


def cpu_seconds(clock):
    return sum(float(part)*60**index for index, part in enumerate(reversed(clock.split(":"))))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/tha"))
    parser.add_argument("--containers", type=int, default=1)
    parser.add_argument("--benchmark", action="store_true")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="tha-daemon-") as temporary:
        root = Path(temporary)
        socket = root / "engine.sock"
        database = root / "history" / "metrics.sqlite"
        config = root / "config.toml"
        config.write_text('save_preferences=false\ndiscover_connections=false\nrefresh_seconds=2\nhistory_disk_mb=64\nhistory_database=' + json.dumps(str(database)) + '\n[[connections]]\nname="History fixture"\nkind="docker"\nendpoint=' + json.dumps("unix://"+str(socket)) + '\n')
        binaries = root / "bin"
        binaries.mkdir()
        for name in ["podman", "launchctl", "systemctl"]:
            script = binaries / name
            script.write_text('#!/bin/sh\nprintf "[]\\n"\n')
            script.chmod(0o700)
        env = {**os.environ, "HOME": temporary, "DOCKER_CONFIG": temporary, "XDG_CONFIG_HOME": temporary, "XDG_STATE_HOME": temporary, "XDG_RUNTIME_DIR": temporary, "TERM": "xterm-256color", "PATH": str(binaries)+os.pathsep+os.environ.get("PATH", "")}
        env.pop("DOCKER_HOST", None)
        server = Server(str(socket), Engine)
        server.daemon_threads = True
        server.containers = args.containers
        server.calls = {}
        server.guard = threading.Lock()
        threading.Thread(target=server.serve_forever, daemon=True).start()
        daemon = None
        ui = None
        master = slave = None

        def launch():
            return subprocess.Popen([str(binary), "--config", str(config), "daemon"], stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)

        def scalar(sql):
            with sqlite3.connect(database, timeout=.5) as db:
                return db.execute(sql).fetchone()[0]

        def live_rows(since=None, all_containers=False):
            keys=[f"/container-{i}" for i in range(args.containers)] if all_containers else ["/container-0"]
            payload=json.dumps({"version":1,"runtime":"Docker:history-fixture","keys":keys,"since_ms":since}).encode()
            with unix_socket.socket(unix_socket.AF_UNIX,unix_socket.SOCK_STREAM) as client:
                client.settimeout(2)
                client.connect(str(database.with_suffix(".sock")))
                client.sendall(struct.pack("!I",len(payload))+payload)
                def read_exact(size):
                    result=b""
                    while len(result)<size:
                        data=client.recv(size-len(result))
                        assert data,"Daemon closed live socket"
                        result+=data
                    return result
                size=struct.unpack("!I",read_exact(4))[0]
                assert size<=2*1024*1024
                return json.loads(read_exact(size))["samples"]

        try:
            daemon = launch()
            wait(lambda: database.exists() and len(live_rows(all_containers=True))==args.containers and all(sample["usage"] and sample["usage"]["cpu_percent"] is not None for sample in live_rows(all_containers=True)), "Collector did not publish live samples for all containers")
            if args.benchmark:
                before = time.monotonic()
                wall_before=int(time.time()*1000)
                cpu_before = subprocess.check_output(["ps", "-p", str(daemon.pid), "-o", "time="], text=True).strip()
                time.sleep(16)
                cpu_after = subprocess.check_output(["ps", "-p", str(daemon.pid), "-o", "time="], text=True).strip()
                rss = subprocess.check_output(["ps", "-p", str(daemon.pid), "-o", "rss="], text=True).strip()
                cpu_elapsed=time.monotonic()-before
                daemon.send_signal(signal.SIGTERM)
                daemon.wait(timeout=5)
                assert daemon.returncode==0,daemon.stderr.read().decode()
                started = time.monotonic()
                scalar("SELECT MAX(cpu) FROM samples WHERE container=1 AND at >= CAST(strftime('%s','now') AS INTEGER)*1000-86400000")
                latency = (time.monotonic()-started)*1000
                size = sum(p.stat().st_size for p in database.parent.glob("metrics.sqlite*") if p.is_file())
                print(json.dumps({"containers": args.containers, "seconds": round(cpu_elapsed,2), "new_samples": scalar(f"SELECT COUNT(*) FROM samples WHERE at>={wall_before}"), "valid_cpu_samples": scalar(f"SELECT COUNT(cpu) FROM samples WHERE at>={wall_before}"), "daemon_cpu_percent_one_core": round((cpu_seconds(cpu_after)-cpu_seconds(cpu_before))/cpu_elapsed*100,2), "daemon_rss_kib": int(rss), "database_and_wal_bytes": size, "short_window_selected_query_ms": round(latency,2), "dropped_samples": scalar("SELECT dropped FROM health")}, indent=2))
                return
            wait(lambda:len(live_rows(int(time.time()*1000)-45_000))>=3,"Recent live window was not available before first flush")
            assert scalar("SELECT COUNT(*) FROM samples")==0,"Samples were written before the 15-second flush"
            assert live_rows()[0]["usage"]["network_rx_rate"] is not None,"Live I/O rates missing"
            second = subprocess.run([str(binary), "--config", str(config), "daemon"], capture_output=True, env=env, timeout=5)
            assert second.returncode != 0 and b"Another daemon" in second.stderr
            master, slave = pty.openpty()
            original = termios.tcgetattr(slave)
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 55, 160, 0, 0))
            screen = pyte.Screen(160, 55)
            stream = pyte.Stream(screen)
            decoder = codecs.getincrementaldecoder("utf-8")("replace")
            ui = subprocess.Popen([str(binary), "--overview", "--config", str(config)], stdin=slave, stdout=slave, stderr=slave, env=env, start_new_session=True)

            def drain(seconds=.3):
                end = time.monotonic()+seconds
                while time.monotonic()<end:
                    if select.select([master], [], [], .02)[0]:
                        data = os.read(master, 65536)
                        if b"\x1b[6n" in data:
                            os.write(master, b"\x1b[1;1R")
                        stream.feed(decoder.decode(data))

            def keys(value):
                os.write(master, value)
                drain()

            def expect(value):
                for _ in range(30):
                    if value in "\n".join(screen.display):
                        return
                    drain(.2)
                raise AssertionError(f"Missing {value!r}:\n"+"\n".join(screen.display))

            drain(1)
            keys(b"/history-container-0\rG5z")
            expect("Metrics ·")
            expect("1 minute")
            expect("Disk read / write")
            expect("Network receive / transmit")
            expect("Daemon")
            assert scalar("SELECT COUNT(*) FROM samples")==0,"History should render before the first SQLite flush"
            keys(b"]]]]")
            expect("24 hours")
            keys(b"c")
            expect("Paused")
            expect(" @ ")
            keys(b"hf")
            expect("Live")
            before = sum(server.calls.values())
            drain(4)
            assert sum(server.calls.values())-before <= 6, "UI started duplicate stats polling"
            observed=max(sample["at_ms"] for sample in live_rows(int(time.time()*1000)-45_000))
            daemon.send_signal(signal.SIGTERM)
            daemon.wait(timeout=5)
            assert daemon.returncode == 0, daemon.stderr.read().decode()
            assert scalar("SELECT MAX(at) FROM samples")>=observed,"Shutdown failed to flush live ticks"
            expect("local stats sampling resumed")
            before=sum(server.calls.values())
            drain(3)
            assert sum(server.calls.values())>before, "Local collection did not resume after daemon exit"
            daemon=launch()
            expect("Connected to daemon metrics")
            before=sum(server.calls.values())
            drain(3)
            assert sum(server.calls.values())-before<=5, "Daemon handoff left a local collector running"
            keys(b"\x03")
            ui.wait(timeout=5)
            assert ui.returncode == 0 and termios.tcgetattr(slave) == original
            daemon.send_signal(signal.SIGTERM)
            daemon.wait(timeout=5)
            assert daemon.returncode == 0, daemon.stderr.read().decode()
            retained = scalar("SELECT COUNT(*) FROM samples")
            with sqlite3.connect(database) as db:
                db.execute("INSERT INTO samples SELECT container,at-90000000,cpu,memory,mem_limit,rx,tx,dread,dwrite,rx_rate,tx_rate,read_rate,write_rate,flags FROM samples ORDER BY at LIMIT 1")
            daemon = launch()
            wait(lambda: scalar("SELECT COUNT(*) FROM samples WHERE at < CAST(strftime('%s','now') AS INTEGER)*1000-86400000") == 0, "Startup expiry did not delete old readings")
            assert scalar("SELECT COUNT(*) FROM samples") >= retained
            # Service lifecycle is routed only to fixture scripts in a private HOME.
            for action in ["install", "start", "status", "stop", "uninstall"]:
                result = subprocess.run([str(binary), "--config", str(config), "service", action], capture_output=True, env=env, timeout=5)
                assert result.returncode == 0, result.stderr.decode()
            assert database.exists(), "Service uninstall removed history"
            print("PASS: read-only daemon, 15-second batched SQLite writes, live History before first flush, startup backfill, shutdown flush, persistent CPU/RAM/I/O, History ranges/cursor/zoom, no duplicate UI stats, live daemon/local handoff, startup expiry, restart, fake user-service lifecycle, terminal restoration")
        finally:
            for process in [ui, daemon]:
                if process is not None and process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
            if master is not None:
                os.close(master)
                os.close(slave)
            server.shutdown()
            server.server_close()


if __name__ == "__main__":
    main()
