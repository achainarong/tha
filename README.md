# Tha · ท่า

A runtime-first container terminal UI, written in Rust. **Tha** is the product;
**`tha`** is the executable. The name is Thai for a pier or port.

Explore **engine → Compose project → service → container**, with networks and
volumes next to their workloads. Native containerd adds a namespace level.

## Start

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
- Embedded PTY container exec; **no start, stop, restart, delete, or Compose mutation actions yet**.

### Runtime coverage

| Runtime | MVP connection | Notes |
| --- | --- | --- |
| Docker Engine | Docker API over Unix socket | Local and rootless endpoints |
| Docker Desktop | Docker API over Unix socket | Contexts and macOS socket discovery |
| OrbStack | Docker API over Unix socket | Context and macOS socket discovery |
| Docker-backed Colima | Docker API over Unix socket | Contexts and profile sockets |
| Podman | Docker-compatible API socket | API service must already be available; compatibility needs live validation |
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

## Container state and image architecture

Status circles use the terminal palette, with text labels so color is not the
only signal:

- **Green:** running, or explicitly healthy.
- **Yellow:** health-check startup, created/starting, restarting, paused,
  stopping/removing, or a group with some stopped members.
- **Red:** unhealthy, dead/failed, OOM-killed when reported, or a nonzero exit.
- **Neutral:** normal completion (exit 0), stopped/exited without a known failure,
  or unknown status. Completed batch jobs are not automatically treated as errors.

Projects/services/runtimes inherit the most important member state: errors take
priority over warnings. Disconnected engines are explicitly unavailable.

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
```

The exec smoke test uses a fake runtime CLI and mock engine; it never executes
commands inside real containers. It checks embedded exec, key routing,
Ctrl-] navigation, resize/paste, retained exit output, OSC containment, image
platform, exact endpoint routing, and removal of ambient context overrides.

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

### Structure

- `backend/`: Docker API and native containerd adapters.
- `discovery.rs`: endpoint discovery without changing engine state.
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
live Podman/containerd compatibility testing, native containerd transport,
structured log exploration, configurable bindings, and explicitly confirmed
lifecycle actions.
