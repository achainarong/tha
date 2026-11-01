# Tha user guide

A runtime-first container terminal UI, written in Rust. **Tha** is the product;
**`tha`** is the executable. The name is Thai for a pier or port.

Explore **engine → Compose project → service → container**, with networks and
volumes next to their workloads. Native containerd adds a namespace level.

## Quick install

On Linux (amd64, arm64) or macOS (Apple Silicon), paste one of these into a terminal.
No Rust installation or sudo is needed.

**Standalone** (default) installs only the `tha` binary. Metrics are sampled while
the UI is open; nothing runs in the background:

```sh
curl -fsSL https://raw.githubusercontent.com/achainarong/tha/main/scripts/install.sh | sh
```

**Service** also installs and starts the per-user
[background collector](#cpu-and-memory) (launchd/systemd, no sudo), so History
keeps 24 hours of metrics even when the UI is closed:

```sh
curl -fsSL https://raw.githubusercontent.com/achainarong/tha/main/scripts/install.sh | sh -s -- service
```

**Service + Security** additionally sets up the
[security module](#optional-security-module) (Grype image vulnerability scanning):

Requires the Docker CLI and a **running Docker-compatible container engine** during
setup and scanning. Colima is optional; native Docker Engine works on Linux.

```sh
curl -fsSL https://raw.githubusercontent.com/achainarong/tha/main/scripts/install.sh | MODULES=security sh
```

**Agent** is for a server you manage from Tha on another machine. It installs only
`tha-agent`, with no UI, as a root-owned binary and persistent systemd service.
Linux only, and it does need root; see [Remote servers](#remote-servers):

There are two machines here: **your computer**, where you run `tha`, and **the server** you
want to manage.

**On your computer**, create the SSH key that `tha` will log in with, and print its public
part. The private key never leaves your computer:

```sh
ssh-keygen -t ed25519 -f ~/.ssh/tha-agent
cat ~/.ssh/tha-agent.pub
```

**On the server**, run this one command with the public key you just printed. It installs
the agent and sets the server up. The form under `:servers` shows it with your key filled in:

```sh
curl -fsSL https://raw.githubusercontent.com/achainarong/tha/main/scripts/install.sh | sudo sh -s -- agent --key 'ssh-ed25519 AAAA...'
```

This creates the dedicated `tha` user for you, so do not create it first, and exposes every
Docker and Podman socket it finds. One it cannot reach is skipped with a message. Add `--user NAME` for another user name, or
`--allow-delete` to let this server delete containers, images and volumes.

**Permissions:** you only need root or `sudo` to run it. You do not need to give `tha` any
permissions yourself. Setup adds it to the group that owns the engine's socket, usually
`docker`, and it gets nothing else: no usable password, no sudo, and a key that can only
run the agent. Do not add sudo rules or other keys, because they would bypass the agent.
If only the socket's owner can use it, as with most rootless engines, setup stops and
explains why instead of widening access.

To use an existing root key instead, with no new user or key:

```sh
curl -fsSL https://raw.githubusercontent.com/achainarong/tha/main/scripts/install.sh | sudo sh -s -- agent --user root
```

To update the agent later, run the install again with just `agent`; setup stays in place.

See [Remote servers](#remote-servers) for what each one means.

The [script](../scripts/install.sh) downloads the release archive and `SHA256SUMS`,
verifies the checksum, and installs `tha` into `~/.local/bin`. It needs `curl` or
`wget`, `tar`, and `sha256sum` (Linux) or `shasum` (macOS). Optional settings:

| Variable | Effect |
| --- | --- |
| `THA_MODE=service` | Same as passing `service` (or `agent`) |
| `THA_VERSION=v0.6.0` | Install that release instead of the latest (pin or roll back) |
| `THA_BIN_DIR=...` | Install somewhere other than `~/.local/bin` (agent: `/usr/local/bin`) |
| `THA_INSTALL_LOG_DIR=...` | Override the installer debug log directory (default `$XDG_STATE_HOME/tha/install`, or `~/.local/state/tha/install`) |
| `MODULES=security` | Install the Security service module; implies service mode; requires Docker CLI and a running Docker-compatible Unix endpoint |

Re-running the script updates in place. A running collector is stopped before the
binary is replaced and started again afterwards, in either mode. Configuration and
history are never touched.

The installer displays progress and prints the path to a private debug log on
success or failure. Each run keeps its own log with timestamps, command exit codes,
and subprocess output (including Security setup). Review logs before sharing:
diagnostics may contain local paths or runtime details. There is no environment
dump or shell tracing. Logs are kept until you remove them.

Security database downloads display real transferred/total counts, for example
`12.0 MB / 180.0 MB`, followed by validation/unpacking status. If the scanner does
not report byte counts, setup still reports elapsed activity rather than guessing
the download size or percentage.

### Optional security module

This section describes local scans. For scheduled server-side scans and centrally
saved results, see [remote security](remote-security.md).

Start your Docker-compatible container engine before installing or scanning. Setup
pulls the scanner image and runs it to initialize the vulnerability database; Tha
does not start the engine for you. Installation prints each setup stage and elapsed
time every 10 seconds while pulling the scanner or updating its database.

```sh
# MODULES belongs on the shell side of the pipe, not on curl.
curl -fsSL https://raw.githubusercontent.com/achainarong/tha/main/scripts/install.sh | MODULES="security" sh
# Or add it to an existing service installation:
tha service install
tha service start
tha module install security
tha module disable security
tha module enable security
tha module status
```

`MODULES` is a comma-separated list; currently `security` is the only module.
Modules extend **service mode only**. `MODULES=security` chooses service mode when no
mode was specified; combining it with explicit `standalone` is rejected before any
download. `tha --standalone` keeps the existing standalone feature set and hides Security.
Enable/disable takes effect in the running service and UI without restarting them.
Disabling cancels the active security queue and removes its scanner containers, but
keeps the image cache, database volume and saved reports. Re-enabling does not download
resources again until the next scan needs an update.
Unknown names fail before the installer downloads or replaces Tha. Omitting it
does not install security or remove modules already installed. The installed Tha
release must include module support; a source build can use `cargo run -- module install security`.

Setup pulls the official `anchore/grype:latest` scanner and initializes its database
(approximately 196 MB compressed at time of writing, plus extracted disk usage).
`DOCKER_HOST` selects the setup engine; otherwise the current Docker context is used.
Setup does not start an engine. A setup failure leaves Tha installed and prints a retry command.
The installer starts the per-user service before provisioning the module. Manual
`tha module install security` requires that service to be running. The Docker CLI's
absolute path is recorded during setup so launchd/systemd need not inherit your shell's PATH.

When enabled, **Security is a fourth top-level view**, alongside Containers, Images
and Volumes. **Ctrl+Left/Right** (or F7/F8) cycles all four; **Alt+4** opens Security
directly. Without the module, only the original three views are shown.

In **Security** (or **Images**, `:images`), press **m** to check individual images or **M** to check
all visible images, then **`:scan-images`**. Checks are separate from the existing
Space/`u` cleanup marks; scanning works on used and unused images. The queue includes
checked images still present in that runtime, even if a filter hides them. With no
checks, it scans the selected image. A vanished checked image is skipped; no image is
pulled as a replacement. Press **8** or **`:security`** on an image to browse its saved
report, then **`:scan-image`** to scan/rescan it (clear checks first for just that image).
**Tab** switches between Security's
image list and findings; **Enter** on an image focuses its findings. A single container
shows its image's saved report in its own **8 Security** tab, without leaving Containers. That tab
is read-only: **j/k**, **Enter** and **z** browse findings, and scans only start from the Security
view or Images. Merely opening either never scans or downloads anything.
Within Security, **s** or **S** scans checked images (or the selected image if none are checked),
and **x** cancels the service's active queue. These shortcuts never delete images.
**Space** also toggles scan checks in Security; **c** clears scan checks without
changing cleanup marks in Images. Findings use a responsive, scrollable table;
narrow terminals show fewer columns, with all fields available under Enter.
Starting a scan keeps keyboard focus where it was; use **Enter** or **Tab** to focus
findings. The active pane has a thick, theme-colored border (bold in monochrome).
Queues continue after individual image errors and summarize completed/failed images;
cancellation still stops the remaining queue. A failed image is never a clean report.
Saved CVEs remain visible and browsable during scanning; background scans do not
change the selected image. The bottom status strip shows the service's current stage,
including pulling `anchore/grype:latest` again if the scanner image is missing (for
example after pruning images) and refreshing the vulnerability database, plus an
animated activity indicator, elapsed queue time, and **x** to cancel. The animation
indicates UI activity, not measured scan progress. Findings refresh when the selected
image finishes scanning; previously saved results are explicitly labeled as saved reports.

Security findings are sorted Critical → High → Medium → Low → Negligible → Unknown,
then by descending highest reported CVSS base score. Missing scores are shown as `—`,
not zero. **j/k** selects findings; **Enter** opens package versions, available fixes,
description and advisory URL; **Esc** returns. **z** expands/restores Security across
the terminal. Images also supports **z** to hide the application header and focus
expanded image details. **`:cancel-scan`** cancels the queue.

Scans run sequentially in the **selected image's runtime endpoint**, not whichever
Docker context happens to be active. The module exports the immutable local image
to a private temporary archive (10 GiB maximum), copies it into an ephemeral Grype
container, and reads JSON findings. It never mounts the engine socket into Grype,
executes the target image, or uploads the package inventory. The scan container has
no network; a separate ephemeral database-update container has network access.
The scanner containers are removed after use; the named volume
`tha-security-grype-db` persists **per runtime**. Failed/cancelled scans do not become
clean reports. Completed reports survive later queue errors and application restarts
in `$XDG_DATA_HOME/tha/modules/security/reports.sqlite3` (default
`~/.local/share/tha/modules/security/reports.sqlite3`), separate from metrics history.
The latest report per runtime/image is retained, up to 10,000 images and a 1 GiB database budget.
The service owns scan queues: closing Tha only disconnects the UI, not the job.
Reopen an image in Security to load its completed report. `:cancel-scan` sends an
explicit cancellation request, including for a queue started by a previous UI session.
An application/service restart cancels in-progress work; queued jobs are not crash-durable.

Opting into the module permits scanner pulls and database refreshes, checked at most
once per day **on scan use**, not by an always-running container or background scheduler.
The actual scanner image ID is pinned for each scan. After a successful update, Tha
removes only the previously tracked Grype image, without force or parent pruning.
If that old image is still referenced, the queue reports an error rather than force
deleting it. **`:update-security-db`** forces an update and rescan of the selected image.
Docker-compatible Unix endpoints are supported initially; native containerd is not.
Podman compatibility depends on its Docker API and the Docker CLI and must be verified
for the installed version.

Reports describe the image, not running-container changes or mounted files. Old saved
reports are not automatically reevaluated after database updates. Zero findings is
not a guarantee of security; package detection and advisory coverage have limits.

Then run `~/.local/bin/tha`, or add `~/.local/bin` to your `PATH`:

```sh
export PATH="$HOME/.local/bin:$PATH"
tha
```

To switch an existing install from `service` to `standalone`, run
`tha service uninstall` (history and configuration are kept). To switch from
`standalone` to `service`, re-run the installer with `service`, or run
`tha service install && tha service start`.

Linux archives are statically linked with musl, avoiding a dependency on the
build runner's glibc version. ARM means **64-bit ARM**, not ARMv7. The macOS binary
is ad-hoc signed, not Apple-notarized; Intel Macs are not included. Checksums
detect corrupted/mismatched downloads, not an independently authenticated
publisher. Only install releases from the repository you trust.

Releases are listed on [GitHub Releases](https://github.com/achainarong/tha/releases).
No runtime state or user configuration is migrated by installation.

## Build from source

Requires macOS or Linux, Rust 1.88+ (current stable recommended), and an
interactive terminal. No Docker CLI is required for Docker API browsing;
interactive exec and `:pull-image` require an installed runtime CLI, and the
`:compose-*` actions also require [Compose](#compose-update-actions).

```sh
make demo       # Try the UI without contacting any runtime
make run        # Discover and browse your actual engines
make install                # Standalone: install tha into ~/.cargo/bin (or your Cargo root)
make install MODE=service   # Also install and start the per-user background collector
make install:full           # Service + security; requires Docker CLI and a running engine
make install:agent           # Only tha-agent, for a server; uses sudo
tha
```

Ensure Cargo's bin directory is on `PATH`. Installation does not require sudo.
If both install locations are on `PATH`, `command -v tha` shows which takes precedence.
Tha never starts an engine, VM, or workload container automatically. Enabled service
modules may run their documented helper containers after explicit setup/scan requests.

## First MVP

- Concurrent runtime discovery and refresh; a failed engine does not block others.
- Runtime-first explorer with label-based Compose grouping and standalone containers.
- Project/service running-versus-total container counts and distinct service counts.
- One-second CPU/memory sampling, explorer usage columns, CPU sparklines, and memory bars.
- Compose-focused workspaces, back navigation, and current-directory project matching.
- Network/volume ownership and attachment lists; shared resources are marked.
- Container overview: image platform (ARM/AMD), health/state, ports, networks, and named volumes.
- Live logs for containers, services, or projects, with source labels.
- Bounded log history, highlighted search, wrapping toggle, scrollbar, and retained-row position.
- Vim navigation, command palette, scrollable shortcut cheatsheet, and narrow-terminal layout.
- Terminal-native foreground/background and ANSI palette; no hardcoded RGB theme.
- Embedded PTY container exec and confirmation-backed Docker/Podman lifecycle actions.

### Runtime coverage

| Runtime | MVP connection | Notes |
| --- | --- | --- |
| Docker Engine | Docker API over Unix socket | Local and rootless endpoints |
| Docker Desktop | Docker API over Unix socket | Contexts and macOS socket discovery |
| OrbStack | Docker API over Unix socket | Context and macOS socket discovery |
| Docker-backed Colima | Docker API over Unix socket | Contexts and profile sockets |
| Podman | Docker-compatible API socket | Native machine discovery plus rootless/rootful local sockets; machine/API service must already be running |
| Native containerd / nerdctl | `nerdctl` subprocess adapter | Requires `nerdctl` on PATH and an accessible containerd socket |

Containerd namespaces are shown separately. Its network/volume inventory is
**not implemented**; logs depend on nerdctl's logging configuration. Kubernetes
workloads may not expose Docker-shaped inspect data and remain inspection-only.
On macOS, a VM's containerd guest socket is not automatically reachable from the
host. Guest/VM transports are not implemented in this MVP.

SSH, TLS, and TCP Docker contexts remain visible but are marked unsupported.
Tha does not silently turn secure connections into plaintext connections. To
reach another machine over SSH, see [Remote servers](#remote-servers).

## Connections

Tha reads Docker context metadata, `DOCKER_HOST`, Colima profiles, and known
Docker/OrbStack/Podman/containerd sockets. Rootless discovery also checks
`XDG_RUNTIME_DIR`. `DOCKER_CONFIG` changes the Docker configuration location.
All discovered contexts are considered, not just the active one; Tha never
changes your active context.

### Podman machines

With `podman` on PATH, Tha asynchronously reads `podman machine list --format json`
and inspects running machines for `ConnectionInfo.PodmanSocket.Path`. Each machine
appears as **Podman / machine-name** without a Docker context, Podman Desktop, or
privileged helper. Multiple machines are supported. Missing Podman and empty
machine lists are optional-provider states, not application failures.

Discovery refreshes with inventory and `:refresh`. Stopped/disappeared machines
remain visible as `(offline)` with last-known data; Tha does not connect to a
stopped machine. Restarting or changing a forwarded socket replaces that machine's
inventory/stats workers without changing its navigation identity. Old worker
responses are rejected, logs resubscribe to the new socket, and an existing Exec
session is never redirected. Docker-context aliases sharing an engine ID or a
canonical Unix socket do not duplicate workload trees or metrics.

Machine commands have a 3-second deadline, a 1 MiB output limit, and at most four
concurrent inspections. Invalid JSON, inspect failures, and API failures remain
local warnings; other runtimes keep refreshing. Inventory uses the negotiated
Docker-compatible API. Container images are listed with tags, IDs, and reported
sizes at the bottom of runtime Overview (Tab into details and scroll). Image
listing failures do not hide containers. Compose grouping recognizes both
`com.docker.compose.*` and `io.podman.compose.*` labels.

Tha starts/stops/restarts machines only after explicit runtime confirmation. It
never changes rootless/rootful settings, installs helpers, or changes the default
Docker/Podman connection. Live compatibility is version-
specific; the fixture checks below cover the installed macOS Podman machine,
not every Podman version or remote transport.

Explicit connections are useful for endpoints discovery cannot find:

```sh
tha --connection 'OrbStack=unix:///Users/you/.orbstack/run/docker.sock'
tha --connection 'Podman=unix:///run/user/1000/podman/podman.sock'
tha --containerd 'containerd=/run/containerd/containerd.sock'
tha --list-connections
tha --monochrome
```

Repeat connection flags to add multiple engines. Canonical socket paths are
deduplicated. Docker endpoints reporting the same engine ID are shown as aliases
without duplicate workload trees. Native containerd has endpoint-level deduplication.

Configuration lives at `$XDG_CONFIG_HOME/tha/config.toml`, falling back to
`~/.config/tha/config.toml`. Use `--config PATH` to override. See
[`tha.example.toml`](../tha.example.toml). Explicit connections supplement discovery.

## Remote servers

Manage containers on other machines next to your local ones. Each server gets its
own SSH key and runs `tha-agent`, an allowlist in front of its Docker socket. The
allowlist runs on the server, so with a dedicated user it holds even for someone who
has stolen the key and does not use Tha.

**Add a server:** press `.` or type `:servers`, then `a`. Enter the host, user and
key path, or create a key from the form. It shows the one command to run on the
server, as an admin. It installs `tha-agent` and sets the server up:

```sh
curl -fsSL https://raw.githubusercontent.com/achainarong/tha/main/scripts/install.sh | sudo sh -s -- agent --key 'ssh-ed25519 AAAA...'
```

`tha-agent` is all the server needs; the full `tha` stays on your own machine.
The persistent systemd service owns the allowlist and optional scan schedules. SSH
relays requests to root-owned Unix sockets; there is no additional network listener.
Update agents along with `tha`. See [remote security](remote-security.md) for
automatic scanner verification, resource limits, scheduling and central report synchronization.

Setup creates the dedicated user, writes the root-owned policy `/etc/tha/agent.toml`,
and restricts the key in `authorized_keys` to the agent. It stops if that user already
has a key that could bypass the agent. Do not give the agent user any other key,
password or sudo rule.

Then use **Test connection**: OpenSSH shows the host key fingerprint for you to
confirm, and Tha never accepts a host key itself.

**Use a dedicated user instead of root:** you do not create the user yourself; setup does.

1. On your computer, not the server, create a key just for this server, or use **Create new
   key** in the form:

   ```sh
   ssh-keygen -t ed25519 -f ~/.ssh/tha-agent
   cat ~/.ssh/tha-agent.pub
   ```

2. On the server, as an admin, run the install with that public key:

   ```sh
   curl -fsSL https://raw.githubusercontent.com/achainarong/tha/main/scripts/install.sh | sudo sh -s -- agent --key 'ssh-ed25519 AAAA...'
   ```

   This creates `tha` with no usable password, gives it access to the engine's socket, and
   installs the key so that it can only run the agent. If the server is already set up for
   root, this adds the user and keeps the existing policy.

3. In the Servers form, set **User** to `tha` and **SSH key** to `~/.ssh/tha-agent`.

To check the limit, run `ssh -i ~/.ssh/tha-agent tha@SERVER id`. It should refuse with
`this key only runs the tha-agent` instead of printing the user. An existing user also
works if it has no other SSH keys, but setup stops if any are there.

You can also connect as `root` with an existing key. Setup writes the policy and enables the service:

```sh
curl -fsSL https://raw.githubusercontent.com/achainarong/tha/main/scripts/install.sh | sudo sh -s -- agent --user root
```

`tha`'s own traffic still goes through the allowlist, but the key is not restricted:
anyone holding it has full root access on the server. Use a dedicated user if the limit
must hold for a stolen key.

If Test connection says it cannot read `/etc/tha/agent.toml`, the agent is installed but
setup has not run yet.

Servers can also be written by hand as `[[remotes]]` in `config.toml`; see
`tha.example.toml`. Those show as read-only in the modal. Servers added in the modal
are saved in `remotes.toml` next to it, mode `0600`. Only the path to each key is
stored; Tha never reads or copies a private key.

**Keys with a passphrase:** when you test or connect and the key is locked, OpenSSH
asks for the passphrase in the embedded terminal, so Tha never sees it. You are asked
once: `ssh-agent` then holds the unlocked key, whether or not the server has been saved,
and servers sharing the key do not ask again. Tha uses your `ssh-agent` if one is
running, and the key stays loaded there after Tha closes, like any other `ssh-add`.
Otherwise Tha starts a private agent that forgets the key when Tha closes, so you are
asked once per run.

A key that is gone from the agent during a background reconnect never interrupts you.
The server shows that its key is locked until you press Enter on it. The background
service cannot ask at all, so a server it connects to needs the key already loaded in an
agent it can reach.

Login is by SSH key only. Tha never asks for or stores a password, and tells `ssh` not
to use one, even if the server allows it. That is a client setting: to turn password
login off for everyone, set `PasswordAuthentication no` on the server.

**What the agent allows:** listing and inspecting containers, images, networks and
volumes, image history, logs, stats, events, and start, stop and restart. Everything
else is refused, including general create, exec, pull, build, file copy, prune and image
export. Security endpoints can request only agent-constructed, verified scanner jobs
(enabled by setup unless `--no-security` is specified).
Deleting containers, images and volumes is off unless setup ran with
`--allow-delete`, and force is always refused. Writes and refusals go to the server's
system log.

**Not available on servers:** exec, image pull, image update checks, Compose actions,
background metrics history, and starting or stopping the engine itself.

**What it does not protect:** reading is still sensitive, because container inspect
includes environment variables and logs can contain secrets. A stolen key can also
stop every container. Removing a server from Tha leaves its key valid until you remove
it from that user's `authorized_keys`.

**Machines in the Explorer:** once a server exists, engines group under their machine.
`Enter` on a machine shows only that machine, and connects it if needed. `Backspace`
returns, and `:all` shows everything. `Shift+C` connects or disconnects the selected
server. Without servers, the Explorer does not change.

**Traffic:** "Connect to servers" in Settings is **Manual** by default, so nothing is
sent until you connect; a server's `connect` setting overrides it. With Security enabled,
the main service also connects to startup-enabled servers to retrieve saved reports.
Manual servers are synchronized only while connected in the UI. UI stats refresh
every 5 seconds unless `metrics_refresh_seconds` says otherwise. A refused key or
changed host key stops and waits for you instead of retrying.

Requires OpenSSH on your machine, and Linux on the server. Only Docker and
Docker-compatible engines are supported through the agent. A rootless engine's socket
usually cannot be shared with a separate user, so setup explains the problem and stops.

To try it against a disposable server, with Docker installed:

```sh
tha --demo --demo-servers              # Shows sample servers; contacts nothing
python3 scripts/remote_smoke.py --try  # Keeps a test server running until you press Enter
python3 scripts/remote_smoke.py        # Runs the automated end-to-end checks
```

## Compose workspaces

Press `Enter` on a Compose project to focus its services, containers, networks,
volumes, and logs. The breadcrumb shows the selected engine/project. Shared
resources remain marked; attachments belonging to other projects are counted
but their workload names are hidden. `Backspace` returns to your previous view
and restores its selection and expanded branches. `Esc` first clears search or
returns from details, then goes back. `:all` returns directly to all runtimes.

```sh
tha             # Automatically open a project matching this directory
tha --overview  # Always start with all runtimes
```

Auto-opening uses Compose working-directory labels, including when launched in
a subdirectory. It never guesses from the directory basename or runs Compose.
Nested projects prefer the most specific matching directory. Multiple matching
connections open a picker. No metadata match leaves the full overview visible.
Startup matching waits for each connection's first result (including failures);
any keyboard interaction cancels auto-opening so a late engine cannot interrupt
your navigation. Demo mode never auto-opens a project.

## Keyboard

| Key | Action |
| --- | --- |
| `j` / `k`, up / down | Move or scroll the focused pane |
| `h` / `l`, left / right | Collapse/parent or expand/child in the tree; previous/next tab in focused details; horizontal scroll in unwrapped logs; pan in Metrics |
| `H` / `L` | Previous/next tab from any focused detail tab, including Metrics and unwrapped logs |
| Mouse | Click selects and focuses, click again opens like `Enter`; click an arrow to expand or collapse; click tabs and overviews; wheel scrolls the pane under the pointer; drag the log scrollbar; right-click goes back like `Esc`. Confirmations stay keyboard-only. Hold Shift to select terminal text, or set `mouse = false` |
| `,` / `:settings` | Settings pane: theme, language, keyboard layout, mouse, log wrapping |
| `Space` | Toggle the selected explorer branch open or closed |
| `Enter` | Enter a Compose workspace; otherwise open details; on a focused Networks row, select that container in the Explorer |
| `Backspace` | Return to the previous workspace |
| `gg` / `G` | First / last; `G` resumes log following |
| `Ctrl-u` / `Ctrl-d` | Half-page up / down |
| `Tab` / `Shift-Tab` | Cycle Explorer / Performance (when open) / Details |
| `z` | Expand/restore the focused pane to full width: Explorer, detail tab, or inventory view; `Esc` also restores |
| `p` / `:performance` | Expand/collapse bottom-left global Performance |
| `P` (Shift-p) | Switch Runtime / Selected container Performance; open panel |
| `z` (Performance focused) | Toggle compact / full-width bottom Performance |
| `Esc` (Performance expanded) | Restore compact view first |
| `o` / `:sort-contexts` | Choose connected-first, name, CPU, or memory context order |
| `S` / `s` / `r` / `d` in Explorer | Start / Stop / Restart / Delete selected container or Compose workload |
| `1` … `8` | Overview / Logs / Networks / Exec / Events / Metrics / Layers / Security on the right; a tab is hidden when the selection cannot use it, and Exec needs a running container, a running service, or an open session |
| `a` / `:actions` | Contextual lifecycle menu with eligibility counts and disabled reasons |
| `F3` in Logs / `:filter-logs` | Hide retained logs unless the message contains every word; Tab switches to the advanced JSON filter |
| `:filter-logs-advanced` | Edit the advanced JSON filter directly |
| `:diagnose` or `a` | Why is this unhealthy? Read-only evidence for the selected container |
| `:network-diagnose` or `a` | Passive network evidence and peer comparison for the selected container; sends no probes |
| `:clear-log-filter` | Clear text and advanced filters without clearing highlighted search |
| `:export-logs` | Export matching retained entries to an explicitly chosen new JSONL file |
| `/` | Search tree or highlight log matches when the log pane is focused |
| `n` / `N` | Next / previous match; logs navigate individual occurrences |
| `f` | Toggle follow in the focused log pane |
| `w` | Toggle wrapping immediately in the focused log pane |
| `5` / `:events` | Timeline of container starts, exits, OOM kills and health changes for the selection |
| `a`, then a Compose entry, or `:compose-update` | Pull newer images and apply them to the selected project or service; also `:compose-pull`, `:compose-up`, `:compose-recreate`, `:compose-down`, `:compose-output` |
| `J` / `:json` | Pretty-print JSON log entries; press again for the original text |
| `Ctrl-R` while searching logs | Switch between literal and regex search |
| `:logs-since 15m` | Load logs from a start time (`30s`, `2h`, `1d`, RFC 3339); `off` returns to recent lines |
| `U` / `:check-updates` in Images | Compare visible tagged images with their registries |
| `:pull-image` in Images | Pull the selected image's tags; running containers are unchanged |
| `e` / `:exec` | Open an interactive shell in a selected container/service |
| `Enter` in Exec | Focus the existing shell, or start one if no session exists |
| `Ctrl-]` in Exec | Return keyboard focus to Tha without closing the shell |
| `:close-exec` | Close the PTY/CLI session, not the container |
| `F6` / `:exec-sessions` in navigation | Choose a retained shell; Enter resumes its input |
| `:` | Command palette; `Tab` completes a command |
| `?` / `F1` / `:help` | Open the shortcut cheatsheet; `j`/`k` scroll it |
| `Esc` | Cancel input, clear search, return to explorer, or go back |
| `q` | Return through panes/workspaces; quit from the full overview |
| `Ctrl-c` | Quit Tha; during exec, the shell owns input |

Search is applied with `Enter`; `Esc` cancels input. Navigation shortcuts do not
fire while typing. Toggling a search-expanded branch with `Space` clears the tree
filter and collapses that branch, keeping it selected. Shortcuts can be customized
in the config's `[bindings]` table; see [keybindings](keybindings.md).

### Events

**5** or **`:events`** lists what happened to the selected container, service, project
or runtime, newest first: created, started, restarted, signalled, stopped, exited (with its
exit code), out of memory, health changes and removal. Project and service rows keep
containers that have since been recreated or removed. The Metrics tab marks the same
events on its charts with `┊`, colored by severity, so a memory climb can be read against
the OOM kill that ended it.

In service mode the collector saves events for 24 hours in a separate
`events.sqlite3` next to the metrics history (private to you, at most 16 MiB and
50,000 events). In standalone mode Tha shows what the engine still retains plus anything
that happens while it is open, and nothing is saved. Native containerd is not supported.

### Compose update actions

Select a Compose project or service, then press **`a`** or use a command:

| Command | Runs | Effect |
| --- | --- | --- |
| `:compose-update` | `pull`, then `up --detach` | Download newer images and apply them |
| `:compose-pull` | `pull` | Download only; running containers are unchanged |
| `:compose-up` | `up --detach` | Apply configuration changes |
| `:compose-recreate` | `up --detach --force-recreate` | Recreate every container |
| `:compose-down` | `down` (project only) | Remove containers and networks; **type the project name** to confirm |

Tha never passes `--volumes`, `--rmi` or `--remove-orphans`. Each operation shows its
directory and files before running, because they come from the labels Compose set on the
containers and Compose executes what those files say. Output streams into a dialog; **Esc**
hides it while the operation continues, and **`:compose-output`** reopens it. Only one
operation runs at a time, and quitting Tha stops it where it is. A failure stops at that
step with no retry or rollback.

This needs the `docker compose` plugin, `podman compose`, or `docker-compose` on `PATH`, and
the project's files on this machine. Projects started elsewhere are refused with the path
that is missing. Native containerd is not supported.

### Logs

Streams fetch up to 300 historical entries per container, then follow live output.
At most 32 containers are subscribed to at once; select a service to narrow a
large project. History defaults to 5,000 entries, with bounded channels and
truncated oversized records. The UI reports evicted entries.

Scrolling pauses follow without discarding the retained buffer. If a paused
entry is evicted, the viewport advances to the earliest retained entry. Search
matches source labels and payload text case-insensitively, **without filtering
away surrounding entries**. Every occurrence is highlighted using reverse video;
the current occurrence is also bold/underlined. `n`/`N` navigate with wraparound,
and the status bar displays the current/total occurrence count. Search is literal
by default; **Ctrl-R** in the search prompt switches to case-insensitive regex
(`(?-i)` makes a pattern case-sensitive). An invalid pattern keeps the prompt open and
your current search in place. `Esc` clears highlights. Changing scope or leaving/reopening
the log view starts a fresh subscription.

Streams reconnect automatically when a container restarts or the engine connection
drops. Tha resumes from the last line it received, skips anything replayed, and retries
at most every 5 seconds while a container is stopped. `:reconnect-logs` reloads
immediately and starts from an empty buffer.

**`:logs-since 15m`** asks the engine for everything from that time, instead of the last
300 entries, and stays in effect across selections until **`:logs-since off`**. The retained
history limit still applies: the newest entries are kept and evictions are reported, so
raise `log_capacity` for long ranges. Each container's initial read is limited to 8 MiB and
15 seconds. The F3 filter's `since`/`until` differ: they only narrow what is already retained.

**`J`** indents JSON objects and arrays, keeping key order and exact numbers. Search then
matches the text on screen. The original text is what filters and `:export-logs` use.

Wrapping defaults to on. Press **`w`** in the log pane or use **`:wrap`** to switch
it at any time. `log_wrap = false` changes only the initial configuration;
keyboard toggling always remains available and does not write your configuration.
Unwrapped logs use `h`/`l` for horizontal scrolling. Wrapped logs preserve
whitespace and long tokens and scroll by displayed rows, with Unicode grapheme
boundaries respected. Paused position is anchored to an entry and payload byte
when toggling wrapping or resizing. Following continues to track the latest rows.

The right-edge scrollbar and **Rows first–last / total** readout show your
position in retained history. With wrapping on, these are displayed rows rather
than container log records. With wrapping off, each entry occupies one row.
Follow keeps the thumb at the bottom; pausing, searching, and resizing update
its position. Click or drag it to jump, or click its arrows to move one row; both pause
following, and `f` or `G` resumes.

Merged logs are ordered by engine timestamp. History is fully sorted before it is shown;
live lines are held for 150 ms so concurrent containers interleave correctly, and a line
arriving later than that appears where it arrived. Single-container logs have no delay.
Semantic multiline grouping is follow-up work.

## CPU and memory

### Runtime Images and Volumes

**U** or **`:check-updates`** in Images compares each visible tagged image with its registry
and marks the ones that differ with `↑`; **Enter** shows the result for an image. It only
reads digests, happens only when you ask, and results last for the session. A locally built
image or a private registry reports *unknown*, never *up to date*, because the check is
anonymous. **`:pull-image`** pulls every tag of the selected image with the Docker or
Podman CLI, using your existing logins. It needs no confirmation because it changes no
container: running containers keep their current image until you recreate them, and the old
image stays listed, untagged, until you clean it up.

Use **F7 / F8** to cycle backward/forward through Containers, Images and Volumes.
**Ctrl+Left / Ctrl+Right** cycles these three overview screens. Shift+arrows remain compatibility aliases.
Existing configured navigation bindings take precedence over these defaults.
Or switch directly with **Alt+1** (Containers),
**Alt+2** (Images, also `:images`), and **Alt+3** (Volumes, also `:volumes`).
On macOS these are **Option+1/2/3**; configure your terminal's Option key to send
Alt/Meta (Esc-prefixed keys), rather than special characters. The commands work
when a terminal intercepts these shortcuts.
Images and Volumes are not container detail tabs. Use **Left/Right** to select
a runtime and **Up/Down**, **j/k**, or **PageUp/PageDown** to select table rows.
**gg / G** (or Home / End) selects the first / last row. Tables sort running
references first, then stopped-only, unreferenced, and unknown usage.
**s** cycles usage / name / descending size sorting (unknown sizes last).
**/** opens case-insensitive search across names, tags, IDs, projects, mount
metadata and referencing containers. Search previews as you type; Enter applies,
Esc cancels, and Ctrl-U clears the query. **f** cycles status filters; Images also
offers Untagged, and Volumes offers a separate Bind mounts filter.
**Enter** opens full details alongside the table on wide terminals, or in place
of the table on smaller terminals; j/k and paging scroll details. **Esc** closes
details first, then returns to Containers. **Alt+1** returns without changing its selection or detail
tab. Plain **1–8** select container detail tabs and return to Containers; **8** instead opens the selected image's report in the Security view. Both inventories include resources outside the current workspace.
Selection, search, filter and sort are kept independently for each runtime/view
during the session. Selection follows resource identity across refreshes.

Both views show running and stopped container references. Images include tags,
IDs and reported sizes; untagged does not necessarily mean unused. Usage is marked
unknown when missing image IDs prevent proving that an image is unreferenced.
Volume driver/mountpoint metadata is shown when reported; sizes remain unknown
rather than scanning filesystems. Shared image layers mean image sizes cannot be
added up to estimate reclaimable space. Unreferenced resources are not necessarily
safe to delete.

**Space** marks/unmarks unreferenced rows; **u** marks all unreferenced resources
matching the current search/filter, and **c** clears marks. **Delete** (or **d**)
removes marked resources, or the current row when nothing is marked. Search/filter
changes clear marks; runtime and overview switching keep their selections separate.
One scrollable confirmation lists the frozen count and names: **y / Enter confirms**,
**n / Esc cancels**. No full-ID typing is required for images or volumes.

**p** (or **`:prune`**) prunes all unreferenced images or managed volumes in the
selected runtime, including resources hidden by search/filter and regardless of
marks. It opens the same confirmation with the exact count and names before any
deletion; resources appearing afterward are not added to the confirmed list.

Deleting volumes permanently destroys their stored data. Tha rechecks runtime
identity and all running/stopped references before **each** non-forced deletion,
with at most four requests in flight. Newly referenced resources are skipped;
partial successes, skips and failures are reported via **`:removal-results`**.
Esc can hide progress after confirmation without cancelling sent requests.
Referenced resources, unknown image usage, bind mounts and demo mode are protected.
There is no force or parent-image pruning; this uses individual removal requests,
not the runtime's broad prune endpoint. Failed/time-out responses are never retried
automatically; refresh and verify the outcome before trying again. Container
deletion retains its existing exact-name confirmation.

Compose projects disappear from the Containers explorer after their final
container is removed and a successful inventory refresh arrives. Retained labelled
networks/volumes do not recreate empty stacks: they remain available under runtime
resources and in Volumes. Stopped containers still keep their project visible.

Docker-compatible runtimes (including Podman) use the existing snapshot inventory.
Unsupported or failed inventory queries display an unavailable message, not an
empty inventory; native containerd inventory is currently unavailable.

### Background collection and container Metrics

Press **`6`** on a container (or `:metrics`; `:history` remains an alias) for CPU, RAM, disk read/write and
network receive/transmit charts. The range legend highlights the selected window.
**`[`** selects a shorter range; **`]`** selects a longer range: 1 minute, 3 minutes,
5 minutes, 15 minutes, 30 minutes, 1 hour, 3 hours, 6 hours, 12 hours or 24 hours.
Each container remembers its last selected range for the current Tha session.
**Left/right** browse the timeline; **`f`** returns
to live. **`c`** enables a shared time cursor and **`h` / `l`** inspect readings.
**`z`** enlarges Metrics across the terminal and **Esc** restores the split view.
Small terminals show one metric group at a time; **`j` / `k`** switches groups.
Times are UTC. Downsampled charts preserve bucket peaks; cursor values are bucket
averages. Unsupported/missing data stays absent, not zero. Disk/network rates use
actual elapsed time and reset safely if counters decrease; totals are retained
in the database. These are runtime-reported container I/O, not filesystem size.

Metrics uses gap-aware Braille line charts with labeled, zero-based vertical axes.
CPU is shown in percent (**100% = one core**) and the inspected reading also shows
cores; memory and I/O retain absolute byte/rate units. Each runtime/container/range
has independent adaptive scales, based on its own weighted averages and peaks.
Scales expand immediately with headroom; they shrink only after 30 seconds of
sustained lower demand, and stay fixed while inspecting a paused window. CPU/RAM
show a dotted average reference; paired I/O lines use blue for read/receive and the
second color for write/transmit. Peak markers preserve spikes after downsampling.

Collection is **opt-in** and read-only:

```sh
tha daemon                    # Foreground collector; Ctrl-C stops it
tha service install           # Write a per-user launchd/systemd definition
tha service start             # Enable/start background collection
tha service status
tha service stop
tha service uninstall         # Preserve history and configuration
tha history status
tha history compact           # Offline maintenance; stop daemon first
tha --standalone               # No daemon/history DB; session-only sampling
```

Alternatively, install in service mode with the [quick installer](#quick-install)
(`sh -s -- service`) or `make install MODE=service`; both install and start the
user service. The default `standalone` mode installs only the CLI. No sudo or system-wide
service is used. Services run as your user and need the same runtime/socket
permissions and CLI availability as interactive Tha. Use
`tha --config /absolute/path/config.toml service install` to pin a configuration.
Stop/uninstall the user service before removing or moving its executable.
On macOS, a user agent starts after login; on Linux, a user manager must be
available (collection after logout depends on the user's systemd setup).

The daemon targets **one-second sampling**, retains a rolling **24 hours** of
individual container readings in SQLite WAL mode, and survives restarts. It does
not start/stop containers, collect logs, pull images, or store container secrets.
The UI automatically uses daemon metrics while its writer lock is active;
starting/stopping the daemon hands collection between modes. Missing daemon
targets stay unavailable instead of triggering a second metrics collector.
Without a daemon, Metrics is session-only and currently limited to the most
recent **60 sampling batches**, clearly labelled in the view.

Default history paths:
- macOS: `~/Library/Application Support/tha/metrics.sqlite`
- Linux: `$XDG_STATE_HOME/tha/metrics.sqlite` (default `~/.local/state/tha/metrics.sqlite`)

```toml
history_disk_mb = 1024          # Approximate DB/WAL guard, 16–65536 MiB
# history_database = "/private/user-owned/directory/metrics.sqlite"
# discover_connections = false # Use only explicit [[connections]], also in daemon
```

Expired readings are deleted in small transactions at startup and during
minute-based maintenance; cleanup continues between ingestions until caught up.
Queries always exclude readings older than 24 hours. SQLite reuses freed pages;
the database file may retain its high-water size. Periodic WAL checkpoints limit
normal write-file growth; offline compaction reclaims free pages and needs spare
disk space. The disk guard pauses sample storage with an explicit warning/dropped
sample count rather than deleting recent history. It is not an OS filesystem
quota: in-flight transactions/checkpoints need headroom. Increase the budget for
larger fleets. Slow runtimes have gaps rather than an unbounded request backlog.
At most two runtime stats batches (eight container requests each) run concurrently;
the writer queue is bounded to eight batches and excess data is reported dropped.

**Live data and persistence are separate.** Samples reach the UI over a private,
same-user Unix socket every second. SQLite receives one transaction across runtime
batches **every 15 seconds**, or earlier when the persistence buffer reaches
20,000 readings / approximately 4 MiB. Normal shutdown flushes the remaining
buffer. An unexpected crash can lose the unflushed window (normally up to 15
seconds); this is a deliberate durability/performance tradeoff, not downsampling.
Every individual collected tick is preserved when it is flushed.
Routine heartbeat, flush cadence and sample-count diagnostics go to the daemon's
stderr log and remain available through `tha history status`, not the metrics UI.
History errors and persistence/data-loss warnings remain visible in the UI.

Opening a UI session fetches recent live readings as well as SQLite history. The
merge excludes exact live timestamps from the SQL query, so a concurrent flush
cannot create missing or duplicate ticks at the boundary. Genuine collection
failures/downtime remain gaps. The recent live cache is bounded to 45 seconds,
50,000 readings and approximately 16 MiB; overload can reduce available backfill.
Live responses and clients have size/time/concurrency limits. The socket is mode
0600 in the private history directory, and both peers verify same-user ownership.
If the daemon is stopped while a UI remains open, its local session readings can
overlay persisted history without writing to the database. Daemon-origin readings
are not replayed as session samples, preventing duplicates during source handoff.

Offline checks and collector measurements (no real runtimes or services):

```sh
uv run --with pyte python scripts/daemon_smoke.py
uv run --with pyte python scripts/daemon_smoke.py --benchmark --containers 100 --binary target/release/tha
cargo test --release --locked --lib history::tests::benchmark_one_day_storage_and_chart_query -- --ignored --nocapture
```

The explorer shows aligned CPU/Mem columns for runtimes, namespaces, Compose
projects, services, and containers. Press **`p`** to expand the **bottom-left
Performance panel**: per-runtime CPU sparklines, memory bars, and sample coverage.
It starts collapsed to one header row and shows **connected, non-alias runtimes**
across all projects, even inside a Compose workspace. Offline/stopped, connecting,
and alias contexts are omitted from Performance; offline rows remain in Explorer
so you can start them again. Tab into Performance and use `j`/`k`, paging, `gg`/`G`
to navigate; `Enter` opens the highlighted runtime's Overview. Selection follows
the same runtime across sorting and refreshes while it stays connected. An empty
panel says **No connected runtimes**. The right pane keeps its full height for the
active tab in compact mode. Press **`P` (Shift-p)** to switch to the selected
container's own CPU and RAM history (and open the panel if closed). Container
mode follows Explorer selection; groups, networks and volumes say **Select a
container** rather than reusing another target's readings. In container mode,
`j`/`k` moves Explorer selection and `Enter` inspects that container.

With Performance focused, **`z`** toggles a larger **full-width bottom panel**,
keeping Explorer and details above it. **Esc** restores compact mode first;
closing with `p` also resets zoom. Tiny terminals fall back to compact rendering.
CPU/RAM graphs share a timestamp-based axis ending at **latest**; the sample-age
label tells you how long ago that was, rather than implying stale data is live.
History is session-only, bounded to 60 sampling batches, and collected for all
running containers regardless of which one is selected. Missing readings remain
gaps, not zeros. Container memory bars use its reported limit, not VM capacity.
Small terminals keep metrics in Overview even without explorer columns.

**Sampling targets one update per second**, independently of the five-second
inventory refresh. Set `metrics_refresh_seconds = 1` in the config (the default;
valid range 1–60). Each engine has an independent worker, with at most eight
concurrent container requests, two-second request timeouts, and a bounded batch
deadline. Slow runtimes may refresh less frequently; Tha does not overlap polls
or block keyboard input/logs to maintain an artificial one-second rate. Large
inventories rotate through the bounded queue to avoid starvation.

- **CPU:** `100%` means one fully used CPU core, so values can exceed `100%`.
  Docker-compatible engines use deltas between observed CPU/system counters,
  accounting for online CPUs. The initial sample and reset counters show `warm`
  until a valid delta exists. Idle CPU is a measured `0.0%`, not an error fallback.
- **Memory:** Docker-compatible engines subtract inactive-file cache when valid
  metadata is present (Docker CLI working-set convention); otherwise they show
  reported usage. Containerd uses nerdctl's reported, rounded usage values.
- **Totals:** runtimes/projects/services sum the currently known running
  containers. They are **not whole-host or whole-VM usage**. Container details
  show the runtime-reported memory limit; Performance bars use engine memory
  capacity when available. Limits are never summed into a fictitious total.
- **Freshness:** `--` means unavailable; `stopped` means not running; `stale` means
  an old or failed measurement. Successful measurements stay fresh for 30 seconds
  on both local engines and remote servers; older samples are excluded from live
  totals. Failed measurements or disconnected engines are stale immediately.
  `*` marks partial totals, with exact coverage in
  Overview. Last-known failed samples remain visible in container details only,
  explicitly labelled stale. No error is silently converted to zero.
  A connected workload with no running containers has a known zero aggregate;
  stopped individual containers still show `stopped` instead of a fake sample.
- **History:** retain at most 60 recent sampling batches. Charts use each engine's
  current workloads, scale to at least one core, and leave gaps for incomplete
  samples rather than inventing zero usage. The horizontal axis is recent
  samples, not a guaranteed wall-clock minute.

Docker, Docker-backed Colima, OrbStack, Docker Desktop, and Podman use the stats
API where supported. Native containerd uses `nerdctl stats --no-stream` and needs
supported cgroups and permissions. Its CLI may take longer than one second per
sample. Missing fields or unsupported stats remain unavailable. Alias contexts
are not sampled twice. Stopped/deleted containers are removed from the live
metrics cache, and changing engine identity discards old readings.

## Interactive exec

Select a running container and press `e` or run `:exec`. A service with multiple
eligible running containers opens a picker—Tha never silently picks a replica.
For remote containers, an administrator must add `--allow-exec` to the server's
setup command (or set top-level `allow_exec = true` in `/etc/tha/agent.toml`, followed
by `sudo systemctl restart tha-agent`). It is disabled by default and independent
of `allow_delete`. Setup reruns preserve an existing opt-in.

The same opt-in also enables **server shells**: select a connected remote server
row and press `e`. This opens the SSH user's configured login shell interactively
(falling back to `/bin/sh` if unavailable), with that shell's normal completion
and startup files. It runs as the SSH login user, not as the root agent daemon.
A root SSH login therefore opens a root shell; the
dedicated `tha` login opens a shell with that user's permissions. The restricted
key's forced-command and forwarding restrictions are not relaxed by Tha. Full
shell access can modify account-owned files and keys; disabling `allow_exec`
does not undo such changes or close existing host-shell sessions. See the
[README's enabled/disabled examples](../README.md#remote-container-and-server-shells-server-opt-in).

Exec is disabled for demo workloads, disconnected engines, stopped containers,
and Kubernetes-managed containers. This is container exec, **not Kubernetes
`kubectl exec` support**.

Exec stays in the **right pane**, using a native PTY, vt100 terminal emulator,
and Ratatui terminal widget. Tha never hands off the host terminal or prints
child control sequences directly to it. The explorer, metrics, and navigation
remain available. For local containers, the runtime CLI (`docker`, `podman`, or `nerdctl`) gets the exact
endpoint and containerd namespace; ambient context/TLS overrides are removed.
Your active context is never changed. No host shell interpolation is used.
Remote container exec uses the agent's Docker API connection instead of a CLI;
input, raw terminal output, resizing and exit status stay on the authenticated
SSH transport. Host shells use a separate forced-command SSH channel and a
server-side PTY, without relaxing the key's no-PTY/no-forwarding restrictions.

Press `4` to select **Exec**, then `Enter` to start/focus shell input. `e` also
starts or refocuses a shell. The right-pane header always identifies the mode,
session's actual runtime/container, connection state, and **SHELL INPUT** versus
**NAVIGATION**. While in shell input, Escape, Ctrl-C, arrows, Tab, letters,
and numbers go to the container. **Ctrl-]** returns to Tha navigation (legacy
terminals encode it identically to Ctrl-5). Then `?` opens Tha's cheatsheet.
Use `4` then `Enter` to resume. In navigation, `j`/`k`, paging, `gg`/`G` scroll
the retained shell screen; typing resumes at the live prompt.

Up to **four retained shell sessions** are available by default. `e` opens the
selected container or refocuses its existing running session. Each session has a
stable ID shown in the Exec pane title, its own target, state, input queues,
emulator, and scrollback. **Browsing never redirects a session.** Background
sessions continue to drain output without stealing keyboard focus.

Use **Ctrl-]**, then **F6** (or `:exec-sessions`) to select a session. Selection
shows its output in navigation mode; **Enter explicitly resumes input**. While
shell input is active, F6 goes to the shell like other function keys. Exited output
is retained and counts toward the limit. `:close-exec` closes/removes only the
active slot; another retained session is selected in navigation mode, never with
automatic input focus. No session is evicted when the limit is reached. Set
`max_exec_sessions = 4` (range 1–8) in configuration to change the resource cap.
Sessions are never serialized or automatically reopened on startup.

Normal nonzero exits remain visible; exit 130 is shown as interrupted rather
than automatically blamed on a missing shell. Closing Tha closes all its local
PTY/CLI processes; it does not stop containers and cannot guarantee termination
of every background command started inside it. Exit the shell cleanly when needed.

Viewport resizing is forwarded through the PTY, with bounded output queues,
input queues, and 500 rows of scrollback. Pasting over 64 KiB is rejected rather
than truncated into a different command. Clipboard/title/host-resize control
sequences are contained in the emulator, not applied to the host terminal.

The default container command is `sh`. Set `exec_command = ["bash", "-l"]`
or another argv array in the config for images without `sh`. If the runtime CLI
is missing or exits unsuccessfully, Tha shows the error in its existing view.
Exec may modify container contents; it is intentionally **not read-only**.

## Saved preferences and navigation keys

On a normal exit Tha atomically saves UI preferences to
`$XDG_STATE_HOME/tha/preferences.toml` (default `~/.local/state/tha/preferences.toml`).
This includes Performance visibility, wrapping, context sorting, the non-Exec tab,
expanded branches, selection, and Compose workspace. The first launch still starts
with Performance collapsed. Set `save_preferences = false` to disable loading and
saving; demo and connection-listing modes never write preferences. Invalid or
unknown-version state is reported and preserved rather than overwritten.

Directory-based Compose matching takes priority over saved navigation; `--overview`
skips navigation restoration. Any key press cancels pending navigation restoration,
so late inventory cannot take over your selection. Restored names must identify a
unique connection; missing/ambiguous targets fall back safely. No logs, filters,
search text, shell contents, endpoints, credentials, or sessions are persisted.
Restoration never grants shell input or opens a mutation dialog. To reset, move
the preferences file aside while Tha is closed.

Known local runtime provider/profile/machine names are also retained (not socket
endpoints, commands, or credentials), so offline runtimes remain selectable after
a normal exit and relaunch. This retention requires `save_preferences = true`.
Existing configuration and Docker context metadata are never removed by Stop.
Older Tha versions may reject the new optional runtime metadata; move the
preferences file aside when downgrading rather than deleting runtime configuration.

## Settings

Press `,` or use `:settings` to change these live: **Theme** (terminal colors, Catppuccin
Mocha, Gruvbox Dark, Tokyo Night, Nord), **Language** (English or Deutsch), **Keyboard layout**
(US or German, see [keyboard layouts](keybindings.md#keyboard-layouts)), **Mouse**, and
**Wrap logs**. Enter on Theme or Language opens the full list: moving previews each option,
Enter applies it and Esc restores the previous one.

Language and keyboard layout are separate settings, so any language works with any layout.
Keys never change with the language: `y`/`n` confirm in every language. The translation covers
the interface. Text from your engine, log lines, command names, the security service's scan
progress, and the command-line tool (`--help`, startup errors) stay in English.

Themes only change foreground colors, so transparent terminals keep working; the palettes
need a truecolor terminal. `NO_COLOR`, `--monochrome` and `:monochrome` turn colors off
separately from the theme.

Choices are saved in the preferences file, never `config.toml`, and win over the matching
`theme`, `language`, `keyboard` and `mouse` config values. **Reset to defaults** returns to the config
values. With `save_preferences = false` or `--demo`, changes last for the session.

Add navigation-only aliases using crokey key syntax in `config.toml`:

```toml
[bindings]
"alt-j" = "down"
"alt-k" = "up"
"ctrl-left" = "previous-view"
"ctrl-right" = "next-view"
"x" = "zoom"
"z" = "none" # Disable the old fullscreen key.
"alt-n" = "focus-next"
"alt-s" = "stop" # Confirmation and runtime checks still apply.
```

Every action in the shortcut catalog can be bound, including fullscreen, pane and
overview navigation, metrics controls, inventory, Security, and lifecycle actions.
See the [complete action reference](keybindings.md) or the action-name list in
`?` help. The [example config](../tha.example.toml) includes common bindings.
Bindings override matching defaults **only in navigation**; editors, pickers,
confirmations, and embedded shells receive the original keys. Esc, Enter, Tab,
BackTab, `/`, `:`, Ctrl-c and Ctrl-] (including Ctrl-5) are reserved. Invalid,
duplicate-normalized, and unsupported multi-key bindings fail configuration loading.
Other default keys remain active unless overridden or set to `none`. Restart Tha
after editing the config. Configured aliases are listed in `?` help. Lifecycle
shortcuts can be rebound, but confirmation input and all safety checks are unchanged.

## Why is this unhealthy?

With one container selected, run **`:diagnose`** or choose it from the **`a`** menu. It works
for healthy, unhealthy, restarting and stopped containers, and the runtime is only asked to
inspect: no check is run and nothing is changed. Sections keep what is reported apart from
what might follow from it:

- **Current state**, including a plain statement when no current failure is reported.
- **Health checks**: status, failing streak, and the latest 10 reported attempts with time,
  exit code and output, capped at 4 KiB each. The check command is on the Overview tab.
- **Exit, OOM and restarts**, with times. An old exit or OOM is labeled as history and not as
  the cause of a current failure, and a restart total is not treated as a restart loop. Docker
  clears the exit code and OOM flag when a container starts, so for a running container the
  events are the place to look for earlier exits.
- **Possible explanations (not proven)**. A possible restart loop needs 3 timed exits within
  10 minutes in the available events. It also withholds the "no current failure" statement,
  because a crash-looping container looks fine between crashes.
- **Related events and logs**, previewing 20 of each, only from this exact container ID. Logs
  are called related to a failure only when timestamps place them from 60 seconds before it
  to 10 seconds after. With no known failure time they are labeled recent available logs.

Anything not reported reads *unavailable* or *unknown*, never as zero or healthy.

The Overview tab (`1`) also shows the selected container's health check as configured, with the
same names as a Compose file (`test`, `interval`, `timeout`, `start_period`, `retries`), plus its
failing streak and latest result. Settings that were not set are left out. It says whether the
check came from the image or was set on the container; the runtime keeps no record of Compose
versus `docker run`. `Entrypoint` and `Command` appear only when they differ from the image's own,
so a value set to exactly what the image already has does not show. Docker-compatible engines
have one health check per container, with no separate liveness or readiness probes, and
Kubernetes probes are not visible through the runtime. **These commands can contain credentials,
and this tab shows them as configured.** Tha re-reads only the selected container, every 10 seconds
while the tab is open. A group selection shows only the health status of each container.

Keys: **`j`/`k`** scroll, **`r`** refreshes, **`l`**, **`e`** and **`m`** open Logs, Events and
Metrics for the same container, and **Esc** closes. Logs keep your filter, and the view warns
when it hides related entries. This view does not load logs itself, so its preview is empty
until Logs has been opened for exactly this container. Event coverage is the last 24 hours
with the service, and otherwise only what the engine retained and Tha saw while open.

Each load has a 5-second deadline, and detailed inspection runs only when you open or refresh.
If a container is recreated, its evidence stays labeled stale and does not move to the new
container with the same name. Native containerd shows inventory facts with a notice, because
that adapter has no detailed inspection. Nothing is uploaded or saved. Remember that
health-check output on screen can itself contain secrets.

## Network troubleshooting

With one container selected, run **`:network-diagnose`** or choose it from the **`a`** menu.
**This view is passive.** It reads what the runtime reports and sends no DNS lookups, pings,
TCP or HTTP probes, runs no exec, and starts no helper containers. It cannot tell you that
two containers can reach each other, only what is configured.

- **Network mode**: host, none and container-sharing modes are named as such, and are not
  shown as ordinary network membership.
- **Networks and addresses**: network ID, IPv4 and IPv6 addresses, and reported aliases, which
  are marked as resolution not tested. Missing values read *unavailable* or *unknown*.
- **Exposed and published ports**: exposed-only ports are not shown as host endpoints, and each
  binding states its scope. Addresses are on the runtime host, which may not be this
  workstation.
- **Possible overlaps** with other containers on the same runtime, for the same protocol and
  port. They are labeled unconfirmed: wildcard and dual-stack cases depend on things the
  metadata does not say, and host processes are not checked.
- **Peer comparison**: press **`p`** to choose another container on this runtime. Networks are
  compared by ID, not by name, so two networks that share a name are reported as different.
  A shared network is configuration, not proof of connectivity. No shared network does not
  rule out published-port, host-network, routed or external paths. Missing IDs and special
  modes give an incomplete comparison rather than a definite one.

Keys: **`j`/`k`** scroll, **`r`** refreshes, **`p`** chooses a peer, **`n`** opens Networks, and
**Esc** closes. Inside a project workspace, containers outside the project are not offered as
peers, and appear only as counts of hidden attachments and overlaps. Native containerd keeps
its existing limits: no detailed inspection and no peer comparison.

## Log filters and export

### Text filter

Press **F3 in Logs** or run `:filter-logs` and type some words. Only entries whose
message contains **every** word stay visible, so `backend1 500` keeps
`backend1 returned 500` and `500 returned by BACKEND1`, but hides `backend1 returned 200`,
`backend2 returned 500`, and a `backend1` entry that happens to be next to a `500` entry.

Words are case-insensitive literal text in any order. There are no quotes, operators or
regex, so `[500]` means those five characters. Matching looks at the original message, not
the source name, the timestamp the engine adds, or the indentation from `J`: an entry from
a source named `backend1` with the message `HTTP 500` does not match `backend1 500`. Use
the advanced `source` key to narrow by source.

Enter applies and Esc keeps the previous filter. Applying empty text removes only the text
constraint. **`/` stays a separate highlight search** that hides nothing, and it only
counts entries the filter leaves visible.

### Advanced JSON filter

Press **Tab** in the F3 dialog, or run `:filter-logs-advanced`, to edit a JSON filter. It
combines with the text filter using AND, and applying one never changes the other. Tab
keeps what you typed in the other field, but Enter applies only the field on screen.
Ctrl-u clears the editor, Enter applies valid JSON, and Esc cancels. Invalid filters leave
the previous filter intact. Examples:

```json
{"source":"api","min_level":"warn"}
```

```json
{"fields":{"/http/status":500},"since":"2026-10-29T10:00:00Z","until":"2026-10-29T11:00:00Z"}
```

Supported keys are `source` (case-insensitive substring), `level` (exact severity),
`min_level` (minimum severity), `fields` (typed equality at JSON pointers), and
inclusive RFC3339 `since`/`until` bounds. Severities are trace/debug/info/warn/error/
fatal; warning and critical are accepted aliases. JSON numeric `500` differs from
the string `"500"`. Pointer escapes use `~0` and `~1`.

Structured records recognize `level`, `severity`, `log.level`, or `log.level` nested
as an object. Timestamps come from a runtime RFC3339 prefix, or JSON `timestamp`,
`time`, or `ts` strings. Plain-text severity is recognized only as the first token
after a recognized timestamp (or at the start of a line), not arbitrary words
inside messages. Numeric/ambiguous timestamps are not guessed. Entries lacking
required metadata do not match the corresponding filter.

Filtering never deletes retained entries or fetches older history. Counts show
matching/retained entries; wrapping, scrolling, highlighted search, and match
navigation operate on matching entries. `:clear-log-filter` clears the text and advanced
filters together and restores all retained entries without clearing search; `{}` in the
advanced editor clears only the advanced part. Filters remain visible and
active when browsing another scope until explicitly cleared; they are not written
to disk.

With the desired **Logs view open and reviewed**, run `:export-logs`. The dialog
warns that logs may contain secrets and requires an explicit filename in an
existing directory, and it names the active filter. Enter exports a frozen snapshot of
matching retained entries, with the same text and advanced filters as the view, as JSONL (`sequence`, `source`, `text`, `level`, `timestamp`). Scope changes require
canceling and reopening the dialog. A terminal of at least 60 × 18 is required.

New files use mode **0600** (or stricter under your umask). Existing files and
symlinks are refused atomically; no overwrite option or automatic redaction is
provided. Empty or oversized snapshots are refused before file creation; the
snapshot limit is 16 MiB and never silently truncates. File writes run off the UI
thread. Wait for the completion notice; an I/O failure or interruption can leave
a partial **new** export file, never a truncated existing file.

## Image layers

Press **`7`** (or `:layers`) with one container selected, including a one-container
service. Tha asynchronously inspects the container's resolved image, never a mutable
tag. The view separates **uncompressed filesystem diff IDs** (base → top) from
**build history** (base → top). `j`/`k`, paging, and `gg`/`G` select rows; Enter opens
details, where `j`/`k` scroll and Enter/Esc returns. `:reload-layers` retries a failed
lookup. Switching selection/endpoints cancels the previous lookup and discards late
results. Demo mode uses explicit seeded metadata without contacting an engine.
The list is a selectable table with row number, kind, size, and a shortened diff
ID. Enter shows the full identifier or wrapped instruction/comment separately,
without losing your table selection or scroll position on return.

Filesystem sizes and history-to-filesystem mappings are shown as unavailable when
the runtime does not provide them. A zero-byte history step is **not** assumed to
be metadata-only; only explicit OCI `empty_layer` metadata gets that label.
Docker-compatible history does not expose that flag. History sizes are labelled
as reported history sizes, not compressed registry blob sizes. Native containerd
uses explicit-endpoint, namespace-pinned nerdctl inspect/history commands and
reports unsupported formats without hiding the available filesystem metadata.

Build instructions can contain secrets. They stay hidden in the list until Enter
opens their bounded, sanitized details with a warning. Tha never executes instructions, pulls/builds images,
extracts layers, or exports history automatically.

## Container and Compose lifecycle actions

With **Explorer focused**, select a container, Compose service/project, **Services**
group, or **Standalone** group. Runtime rows have separate runtime-wide behavior
described below.

Press **`a`** to see all four lifecycle actions, eligible target counts, and the
reason an action is disabled. Selecting an enabled action opens the same safety
confirmation as its shortcut; the menu never sends a mutation itself. Availability
is rechecked against current inventory, and a disappearing selection cannot silently
retarget the menu.

- **`S`** starts existing `created`/`exited` containers.
- **`s`** stops running containers.
- **`r`** restarts running containers; stopped members are skipped.
- **`d`** deletes stopped containers (`created`, `exited`, or `dead`) only. Stop all
  members first; a group with running/paused/restarting/unknown members is refused.

These work through the Docker-compatible API for **Docker and Podman**. Services
groups target their project's existing containers; Standalone groups target only
non-Compose containers. **Delete remains disabled on these grouping rows**; select
a container, service, or project for deletion.

Podman is identified from API component metadata, never a displayed connection
name. Its Docker-compatible `/info.ID` can change on every request, so inventory
and lifecycle preflight instead use the canonical local socket path and filesystem
identity. Socket replacement refuses a pending container action. This does not
identify a backend swapped behind an unchanged forwarding socket; full container
IDs, names, Compose labels, states, and management metadata remain checked.

Network/volume rows, namespaces, unavailable/aliased connections, Kubernetes-managed
workloads, demo mode, and native containerd cannot be mutated. A parent selection
containing any Kubernetes-managed or namespaced container is refused in full,
even if that member would otherwise be skipped by the requested action.
While Exec owns input, all these keys go to the shell. `Ctrl-d` remains half-page
navigation in Tha and EOF inside Exec; it is never a delete shortcut.

Every action opens a compact confirmation showing the runtime, target, essential
warning, and (for container actions) target names/count and skipped count. Longer
target lists scroll with ↑/↓ or PgUp/PgDn. Socket paths, IDs, and commands stay in
the explicit results view, not the confirmation. **`y` or Enter confirms Start/Stop/
Restart; `n` or Esc cancels.** Delete requires typing the selected
container/service/project name exactly, then Enter. Esc cancels deletion. A
terminal of at least 44 × 10 is required to review and confirm actions (long delete
names may need more space). Enter outside a confirmation keeps its normal navigation behavior.

**Delete loses the container's writable data. Networks, images, and all volumes
(including anonymous volumes) are retained.** Force deletion is disabled. There
is no bulk image/network/volume removal and no pruning.

Compose actions affect **only the confirmed, existing containers**. They do not
run `compose up/down`, recreate removed containers, build/pull images, or perform
dependency-ordered orchestration. Start does not recreate a deleted container.
Stop/Restart allow a 10-second graceful-stop period before the runtime may kill
the process. Stopping/deleting a container can also terminate its Exec session.

Runtime identity, container IDs/names, Compose labels, management metadata, and
eligible states are rechecked before writes. Targets remain frozen when inventory
refreshes; newly discovered replicas are not silently added. Operations run
sequentially off the UI thread, with per-container outcomes and a refresh when
each container finishes and when the operation completes. Confirming closes the
dialog immediately: navigation, logs, and Exec remain available while the action
runs in the background. The footer shows progress and a retained completion/error
summary; `:action-results` opens details or a runtime chooser when several runtimes
have results. Enter/Esc closes details even while an action is running, without
canceling it. **Independent runtimes can run lifecycle actions concurrently.**
Only actions on the same runtime (including aliases) are serialized. The latest
confirmed operation/results are retained per runtime until its next confirmed
operation; they are not saved across restarts. Every worker's results are tagged
with its operation ID, so completion on one runtime cannot overwrite another.
Bulk operations are **not atomic** and have no automatic rollback.
Normal API rejections are reported per member; ambiguous failures/timeouts are
shown as **outcome unknown**, stop remaining mutations, and are never retried
automatically. Inspect current state before explicitly confirming another action.
Quitting cannot undo a request already received by the runtime.

### Runtime/context Start, Stop, and Restart

On a **runtime row**, `S` / `s` / `r` control the runtime itself, **not a bulk
container selection**. The compact confirmation names the provider target and
warns that it affects **all workloads**. Runtime shutdown affects all workloads,
including other projects and any provider-managed Kubernetes workloads; OrbStack
shutdown also stops its Linux machines. Runtime Delete is disabled.

| Provider | Commands |
| --- | --- |
| Colima | `colima --profile PROFILE start/stop/restart` |
| Podman machine | `podman machine start/stop/restart MACHINE` |
| Docker Desktop | `docker desktop start/stop/restart` |
| OrbStack | `orbctl start/stop`; restart is stop followed by start |

Provider control requires a recognized local socket location or a natively
discovered Podman machine, not a display-name match. Generic Docker sockets,
remote contexts, remote servers, native containerd, and aliases cannot control a daemon. There is
no fallback to stopping containers, no sudo/system-service management, no machine
initialization, and no context switching. Colima/Podman commands verify the named
instance still exists before proceeding. The corresponding CLI must be installed.

Stopped/unreachable contexts stay visible as **`(offline)`**. Select the runtime
and press `S` to start it; Stop/Restart require a connected runtime. API refresh
determines when it is actually available again. Runtime control stays in the
background with `:action-results` details, a 120-second deadline per command and
bounded output. Failures after invoking a lifecycle command are outcome unknown,
never automatically retried; an OrbStack restart does not proceed to Start if
Stop fails. Quitting Tha cannot undo a command already sent to the provider.

### Ports in Container Overview

Overview shows reported host-to-container mappings, for example
`localhost:8080 → 80/tcp (loopback only · 127.0.0.1)`. IPv6 addresses use brackets.
Wildcard bindings such as `0.0.0.0:8080` / `[::]:8080` are labelled **all interfaces,
not localhost-only**. Specific host addresses and TCP/UDP protocols are preserved.
An exposed port without a host publication says **exposed, no host port**; missing
binding addresses are not guessed. Offline bindings are marked last-known and
stopped containers are not presented as listening. These are runtime-reported
bindings, not an active connectivity test: VM/remote forwarding, firewalls, and
whether the application is listening can affect localhost access.

## Context ordering

Contexts default to connected first, then alphabetical (case-insensitive) within
each group. Unavailable contexts remain visible. Press `o` or `:sort-contexts`
to choose connected-first, name, descending CPU, or descending memory ordering.
CPU/memory order captures complete, fresh measurements when you confirm the
choice; it stays fixed until you sort again. Missing/partial/stale values sort
after measured values. These are summed container metrics, not host usage.
Only top-level contexts and the global Performance dock are reordered; workloads,
expanded branches, selection, and logs/Exec targets retain their identities.

Pin preferred contexts first using exact displayed names, in priority order:

```toml
context_order = ["Colima / default", "OrbStack"]
```

Pins take precedence in every sort mode, even when unavailable. Undiscovered
names are ignored; duplicate or empty pin names are rejected.

## Container state and image architecture

Status circles use the terminal palette, with text labels so color is not the
only signal:

- **Green:** running, or explicitly healthy.
- **Yellow:** health-check startup, created/starting, restarting, paused,
  stopping/removing, or a group with some stopped members.
- **Red:** unhealthy, dead/failed, OOM-killed when reported, or a nonzero exit.
- **Neutral:** normal completion (exit 0), stopped/exited without a known failure,
  or unknown status. Completed batch jobs are not automatically treated as errors.

Projects/services inherit the most important member state: errors take priority
over warnings. **Runtime/context rows summarize failed children as yellow
`[workload issues]`**, rather than making a reachable engine look failed. Failed
containers and their Compose groups stay red. Disconnected engines show yellow
unavailable warnings, with full connection diagnostics in Overview only.

A single-container Overview (including a one-container service) shows **Platform**,
such as `linux/arm64/v8 (ARM64)` or `linux/amd64 (AMD64 / x86_64)`. This comes from
the **resolved image metadata**, not Tha's host CPU or a mutable image tag. It
does not claim whether emulation is in use. Lookups are asynchronous and cached
in a bounded, engine/namespace-scoped cache. Missing metadata shows unavailable;
Tha never pulls an image, starts a container, or guesses ARM because it runs on a Mac.

## Appearance

Tha inherits the terminal background and foreground, preserving transparency.
Accents use the terminal's ANSI palette. Selection uses reverse video, while
status also has text and symbols. `--monochrome`, `NO_COLOR`, or `:monochrome`
disable color. The T-shaped pier logo uses `currentColor` in
[`assets/logo.svg`](assets/logo.svg), with a text version alongside it.
There is no startup splash screen.
Pane content has two terminal columns of horizontal padding and one row of
vertical padding where space permits, reduced automatically in small panes.

## Development

### Tagged releases

`.github/workflows/release.yml` runs on pushed `v*` tags. The tag must exactly
match the package version in `Cargo.toml`, for example `v0.1.0` for `0.1.0`.
Prerelease versions such as `0.2.0-rc.1` become GitHub prereleases and are not
selected by the quick-install `latest` URL.

The offline `scripts/release_smoke.py` suite executes the actual `scripts/install.sh`
and workflow shell steps against isolated fake download/GitHub commands. It checks
the supported installer matrix, failed-download/checksum preservation, cleanup,
tag/version and prerelease rules, exact assets, moved tags, existing releases,
upload failures, and publish failures. It never contacts GitHub or reads credentials.
Every native release build also validates its archive's exact regular-file contents
before upload; links, duplicate/hidden members, and non-executable binaries are rejected.

The workflow reuses the full macOS/Linux CI checks, including all five terminal
smoke tests, then tests/builds each release target on a matching native runner:

| Asset | Rust target | Runner |
| --- | --- | --- |
| `tha-linux-amd64.tar.gz` | `x86_64-unknown-linux-musl` | `ubuntu-24.04` |
| `tha-linux-arm64.tar.gz` | `aarch64-unknown-linux-musl` | `ubuntu-24.04-arm` |
| `tha-macos-arm64.tar.gz` | `aarch64-apple-darwin` | `macos-15` |

Only after all checks/builds pass are the three archives and `SHA256SUMS`
uploaded to a draft release and then published. Each archive contains `tha`,
`LICENSE`, and this README. Publishing uses the workflow's `GITHUB_TOKEN` with
`contents: write` only in the publish job; no personal access token is needed.
No release is published on ordinary branch pushes or pull requests.

A maintainer on `main` can bump the version and trigger publication explicitly
(requires Python 3.11+, Cargo, Git, and push access to `origin`):

```sh
make release v0.19.0
```

This updates `Cargo.toml`, runs `cargo check` (updating `Cargo.lock`), stages **all
current changes, including untracked files not ignored by Git**, commits `Release
v0.19.0`, pushes `main`, creates an annotated tag, and pushes the tag. Review
`git status` first. It refuses non-main branches, existing tags, versions that do
not increase, and a local branch behind/diverged from `origin/main`. Nothing is
force-pushed. Failures stop immediately and leave files/commits intact for inspection;
if only the tag push fails, retry `git push origin refs/tags/v0.19.0` after fixing it.

Plain `make release` still only builds an optimized local binary.
Test the command offline with `python3 scripts/release_command_smoke.py`.
No tags or releases are created by local build/test commands. Existing releases
are never overwritten automatically, and moving a tag during the build prevents
publication of assets from the old commit. If publishing fails after creating a draft,
inspect that draft and its assets before deleting it or completing publication
manually; a rerun intentionally refuses to overwrite it.

### Local checks

```sh
make help
make build
make check      # Formatting, Clippy, unit/integration tests; uses 4 cores, CPUS=8 changes it
make release
make run ARGS="--demo --monochrome"
```

Tests include real Docker HTTP contracts over a mock Unix socket, grouping,
namespace isolation, duplicate engines, log limits, keyboard modes, CLI errors,
CPU counter resets, one-second sampling cadence, partial/stale metrics, and
wide/narrow terminal rendering. CI is configured for macOS and Linux.

Optional pseudo-terminal smoke test (Python tooling is not a runtime dependency):

```sh
make build
uv run --with pyte python scripts/smoke.py
uv run --with pyte python scripts/exec_smoke.py
uv run --with pyte python scripts/podman_smoke.py
uv run --with pyte python scripts/actions_smoke.py
uv run --with pyte python scripts/features_smoke.py
uv run --with pyyaml python scripts/release_smoke.py
```

The exec smoke test uses a fake runtime CLI and mock engine; it never executes
commands inside real containers. It checks embedded exec, key routing,
Ctrl-] navigation, resize/paste, retained exit output, OSC containment, image
platform, exact endpoint routing, and removal of ambient context overrides.

The Podman smoke test uses a fake Podman executable and mock API sockets. It
checks machine discovery, images, metrics, stopped machines, socket replacement,
invalid-JSON recovery, and terminal restoration without starting a real VM.

The lifecycle smoke test uses a separate mock API for every mutation. It checks
`S/s/r/d`, `y`/Enter confirmation, `n`/Esc cancellation, exact-name deletion, automatic refresh,
per-container results, and volume-preserving delete flags. `tests/actions.rs`
also covers stale identities/states/labels, partial failures, lost responses,
and no automatic retries against mock Docker 1.53 / Podman-compatible 1.44 APIs.
No lifecycle test reads live-engine endpoint environment variables.

Optional read-only live Docker inventory check:

```sh
THA_TEST_DOCKER_SOCKET=unix:///path/to/docker.sock \
  cargo test --test runtime_contracts live_docker_inventory -- --ignored
```

The matching read-only CPU/memory check requires an already-running container:

```sh
THA_TEST_DOCKER_SOCKET=unix:///path/to/docker.sock \
  cargo test --test runtime_contracts live_docker_metrics -- --ignored
```

With the existing `hello-podman` fixtures running, verify native discovery,
Compose/standalone containers, images, networks, volumes, platform, metrics,
and logs (read-only):

```sh
cargo test --locked --test runtime_contracts live_podman -- --ignored
```

Machine stops/restarts and multiple-machine cases are tested with mocks rather
than stopping real machines. Live coverage: macOS Podman 6.1.4 / API 1.44, plus
read-only Colima, Docker Desktop, and OrbStack regression checks. Other Podman
versions and Linux Podman machines still need live verification. No native
containerd image inventory was added by this change.

### Structure

- `backend/`: Docker API and native containerd adapters.
- `discovery.rs`: endpoint discovery without changing engine state.
- `discovery/podman.rs`: bounded, asynchronous native Podman machine discovery.
- `model.rs`: shared workload model and terminal-output sanitization.
- `metrics.rs`: CPU deltas, freshness, aggregation, bounded history, and sampling workers.
- `app.rs`: tree, grouping, selection, and focus state.
- `logs.rs`: cancellable subscriptions and bounded retained history.
- `logs/view.rs`: wrapped display-line navigation and highlighted search state.
- `workspace.rs`: directory matching and connection-scoped project identity.
- `exec.rs`: explicit-endpoint runtime CLI commands, without a host shell.
- `terminal.rs`: embedded PTY transport, input encoding, and terminal emulation.
- `platform.rs`: resolved image platform lookup and bounded metadata cache.
- `ui/`: Ratatui rendering using the terminal's palette.
- `main.rs`: async event loop, refresh workers, and terminal cleanup.

The UI never awaits engine requests. Each engine refreshes independently;
failures retain visibly stale last-known inventory. Log tasks are cancelled when
the selection changes, and owned nerdctl processes are killed on cancellation.

## Safety and next steps

Engine access itself is privileged. Use only sockets you trust. Tha strips
terminal control sequences from displayed workload data and never renders raw
inspect JSON or container environment variables. Interactive exec deliberately
starts commands in the selected workload—only exec into containers you trust.
Logs can themselves contain
secrets—Tha cannot infer or redact every application-specific secret.

This is an early MVP, not a claim of tested parity across every runtime. Next:
broader Podman/containerd compatibility testing, native containerd transport,
secure remote transports, explicitly associated Compose orchestration, and release-contract automation.
