# Tha improvement ideas

Recorded 2026-11-01. These are product suggestions, not approved implementation
plans or commitments. Effort estimates are relative and need technical scoping.

## Direction

Help users spot problems and resolve them faster rather than adding more tabs.
Connect the capabilities Tha already has: multiple runtimes, SSH servers, Compose
actions, logs, metrics history, exec, vulnerability scanning, and health/network
diagnostics.

## Decisions so far

- **Remote Compose editing is in scope**, using the user's local editor.
- **Editors:** vim, Neovim, nano, and VS Code, chosen in Settings. vim is the default.
- **Kubernetes management, Windows, and Intel Macs are permanently out of scope.**
- **CVE workflows are out of scope:** no scan comparisons, fixable-first views, or
  similar work to turn findings into actions.
- **File download is in scope** for containers and remote server shells. Users run
  their own commands, and Tha only copies the file or folder they name. The dialog
  shows the size, and for folders the number of files, before downloading.
- **`:file-explorer` is the main way in:** the tree on the left, a read-only file on the right, and `e` or
  `:edit` to open it in the local editor. `:download PATH` stays as the shortcut.

## Status

Everything in the delivery order below is finished, along with the other features unless marked. These
are not done:

- **TLS Docker contexts** and **containerd lifecycle actions, events, and Compose:** they need a real TLS
  engine and a real containerd (`nerdctl`) to verify, which were not available.
- **Jumping from a diagnostic finding straight to its log entry:** `L` shows the lines in that view instead.
- **Downsampled long-term history and comparing with yesterday:** `history_retention_hours` keeps more,
  at full detail.

Server paths were tested against a real engine through the agent code, but not over a real SSH connection.
The same is true of server file browsing and editing. Real vim, Neovim, nano, and VS Code were not launched: the
hand-off was tested with a stand-in program.

## Recommended delivery order

This order is a recommendation, not an approved roadmap.

1. **Maintenance, diagnostic log loading, and saved views (all finished):** immediate
   usability wins.
2. **Capability view (finished):** shows which opt-ins each server has before remote
   actions depend on them.
3. **Remote image actions (finished):** remove much of the need to SSH in, with less
   risk than remote Compose.
4. **File download (finished):** gets backups and other files off containers and servers
   without leaving Tha.
5. **Fleet health overview (finished):** the strongest headline feature.
6. **Saved events and the incident timeline (finished):** the timeline cannot explain
   overnight failures without saved events, so they came first.
7. **In-app alerts, then agent-side alerts (finished):** build on the health model.
8. **Remote Compose, Compose editing, and safer updates (finished):** the deepest workflows and
   the largest security review.

Everything else can be scheduled independently as demand becomes clearer.

## Main features

| Feature | User benefit | Estimated effort |
| --- | --- | --- |
| Saved views and filters **(finished 2026-11-01)** | Save named views such as “Production failures,” “Backend errors,” and “Unused images,” including server/project scope, sorting, and filters. | Small–medium |
| Remote image actions **(finished 2026-11-01)** | Pull images, check for image updates, and prune on servers without a separate SSH session. Uses `--allow-images`; pulls are anonymous. | Medium |
| File explorer **(finished 2026-11-01)** | Browse, read, edit, and download files and folders from a container or remote server, such as a database backup, with the size shown first. | Large |
| Fleet health overview **(finished 2026-11-01)** | Show unhealthy/restarting containers, unreachable servers, stale data, and critical vulnerabilities across connected servers. Jump directly to an affected workload. | Medium |
| Unified incident timeline **(finished 2026-11-01)** | Correlate exits, OOM events, health failures, and CPU/memory spikes, with links to available logs. Reuse evidence currently shown separately. | Medium |
| Alerts **(finished 2026-11-01)** | Flag crash loops, OOM kills, unhealthy containers, sustained high memory, and unreachable servers. | Medium–large |
| Remote Compose actions **(finished 2026-11-01)** | Run Compose actions on servers without a separate SSH session. | Large |
| Compose editing **(finished 2026-11-01)** | Edit local and remote Compose files in the selected editor. | Large |
| Safer Compose update workflow **(finished 2026-11-01)** | Preview affected services and image changes, record previous image identities, and check health after updates. | Large |

## Other features

| Feature | User benefit | Estimated effort |
| --- | --- | --- |
| Diagnostic export bundle **(finished 2026-11-01)** | Explicitly export selected health/network evidence, events, and bounded logs, with a preview and redaction controls. | Medium |
| Scriptable CLI **(finished 2026-11-01)** | Read-only commands with stable output, such as `tha ps --json`. Start with local engines, because remote servers may need a passphrase a script cannot enter. | Medium |
| Pause, unpause, and kill **(finished 2026-11-01)** | Add the missing lifecycle actions, with the same confirmations as other actions. | Small |
| File upload **(finished 2026-11-01)** | Copy files into containers, after file download exists. | Medium |
| Copy to clipboard **(finished 2026-11-01)** | Copy a container ID, name, or image name, and the newest visible log line. The log view has no line cursor. | Small; medium with log lines |
| Easier updates **(finished 2026-11-01)** | Add `tha update`, an opt-in new-version notice, and guidance for outdated agents and key revocation. | Medium |
| Native containerd coverage | **Finished 2026-11-01:** read-only network and volume lists. **Not done:** lifecycle actions, events, and Compose, which need a real containerd to verify. | Large |
| TLS Docker contexts **(not done; needs a TLS engine to verify)** | Connect to TLS contexts, which are visible but unsupported today. Plain TCP stays unsupported, because Tha does not downgrade connections to plaintext. | Medium |

Creating networks and volumes is left out for now because it does not serve the
troubleshooting direction.

### Update limits

The restricted `tha` user cannot update the agent or change its own
`authorized_keys`. For both tasks, Tha can only show the admin the command to run.
The new-version notice contacts GitHub, so it must be opt-in.

## File download

### File explorer

**Finished 2026-11-01.** `:file-explorer` replaces the planned download manager. The tree is on the left and the
selected file shows read-only on the right. Files over 256 KiB and binaries show a warning instead of loading. `e` or
`:edit` opens the file in the local editor, using a private temporary copy for a remote file, because a local editor
needs a local file. Editing ends when the editor process exits (or, for VS Code, through `code --wait`), and the file
goes back only if it changed. A separate pane lists downloads, and `--allow-server-edit` covers editing any server file.

### Downloading a file

1. In the container's shell, create the file, for example `/tmp/mydb.dump`.
2. Leave shell input with `Ctrl-]`, then run `:download /tmp/mydb.dump`. Running
   `:download` without a path asks for one.
3. A dialog inside Tha shows the container, the file's size, and an editable
   **Save to** field, prefilled with `~/Downloads/mydb.dump`.
4. `Enter` starts the download in the background and `Esc` cancels. Progress stays
   visible, and the shell and the rest of Tha remain usable.

The path must be absolute, and a trailing slash on a folder is optional. Tha cannot
reliably see the shell's current directory, so a `cd /tmp` followed by
`:download mydb.dump` would not work. The file must exist in
the container selected in Tha; select one replica of a service with several. The dialog follows `:export-logs`: new files use mode 0600, and existing
files and symlinks are refused.

### Downloading from a server shell

`:download` also works with a remote server selected, as after leaving a server shell
opened with `e`. The path then refers to the server's filesystem, and the dialog names
the server instead of a container. The steps and safeguards are otherwise the same.

The transfer differs from a container download, which can use the engine's file copy
API. Read the file in an SSH session as the login user, like the server shell, not in
the root agent service. Normal file permissions decide access, so `root` can download
any file and the dedicated `tha` user only files it can read. The restricted key
keeps its forced command, and plain `scp` or `sftp` stay unavailable.

Server downloads need their own opt-in, `--allow-server-download`, separate from
container downloads (`--allow-download`) and `--allow-exec`. Enabling them lets every holder of an authorized key read whatever
the login user can read, so a root login exposes files such as `/etc/shadow` and
private keys. Show this in the setup warning.

### Size preview and folders

The dialog shows the size before anything downloads, plus the modified time, and warns
if the destination has too little free space.

- **Files:** the size is available immediately from file metadata.
- **Folders on a server:** the SSH session walks the folder and adds up file sizes
  without transferring any contents.
- **Folders in a container:** the engine reports no total for a folder. Tha reads the
  engine's archive stream and adds up its entries, running no command in the container,
  so this also works without `du` or exec. On a server, the agent does the counting and
  sends back only the total.

Counting a folder happens in the background, so the dialog opens at once and shows a
running total, such as `at least 1.2 GB, 3,400 files`, then the final total, such as
`4.7 GB, 12,860 files`. In a container it reads the folder's full contents on the host, which can be slow and disk-heavy, so it
runs at low priority and stops as soon as the dialog closes. Users need not wait for it
before starting the download. A size can also change between the preview and the
download, so the total is an estimate unless the file is no longer changing.

A folder is saved as one `.tar` archive, for example `~/Downloads/data.tar`. Tha does not
unpack it, which avoids writing archive paths and symlinks directly onto the user's disk.

### Limits and permissions

A typical use is a database backup: a user creates a `pg_dump` in a container's
shell, then downloads it. The user runs their own commands. Tha never creates
backups. Apart from `ls` for browsing, it runs no commands for this feature, and it only copies
the file or folder the user names.

Backups can be large and contain all of a database's data. Show progress, allow
cancelling, stream instead of buffering in memory, never overwrite a local file,
create it readable only by the user, and remove partial files after a failure. A
cancelled download starts again; resuming is not planned for the first version.

On servers, the agent refuses file copy today. Use separate server opt-ins for
copying out and copying in, because copying in can change a running workload.

## Remote actions

**Finished 2026-11-01:** each is its own server opt-in, and `--allow-all` turns on all of them. The README lists them.

The agent currently refuses these operations, so each one widens what an authorized
key can do. Make them server opt-in, independently of each other, like
`--allow-exec`, and keep them on the agent's explicit allowlist.

Compose actions need the project files on the server, and projects started elsewhere
must still be refused with a clear reason. Compose runs with engine-level access, and
its project path comes from container labels, so a bad label or a Compose file others
can edit could mean root on the host. The agent must validate the path and the file's
ownership, and `--allow-compose` needs a warning like the one for `--allow-exec`.

## Compose editing through an external editor

Compose editing uses the user's editor, not a built-in Compose editor. Open the
explicitly associated Compose files in the selected editor and return to Tha after
editing. Saving must not apply changes or restart workloads; any validation or
Compose apply action is explicit.

### Editor setting

The editor is chosen in Settings (`,`) and stored in `config.toml`, for example
`editor = "vim"`. The default is **vim**. The supported editors are vim, Neovim,
nano, and VS Code. Tha starts each with the right command, including `code --wait`
for VS Code, and does not read `VISUAL` or `EDITOR`. If the selected editor is not
installed, Tha says so and points to the setting instead of falling back to another
editor.

### Remote files

Nothing is mounted. Tha downloads the file through the existing SSH connection, opens
a private temporary copy in the user's local editor, and uploads it after the editor
exits. vim, Neovim, and nano take over the terminal until they quit. With VS Code,
the upload happens when the file's tab closes, not on each save.

- **Permissions:** the server decides. Editing needs a server opt-in, and the server
  reports per file whether the login user can write. Without access, the file opens
  read-only or the action is hidden, with the reason shown. The check repeats when
  saving.
- **Conflicts:** the upload includes the hash of the downloaded version. If the file
  changed on the server meanwhile, refuse to overwrite and show the difference.
- **Saving:** write atomically, keep a backup, preserve owner and mode, and delete
  the temporary copy afterwards. A failed upload must keep the local copy so edits
  are not lost.
- **Which user writes:** do the file access in the SSH session as the login user,
  not in the root agent service. The service is sandboxed from writing outside its
  own directories (`ProtectSystem=strict`, `ProtectHome=yes`) and should stay that
  way. Normal file permissions then decide access: `root` can edit everything, while
  the dedicated `tha` user can edit only files an admin has given it access to.
- **Risk:** anyone who can both edit a Compose file and apply it can mount the host
  filesystem into a container, which effectively gives them root. This also applies
  to the `tha` user, so document it like `--allow-exec`.

Multiple Compose files and included files still need scoping.

## Alerts

Build alerts in two steps.

**Finished 2026-11-01:** `:alerts`, the `⚠` count, and an opt-in webhook on servers
(`--alert-webhook`). High memory is in-app only.

### Step 1: in-app alerts

Tha evaluates rules on the data it already collects and needs no new server
permissions. Thresholds are configurable; for example:

- **Crash loop:** a container restarted 5 times in 10 minutes.
- **Out of memory:** a container was killed for running out of memory.
- **Health:** a container turned unhealthy.
- **High memory:** usage stayed above 90% for 5 minutes.
- **Server down:** a connected server became unreachable.

The header shows a count from any view, such as `⚠ 2`. A list shows what happened,
where, and when, and `Enter` jumps to the container's logs or diagnostics. Each
incident appears once with a rising count and clears when the problem stops.

These alerts appear only while Tha is open. A crash that recovered before Tha opened
is missed, and the UI must not imply otherwise.

### Step 2: agent-side alerts

The remote agent already samples metrics and runs scans while Tha is closed, so it
can evaluate rules around the clock, including a full disk or new critical
vulnerabilities. Tha then shows what happened while it was closed. Webhooks come
last, because they add outbound traffic and secrets on the server.

Alerts should distinguish an observed failure from stale or unavailable data.

## Update safety

Existing Compose updates stop on failure but have no rollback. Recording previous
image identities is useful evidence, not a guarantee that a deployment can be
rolled back safely: configuration, mutable tags, and application data migrations
need separate consideration. Any recovery action should be explicit.

## Diagnostic exports

Diagnostic exports must be opt-in. Logs and health-check output can contain
secrets; redaction cannot guarantee their removal. Preview the scope and contents
before writing or sharing, and do not upload automatically.

## Improvements to existing features

### 1. Make health diagnostics useful immediately

**Finished 2026-11-01.** `L` in the health view loads up to 300 lines for that exact
container, around a known failure or otherwise the latest lines. They stay in the view
until it closes and are not saved.

### 2. Add keyword-based log issue markers

**Finished 2026-11-01,** except jumping from a diagnostic finding straight to its log entry. `L` in
the diagnostics view shows those lines there instead. Defaults are `i` and `I`.

Start simple: scan loaded log messages locally for common, case-insensitive error
keywords and phrases such as `error`, `fatal`, `panic`, `exception`, `traceback`,
`stack trace`, `connection refused`, `timeout`, and `out of memory`. Use sensible
word boundaries for individual keywords. No AI analysis or confidence scoring is
needed for the first version; matches flag possible issues, not proven failures.

- Highlight matching entries without hiding the surrounding context.
- Provide configurable **next issue / previous issue** commands with single-key
  defaults, because navigation bindings do not support key sequences such as `]q`.
  Avoid `[` and `]` in the German preset, where they need AltGr.
- Add issue markers to the log scrollbar so users can see where matches occur.
- Show the current issue position, for example **Issue 3 of 12**.
- Let diagnostic findings jump to the corresponding entry in Logs, including
  matches in logs loaded around a failure.
- Where a multiline stack trace is identifiable, group it as one issue/jump
  target rather than stopping on every trace line.

Keep issue navigation separate from ordinary search. Respect active filters and
indicate when they hide flagged entries. Markers cover only loaded/retained logs,
not the container's complete history. Keyword matching is intentionally heuristic
and can produce false positives or miss failures.

### 3. Extend network troubleshooting with opt-in tests

**Finished 2026-11-01:** `:network-test host:port`.

Current network diagnostics are passive: they explain reported configuration but
do not test connectivity.

Consider explicit DNS and TCP checks showing:

- Where the test runs.
- Which destination it contacts.
- Whether exec or a helper is required.
- The timeout and result.

Keep passive inspection the default. Active tests require separate design and
permission review, especially for remote servers; existing browsing permissions
must not implicitly authorize exec or helper containers.

### 4. Improve history for real investigations

Current persistent metrics retention is 24 hours. Consider:

- **Finished 2026-11-01:** configurable retention with disk limits (`history_retention_hours`), local service only.
- Downsampled longer-term history.
- Comparing current usage with yesterday.
- Clear coverage and collection-gap indicators.
- **Finished 2026-11-01:** saved container events on servers. The local service already saved
  them; the agent now does too, so `:timeline` can explain what happened overnight.

Preserve the distinction between missing data and zero usage, and explain the
different coverage of standalone, local service, and remote agent modes.

### 5. Make runtime differences clearer

**Finished 2026-11-01.** `:capabilities` shows, per connection, what is available, turned
off, missing a dependency, unsupported, unreachable, or unknown, and what to do about it.
The agent now reports its opt-ins when Tha connects.

### 6. Refine log filtering

**Finished 2026-11-01:** `-word` excludes and `\-word` is literal.

Add exclude terms such as `-healthcheck`. The text filter deliberately has no
operator syntax today, so this needs a small design decision about how to match a
literal leading `-`.

### 7. Reload configuration live

**Finished 2026-11-01:** the file is reloaded within a few seconds; connections still need a restart.

Key binding changes currently require a restart. Apply them without one, and keep
rejecting duplicate keys and unknown actions.

## Maintenance

**Finished 2026-11-01.**

- **Finished: corrected the guide.** Exec and background metrics history are no longer
  listed as unavailable on servers.
- **Finished: retired the MVP wording** in the guide and in app messages. The guide's
  next steps now point to this file.
- **Finished: tidied the repository.** `handoff-podman.md` is removed, and the three
  OpenSpec changes are archived after capping inspect responses before parsing.
- **Finished: the performance work.** Attachments are indexed once per snapshot,
  inventory requests overlap, and Explorer resource grouping and search no longer rescan.
  A cached Explorer tree was measured and not added; `performance-review.md` explains why.
- **Finished: split the largest files.** Tests moved to `main_tests.rs`, `app/tests.rs`,
  and `ui/tests.rs`, and input handling moved to `main_input.rs`.

## Permanently out of scope

- **Kubernetes management:** Tha will not add Kubernetes management. This is a
  product boundary, not a deferred roadmap item.
- **Windows and Intel Mac builds:** Tha supports Linux amd64/arm64 and Apple Silicon
  Macs only.

## Defer for now

- Automatic remediation.

Keep remediation explicit while the existing troubleshooting workflows are
being connected.

## References

- [User guide](guide.md): current workflows, limitations, and safety model.
- [Feature handoff](feature-handoff.md): recently implemented log filtering,
  health diagnostics, and passive network troubleshooting.
- [Remote history](remote-history.md).
- [Remote security](remote-security.md).
