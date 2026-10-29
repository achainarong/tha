# Tha · ท่า

A runtime-first container terminal UI, written in Rust. **Tha** is the product;
**`tha`** is the executable. The name is Thai for a pier or port.

Explore **engine → Compose project → service → container**, with networks and
volumes next to their workloads. Native containerd adds a namespace level.

## Quick install

Prebuilt releases support **Linux AMD64, Linux ARM64, and macOS ARM64**. No Rust
installation or sudo is needed. The following downloads the latest stable
release, verifies its SHA-256 checksum, and installs `tha` into `~/.local/bin`.
Requires `curl`, `tar`, and either `sha256sum` (Linux) or `shasum` (macOS).

**Release downloads become available after the first version tag is published.**
Until then, use the source-build instructions below.

```sh
(
  set -eu
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64|Linux-amd64) asset=tha-linux-amd64.tar.gz ;;
    Linux-aarch64|Linux-arm64) asset=tha-linux-arm64.tar.gz ;;
    Darwin-arm64) asset=tha-macos-arm64.tar.gz ;;
    *) echo 'Supported: Linux AMD64/ARM64 and macOS ARM64 (native shell).' >&2; exit 1 ;;
  esac
  base=https://github.com/achainarong/tha/releases/latest/download
  tmp=$(mktemp -d "${TMPDIR:-/tmp}/tha-install.XXXXXX")
  trap 'rm -f "$tmp/$asset" "$tmp/SHA256SUMS" "$tmp/CHECKSUM" "$tmp/tha"; rmdir "$tmp"' 0
  trap 'exit 1' HUP INT TERM
  curl --proto '=https' --tlsv1.2 -fsSL "$base/$asset" -o "$tmp/$asset"
  curl --proto '=https' --tlsv1.2 -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS"
  awk -v asset="$asset" '$2 == asset { print }' "$tmp/SHA256SUMS" > "$tmp/CHECKSUM"
  if [ "$(wc -l < "$tmp/CHECKSUM" | tr -d '[:space:]')" != 1 ]; then
    echo "Missing or duplicate checksum for $asset; nothing installed." >&2
    exit 1
  fi
  (
    cd "$tmp"
    if command -v sha256sum >/dev/null 2>&1; then
      sha256sum -c CHECKSUM
    else
      shasum -a 256 -c CHECKSUM
    fi
  )
  tar -xzf "$tmp/$asset" -C "$tmp" tha
  mkdir -p "$HOME/.local/bin"
  install -m 755 "$tmp/tha" "$HOME/.local/bin/tha"
  "$HOME/.local/bin/tha" --version
)
```

Then run `~/.local/bin/tha`, or add `~/.local/bin` to your `PATH`:

```sh
export PATH="$HOME/.local/bin:$PATH"
tha
```

Linux archives are statically linked with musl, avoiding a dependency on the
build runner's glibc version. ARM means **64-bit ARM**, not ARMv7. The macOS binary
is ad-hoc signed, not Apple-notarized; Intel Macs are not included. Checksums
detect corrupted/mismatched downloads, not an independently authenticated
publisher. Only install releases from the repository you trust.

To pin or roll back, choose a version from
[GitHub Releases](https://github.com/achainarong/tha/releases), download its matching
archive and `SHA256SUMS`, verify the archive, and replace the installed binary.
No runtime state or user configuration is migrated by installation.

## Build from source

Requires macOS or Linux, Rust 1.88+ (current stable recommended), and an
interactive terminal. No Docker CLI is required for Docker API browsing;
interactive exec requires an installed runtime CLI.

```sh
make demo       # Try the UI without contacting any runtime
make run        # Discover and browse your actual engines
make install    # Install tha into ~/.cargo/bin, or your configured Cargo root
tha
```

Ensure Cargo's bin directory is on `PATH`. Installation does not require sudo.
If both install locations are on `PATH`, `command -v tha` shows which takes precedence.
Tha never starts an engine, VM, or container automatically.

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
Tha does not silently turn secure connections into plaintext connections.

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
[`tha.example.toml`](tha.example.toml). Explicit connections supplement discovery.

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
| `j` / `k`, arrows | Move or scroll the focused pane |
| `h` / `l` | Collapse/parent or expand/child in the tree; horizontal scroll in unwrapped logs |
| `Space` | Toggle the selected explorer branch open or closed |
| `Enter` | Enter a Compose workspace; otherwise open details |
| `Backspace` | Return to the previous workspace |
| `gg` / `G` | First / last; `G` resumes log following |
| `Ctrl-u` / `Ctrl-d` | Half-page up / down |
| `Tab` / `Shift-Tab` | Cycle Explorer / Performance (when open) / Details |
| `p` / `:performance` | Expand/collapse bottom-left global Performance |
| `P` (Shift-p) | Switch Runtime / Selected container Performance; open panel |
| `z` (Performance focused) | Toggle compact / full-width bottom Performance |
| `Esc` (Performance expanded) | Restore compact view first |
| `o` / `:sort-contexts` | Choose connected-first, name, CPU, or memory context order |
| `S` / `s` / `r` / `d` in Explorer | Start / Stop / Restart / Delete selected container or Compose workload |
| `1` / `2` / `3` / `4` / `5` | Overview / Logs / Networks / Exec / Layers on the right |
| `a` / `:actions` | Contextual lifecycle menu with eligibility counts and disabled reasons |
| `F3` in Logs / `:filter-logs` | Edit structured retained-log filters |
| `:clear-log-filter` | Clear filters without clearing highlighted search |
| `:export-logs` | Export matching retained entries to an explicitly chosen new JSONL file |
| `/` | Search tree or highlight log matches when the log pane is focused |
| `n` / `N` | Next / previous match; logs navigate individual occurrences |
| `f` | Toggle follow in the focused log pane |
| `w` | Toggle wrapping immediately in the focused log pane |
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
filter and collapses that branch, keeping it selected. Bindings are fixed for
this MVP; configurable bindings follow later.

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
and the status bar displays the current/total occurrence count. Search is literal,
not regex syntax. `Esc` clears highlights. Changing scope or
leaving/reopening the log view starts a fresh subscription. Use
`:reconnect-logs` after a stream ends or disconnects; automatic log reconnection
is not implemented yet.

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
its position. This is a keyboard-driven indicator, not a mouse-drag scrollbar.

Merged logs show **arrival order**, not a guaranteed cross-container timeline.
Timestamped payloads are preserved. JSON expansion, time-range retrieval,
semantic multiline grouping, and export are follow-up work.

## CPU and memory

### Background collection and container History

Press **`6`** on a container (or `:history`) for CPU, RAM, disk read/write and
network receive/transmit charts. **`[` / `]`** choose 1 minute, 5 minutes,
15 minutes, 1 hour or 24 hours. **Left/right** browse the timeline; **`f`** returns
to live. **`c`** enables a shared time cursor and **`h` / `l`** inspect readings.
**`z`** enlarges History across the terminal and **Esc** restores the split view.
Small terminals show one metric group at a time; **`j` / `k`** switches groups.
Times are UTC. Downsampled charts preserve bucket peaks; cursor values are bucket
averages. Unsupported/missing data stays absent, not zero. Disk/network rates use
actual elapsed time and reset safely if counters decrease; totals are retained
in the database. These are runtime-reported container I/O, not filesystem size.

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

Alternatively, `make install MODE=service` explicitly installs and starts the
user service. Plain `make install` remains CLI-only. No sudo or system-wide
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
Without a daemon, History is session-only and currently limited to the most
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
  an old or failed measurement. Measurements older than three sampling intervals
  are excluded from live totals. `*` marks partial totals, with exact coverage in
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
Exec is disabled for demo workloads, disconnected engines, stopped containers,
and Kubernetes-managed containers. This is container exec, **not Kubernetes
`kubectl exec` support**.

Exec stays in the **right pane**, using a native PTY, vt100 terminal emulator,
and Ratatui terminal widget. Tha never hands off the host terminal or prints
child control sequences directly to it. The explorer, metrics, and navigation
remain available. The runtime CLI (`docker`, `podman`, or `nerdctl`) gets the exact
endpoint and containerd namespace; ambient context/TLS overrides are removed.
Your active context is never changed. No host shell interpolation is used.

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

Add navigation-only aliases using crokey key syntax in `config.toml`:

```toml
[bindings]
"alt-j" = "down"
"alt-k" = "up"
"F7" = "exec-sessions"
```

Actions: `up`, `down`, `left`, `right`, `page-up`, `page-down`, `first`, `last`,
`overview`, `logs`, `networks`, `exec`, `layers`, `open-exec`, `exec-sessions`,
`actions`, `performance`, `sort-contexts`, `wrap`, `follow`, `filter-logs`, `help`.
Bindings override matching defaults **only in navigation**; editors, pickers,
confirmations, and embedded shells receive the original keys. Esc, Enter, Tab,
BackTab, `/`, `:`, Ctrl-c and Ctrl-] (including Ctrl-5) are reserved. Invalid,
duplicate-normalized, and unsupported multi-key bindings fail configuration loading.
Configured aliases are listed in `?` help. Lifecycle confirmations are not remappable.

## Structured log filters and export

Press **F3 in Logs** or run `:filter-logs` to edit a JSON filter. Ctrl-u clears the
editor, Enter applies valid JSON, and Esc cancels. Invalid filters leave the
previous filter intact. Examples:

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
navigation operate on matching entries. `:clear-log-filter` (or `{}` in the editor)
restores all retained entries without clearing search. Filters remain visible and
active when browsing another scope until explicitly cleared; they are not written
to disk.

With the desired **Logs view open and reviewed**, run `:export-logs`. The dialog
warns that logs may contain secrets and requires an explicit filename in an
existing directory. Enter exports a frozen snapshot of matching retained entries
as JSONL (`sequence`, `source`, `text`, `level`, `timestamp`). Scope changes require
canceling and reopening the dialog. A terminal of at least 60 × 18 is required.

New files use mode **0600** (or stricter under your umask). Existing files and
symlinks are refused atomically; no overwrite option or automatic redaction is
provided. Empty or oversized snapshots are refused before file creation; the
snapshot limit is 16 MiB and never silently truncates. File writes run off the UI
thread. Wait for the completion notice; an I/O failure or interruption can leave
a partial **new** export file, never a truncated existing file.

## Image layers

Press **`5`** (or `:layers`) with one container selected, including a one-container
service. Tha asynchronously inspects the container's resolved image, never a mutable
tag. The view separates **uncompressed filesystem diff IDs** (base → top) from
**build history** (base → top). `j`/`k`, paging, and `gg`/`G` select rows; Enter opens
details, where `j`/`k` scroll and Enter/Esc returns. `:reload-layers` retries a failed
lookup. Switching selection/endpoints cancels the previous lookup and discards late
results. Demo mode uses explicit seeded metadata without contacting an engine.

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
remote contexts, native containerd, and aliases cannot control a daemon. There is
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

The offline `scripts/release_smoke.py` suite executes the actual README installer
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

After committing/pushing the intended release source and matching version,
a maintainer can trigger publication explicitly:

```sh
git tag v0.1.0
git push origin v0.1.0
```

No tags or releases are created by local build/test commands. Existing releases
are never overwritten automatically, and moving a tag during the build prevents
publication of assets from the old commit. If publishing fails after creating a draft,
inspect that draft and its assets before deleting it or completing publication
manually; a rerun intentionally refuses to overwrite it.

### Local checks

```sh
make help
make build
make check      # Formatting, Clippy, unit/integration tests
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
