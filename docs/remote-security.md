# Remote security scanning

## Enable on a server

The server needs Linux, systemd, the Docker CLI, and a running Docker-compatible
engine. Normal agent setup enables scheduled scans and automatically verifies the
official Grype image. No manual SHA values or registry configuration are required:

```sh
sudo tha-agent setup --user tha --key 'ssh-ed25519 AAAA...'
# Or use your existing root SSH key:
sudo tha-agent setup --user root
```

Both forms also work with the agent installer. Use `--no-security` to opt out.
The main THA installation still needs its optional Security module to display and
synchronize reports.

Setup installs/enables `tha-agent.service`. Existing installations must rerun
setup once to migrate dedicated-user socket permissions. Subsequent installer
updates restart the service and preserve policy and reports.

```sh
sudo systemctl status tha-agent
sudo journalctl -u tha-agent
```

The root-owned policy is `/etc/tha/agent.toml`. Example security section:

```toml
[security]
enabled = true
interval_seconds = 86400
max_load_percent = 70
min_available_memory_mb = 1536

[security.limits]
cpu_millis = 500
memory_mb = 768
```

Restart the agent after changing policy. Set `enabled = false` and restart to
disable scanning without deleting reports or the scanner/database cache.

## Scheduling and headroom

- Automatic scans are off by default; scans run only when requested from Tha.
- To turn them on, open Security in Tha, select the server's engine and press **t**
  (or `:scan-schedule`). Enter a standard five-field cron schedule, for example
  `0 0 * * *` for every day at 00:00, or leave it empty to turn scans off. Times use
  the server's time zone. Each engine has its own schedule, stored by the agent.
- With a schedule, the agent checks every minute for unique images referenced by
  running or stopped containers. It scans images without a report right away and
  rescans all of them at each scheduled start. A start missed during downtime runs
  once when the agent is back. Unused images can be requested manually from Tha.
- Refresh the vulnerability database with the first scan after midnight.
- One job runs at a time across all engines on a server. Pending jobs are deduplicated.
- Defer jobs when one-minute load exceeds 70% of available cores or `MemAvailable`
  is below 1536 MiB. Unknown pressure also defers. Check again every 30 seconds.
  **Ignore CPU and RAM thresholds** in the schedule dialog skips this check for every
  scan on that engine, manual ones included. It is off by default, and the scanner's
  hard limits below still apply.
- Scanner and database-update containers default to half a CPU, 768 MiB RAM,
  no extra swap, 128 processes, and a low CPU weight. An optional container I/O
  weight is not requested: some engines do not expose the cgroup `io.weight` file.
  Engine/cgroup support is required for the remaining hard limits; unsupported
  limits fail the job rather than silently dropping them.
- The agent service itself runs with low CPU/I/O scheduling priority.
- While a schedule is on, failed scans retry after an hour. Previous successful
  reports remain.

Admission checks do not predict future application load. Hard container limits
remain in force during a scan. Image export still causes host/engine disk I/O and
uses temporary disk space; container limits do not fully govern engine-side export.
Very large images can need higher memory limits; OOM is a failure, never a clean scan.

## Scanner verification

Before every scan, the agent contacts the fixed official
`docker.io/anchore/grype:latest` repository over authenticated HTTPS, obtains its
manifest digest, and hashes the actual manifest bytes to verify that digest.
A mismatch or failed verification prevents both image-layer pulling and execution.
For a multi-platform index, the selected platform manifest must hash to the digest
in that verified index. Pulls use that immutable digest, not `latest`; execution
uses the verified config/image ID with `--pull=never`.

Cached local images are checked against the official config/image ID, not trusted
because their tag says `anchore/grype`. New official releases are resolved
automatically. Failed TLS, missing/mismatching digests or image-ID mismatches stop
the job rather than falling back to an unchecked image.

The trust source is the official Docker Hub repository and HTTPS certificate
verification. This is source/integrity verification, not malware detection or
container publisher-signature verification; it cannot guarantee an upstream release
is free of vulnerabilities or malicious code. Existing optional `scanner_digest`
pins remain an additional restriction; rerunning normal setup removes the need for
the old manual pin.

The agent constructs all scanner arguments. Clients cannot choose image names,
commands, mounts, privileges or limits. General Docker create/pull/export APIs stay
blocked. The scanner receives an exported image archive, never the Docker socket,
and scans with networking disabled. Database updates use a separate restricted
network-enabled container. Target images are never executed.

## Central reports

Enable the Security module on the main THA installation. Reports synchronize over
the existing authenticated SSH connection, without transferring image layers.
With `connect = "startup"`, the main THA service retrieves reports every minute even
when the UI is closed. Manual servers synchronize while connected in the UI.
Passphrase-protected keys must be available to the service through its SSH agent.

Reports are stored by server endpoint, runtime identity, and immutable image ID in
`$XDG_DATA_HOME/tha/modules/security/reports.sqlite3`. The server also retains its
reports under `/var/lib/tha-agent/tha/modules/security/`. Both stores retain up to
10,000 latest image reports with a 1 GiB SQLite page budget (not a filesystem quota).
Scanner digest, scanner version, database metadata and scan timestamp are retained.
Browsing reports never runs a scan. Refreshing the vulnerability database does not
retroactively reevaluate an existing report; the scheduled rescan does that.

Jobs survive SSH/UI disconnects. Cancelling remote following stops the UI queue,
not the agent's scan schedule. A service restart stops active work; pending manual
jobs are not crash-durable, while referenced images are rediscovered. Scans time
out after 30 minutes once admitted; waiting for headroom has no deadline.

Reports cover image contents, not writable-container changes or mounted volumes.
An authenticated compromised server can still lie about its contents or findings.
