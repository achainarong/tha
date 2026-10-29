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
remain visible as unavailable with last-known data; Tha does not connect to a
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

Tha never starts machines, changes rootless/rootful settings, installs helpers,
or changes the default Docker/Podman connection. Live compatibility is version-
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
| `o` / `:sort-contexts` | Choose connected-first, name, CPU, or memory context order |
| `S` / `s` / `r` / `d` in Explorer | Start / Stop / Restart / Delete selected container or Compose workload |
| `1` / `2` / `3` / `4` | Overview / Logs / Networks / Exec on the right |
| `/` | Search tree or highlight log matches when the log pane is focused |
| `n` / `N` | Next / previous match; logs navigate individual occurrences |
| `f` | Toggle follow in the focused log pane |
| `w` | Toggle wrapping immediately in the focused log pane |
| `e` / `:exec` | Open an interactive shell in a selected container/service |
| `Enter` in Exec | Focus the existing shell, or start one if no session exists |
| `Ctrl-]` in Exec | Return keyboard focus to Tha without closing the shell |
| `:close-exec` | Close the PTY/CLI session, not the container |
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

The explorer shows aligned CPU/Mem columns for runtimes, namespaces, Compose
projects, services, and containers. Press **`p`** to expand the **bottom-left
Performance panel**: per-runtime CPU sparklines, memory bars, and sample coverage.
It starts collapsed to one header row and **always covers all runtimes**, even
inside a Compose workspace. Tab into it and use `j`/`k` to scroll runtimes;
`Enter` opens the highlighted runtime's Overview. Alias connections are labelled
without duplicate measurements. The right pane keeps its full height for the
active tab; selected-container metrics remain compact numeric fields there.
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

One shell session is retained. Browsing other containers or tabs does **not**
redirect it or change its target. Starting another target is refused until the
existing shell exits or you explicitly run `:close-exec`. Exited output stays
visible, including normal nonzero exit codes; exit 130 is shown as interrupted,
not automatically blamed on a missing shell. Closing Tha closes its local
PTY/CLI process; it does not stop the container and cannot guarantee termination
of every background command started inside it. Exit the shell cleanly when needed.

Viewport resizing is forwarded through the PTY, with bounded output queues,
input queues, and 500 rows of scrollback. Pasting over 64 KiB is rejected rather
than truncated into a different command. Clipboard/title/host-resize control
sequences are contained in the emulator, not applied to the host terminal.

The default container command is `sh`. Set `exec_command = ["bash", "-l"]`
or another argv array in the config for images without `sh`. If the runtime CLI
is missing or exits unsuccessfully, Tha shows the error in its existing view.
Exec may modify container contents; it is intentionally **not read-only**.

## Container and Compose lifecycle actions

With **Explorer focused**, select a container, Compose service, or Compose project:

- **`S`** starts existing `created`/`exited` containers.
- **`s`** stops running containers.
- **`r`** restarts running containers; stopped members are skipped.
- **`d`** deletes stopped containers (`created`, `exited`, or `dead`) only. Stop all
  members first; a group with running/paused/restarting/unknown members is refused.

These work through the Docker-compatible API for **Docker and Podman**. Runtime
contexts, generic grouping/resource rows, unavailable/aliased connections,
Kubernetes-managed workloads, demo mode, and native containerd cannot be mutated.
While Exec owns input, all these keys go to the shell. `Ctrl-d` remains half-page
navigation in Tha and EOF inside Exec; it is never a delete shortcut.

Every action opens a scrollable confirmation listing the exact runtime, endpoint,
container names/IDs, target count, and skipped count. **`y` confirms Start/Stop/
Restart; Enter or Esc cancels by default.** Delete requires typing the selected
container/service/project name exactly, then Enter. Esc cancels deletion. A
terminal of at least 60 × 18 is required to review and confirm actions.

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
finished. Bulk operations are **not atomic** and have no automatic rollback.
Normal API rejections are reported per member; ambiguous failures/timeouts are
shown as **outcome unknown**, stop remaining mutations, and are never retried
automatically. Inspect current state before explicitly confirming another action.
Quitting cannot undo a request already received by the runtime.

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

The workflow reuses the full macOS/Linux CI checks, including all four terminal
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
```

The exec smoke test uses a fake runtime CLI and mock engine; it never executes
commands inside real containers. It checks embedded exec, key routing,
Ctrl-] navigation, resize/paste, retained exit output, OSC containment, image
platform, exact endpoint routing, and removal of ambient context overrides.

The Podman smoke test uses a fake Podman executable and mock API sockets. It
checks machine discovery, images, metrics, stopped machines, socket replacement,
invalid-JSON recovery, and terminal restoration without starting a real VM.

The lifecycle smoke test uses a separate mock API for every mutation. It checks
`S/s/r/d`, cancel-by-default confirmation, exact-name deletion, automatic refresh,
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
structured log exploration, configurable bindings, and image-layer exploration.
