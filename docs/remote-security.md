# Remote security scanning

## Enable on a server

The server needs Linux, systemd, the Docker CLI, and a running Docker-compatible
engine. Install/reconfigure as root, using your dedicated public SSH key and an
**independently reviewed manifest/index digest** for `docker.io/anchore/grype:latest`:

```sh
sudo tha-agent setup --user tha --key 'ssh-ed25519 AAAA...' \
  --scanner-digest sha256:APPROVED_MANIFEST_DIGEST
```

The same `--scanner-digest` option works with the agent installer. It enables
Security only on that server. Do not use Docker's local image ID as the approval:
the registry manifest/index digest is a different identity.

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
scanner_digest = "sha256:APPROVED_MANIFEST_DIGEST"
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

- Every minute, discover unique images referenced by running or stopped containers.
- Scan new images and rescan completed images after the configured interval
  (default 24 hours). Unused images can be requested manually from Tha.
- One job runs at a time across all engines on a server. Pending jobs are deduplicated.
- Defer jobs when one-minute load exceeds 70% of available cores or `MemAvailable`
  is below 1536 MiB. Unknown pressure also defers. Check again every 30 seconds.
- Scanner and database-update containers default to half a CPU, 768 MiB RAM,
  no extra swap, 128 processes, and low CPU/I/O weights. Engine/cgroup support is
  required; unsupported hard limits fail the job rather than silently dropping them.
- The agent service itself runs with low CPU/I/O scheduling priority.
- Failed scheduled scans retry after an hour. Previous successful reports remain.

Admission checks do not predict future application load. Hard container limits
remain in force during a scan. Image export still causes host/engine disk I/O and
uses temporary disk space; container limits do not fully govern engine-side export.
Very large images can need higher memory limits; OOM is a failure, never a clean scan.

## Scanner verification

Before every scan, the agent contacts the fixed Docker Hub endpoints over HTTPS,
hashes the `latest` manifest bytes, and compares them to the approved digest.
A mismatch or failed verification prevents both image-layer pulling and execution.
For a multi-platform index, the selected platform manifest must hash to the digest
in that approved index. Pulls use that immutable digest, not `latest`; execution
uses the verified config/image ID with `--pull=never`.

Approving a digest is the trust decision; matching a hash is not publisher signature
verification. When Anchore changes `latest`, new scans stop until an administrator
reviews and approves its new digest. Saved reports remain readable.

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
